package api

import (
	"crypto/rand"
	"encoding/hex"
	"log/slog"
	"net/http"
	"strconv"
	"time"

	"github.com/acme/jobqueue/internal/logging"
	"github.com/acme/jobqueue/internal/metrics"
	"github.com/acme/jobqueue/internal/ratelimit"
)

type statusRecorder struct {
	http.ResponseWriter
	status int
}

func (s *statusRecorder) WriteHeader(code int) {
	s.status = code
	s.ResponseWriter.WriteHeader(code)
}

// accessLog attaches a request id and logs one line per request.
func accessLog(base *slog.Logger, m *metrics.Registry, next http.Handler) http.Handler {
	return http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		var b [8]byte
		_, _ = rand.Read(b[:])
		reqID := hex.EncodeToString(b[:])
		w.Header().Set("X-Request-Id", reqID)

		log := base.With("req", reqID, "tenant", r.Header.Get(tenantHeader))
		rec := &statusRecorder{ResponseWriter: w, status: http.StatusOK}
		start := time.Now()
		next.ServeHTTP(rec, r.WithContext(logging.WithContext(r.Context(), log)))
		dur := time.Since(start)
		log.Info("http", "method", r.Method, "path", r.URL.Path, "status", rec.status, "dur_ms", dur.Milliseconds())
		m.Observe("http_request_seconds", dur.Seconds(), "path", r.URL.Path, "code", strconv.Itoa(rec.status))
	})
}

// throttle rejects producers that exceed their tenant's allowance.
func throttle(l *ratelimit.Limiter, m *metrics.Registry, next http.Handler) http.Handler {
	return http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if r.Method != http.MethodPost {
			next.ServeHTTP(w, r)
			return
		}
		tenant := r.Header.Get(tenantHeader)
		if ok, wait := l.Allow(tenant); !ok {
			secs := int(wait.Seconds()) + 1
			w.Header().Set("Retry-After", strconv.Itoa(secs))
			m.Inc("http_throttled_total", "tenant", tenant)
			writeJSON(w, http.StatusTooManyRequests, map[string]string{"error": "rate limit exceeded"})
			return
		}
		next.ServeHTTP(w, r)
	})
}
