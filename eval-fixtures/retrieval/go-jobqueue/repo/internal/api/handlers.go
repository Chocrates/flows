// Package api exposes the HTTP interface for producers and operators.
package api

import (
	"encoding/json"
	"errors"
	"io"
	"log/slog"
	"net/http"
	"strings"
	"time"

	"github.com/acme/jobqueue/internal/deadletter"
	"github.com/acme/jobqueue/internal/job"
	"github.com/acme/jobqueue/internal/logging"
	"github.com/acme/jobqueue/internal/metrics"
	"github.com/acme/jobqueue/internal/queue"
	"github.com/acme/jobqueue/internal/ratelimit"
	"github.com/acme/jobqueue/internal/store"
)

const tenantHeader = "X-Tenant"

// Server bundles dependencies for the HTTP handlers.
type Server struct {
	Broker  *queue.Broker
	Parker  *deadletter.Parker
	Limiter *ratelimit.Limiter
	Metrics *metrics.Registry
	Log     *slog.Logger
}

// Routes builds the mux with middleware applied.
func (s *Server) Routes() http.Handler {
	mux := http.NewServeMux()
	mux.HandleFunc("POST /v1/jobs", s.submit)
	mux.HandleFunc("GET /v1/jobs/{id}", s.show)
	mux.HandleFunc("GET /v1/jobs", s.list)
	mux.HandleFunc("POST /v1/dead/{id}/replay", s.replay)
	mux.HandleFunc("DELETE /v1/dead", s.purge)
	mux.HandleFunc("GET /v1/stats", s.stats)
	mux.Handle("GET /metrics", s.Metrics.Handler())
	mux.HandleFunc("GET /healthz", func(w http.ResponseWriter, _ *http.Request) { w.WriteHeader(http.StatusNoContent) })
	return accessLog(s.Log, s.Metrics, throttle(s.Limiter, s.Metrics, mux))
}

func writeJSON(w http.ResponseWriter, code int, v any) {
	w.Header().Set("Content-Type", "application/json")
	w.WriteHeader(code)
	_ = json.NewEncoder(w).Encode(v)
}

func (s *Server) submit(w http.ResponseWriter, r *http.Request) {
	tenant := strings.TrimSpace(r.Header.Get(tenantHeader))
	if tenant == "" {
		writeJSON(w, http.StatusUnauthorized, map[string]string{"error": job.ErrEmptyTenant.Error()})
		return
	}
	var sub job.Submission
	body := io.LimitReader(r.Body, job.MaxPayloadBytes+4096)
	if err := json.NewDecoder(body).Decode(&sub); err != nil {
		writeJSON(w, http.StatusBadRequest, map[string]string{"error": "malformed JSON body"})
		return
	}
	if key := r.Header.Get("Idempotency-Key"); key != "" && sub.IdempotencyKey == "" {
		sub.IdempotencyKey = key
	}
	if err := sub.Check(); err != nil {
		var p job.Problems
		if errors.As(err, &p) {
			writeJSON(w, http.StatusUnprocessableEntity, map[string]any{"error": "invalid job", "problems": p})
			return
		}
		writeJSON(w, http.StatusBadRequest, map[string]string{"error": err.Error()})
		return
	}
	j, replayed, err := s.Broker.Enqueue(r.Context(), tenant, &sub)
	if err != nil {
		logging.From(r.Context()).Error("submit failed", "err", err)
		writeJSON(w, http.StatusInternalServerError, map[string]string{"error": "could not store job"})
		return
	}
	if replayed {
		w.Header().Set("Idempotent-Replayed", "true")
		writeJSON(w, http.StatusOK, j)
		return
	}
	s.Metrics.Inc("jobs_submitted_total", "kind", j.Kind)
	writeJSON(w, http.StatusAccepted, j)
}

func (s *Server) show(w http.ResponseWriter, r *http.Request) {
	j, err := s.Broker.Store().Get(r.Context(), r.PathValue("id"))
	if errors.Is(err, store.ErrNotFound) || (err == nil && j.Tenant != r.Header.Get(tenantHeader)) {
		writeJSON(w, http.StatusNotFound, map[string]string{"error": "not found"})
		return
	}
	writeJSON(w, http.StatusOK, j)
}

func (s *Server) list(w http.ResponseWriter, r *http.Request) {
	q := r.URL.Query()
	jobs, err := s.Broker.Store().List(r.Context(), store.Filter{
		Tenant: r.Header.Get(tenantHeader), State: job.State(q.Get("state")), Kind: q.Get("kind"), Limit: 100,
	})
	if err != nil {
		writeJSON(w, http.StatusInternalServerError, map[string]string{"error": err.Error()})
		return
	}
	writeJSON(w, http.StatusOK, jobs)
}

func (s *Server) replay(w http.ResponseWriter, r *http.Request) {
	j, err := s.Parker.Revive(r.Context(), r.PathValue("id"))
	if err != nil {
		writeJSON(w, http.StatusConflict, map[string]string{"error": err.Error()})
		return
	}
	writeJSON(w, http.StatusOK, j)
}

func (s *Server) purge(w http.ResponseWriter, r *http.Request) {
	age, err := time.ParseDuration(r.URL.Query().Get("older_than"))
	if err != nil {
		age = 7 * 24 * time.Hour
	}
	n, err := s.Parker.Purge(r.Context(), r.Header.Get(tenantHeader), time.Now().Add(-age))
	if err != nil {
		writeJSON(w, http.StatusInternalServerError, map[string]string{"error": err.Error()})
		return
	}
	writeJSON(w, http.StatusOK, map[string]int{"deleted": n})
}

func (s *Server) stats(w http.ResponseWriter, r *http.Request) {
	counts, err := s.Broker.Store().Counts(r.Context())
	if err != nil {
		writeJSON(w, http.StatusInternalServerError, map[string]string{"error": err.Error()})
		return
	}
	writeJSON(w, http.StatusOK, counts)
}
