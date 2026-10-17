// Package logging configures the process-wide structured logger.
package logging

import (
	"context"
	"io"
	"log/slog"
	"os"
	"strings"
)

type ctxKey struct{}

// New builds a logger writing JSON (or text, for local dev) at level.
func New(w io.Writer, format, level string) *slog.Logger {
	if w == nil {
		w = os.Stderr
	}
	opts := &slog.HandlerOptions{Level: parseLevel(level)}
	var h slog.Handler
	if strings.EqualFold(format, "text") {
		h = slog.NewTextHandler(w, opts)
	} else {
		h = slog.NewJSONHandler(w, opts)
	}
	return slog.New(h).With("service", "jobqueue")
}

func parseLevel(s string) slog.Level {
	switch strings.ToLower(s) {
	case "debug":
		return slog.LevelDebug
	case "warn", "warning":
		return slog.LevelWarn
	case "error":
		return slog.LevelError
	default:
		return slog.LevelInfo
	}
}

// WithContext stores l in ctx so request-scoped fields (request id, tenant)
// follow the call chain.
func WithContext(ctx context.Context, l *slog.Logger) context.Context {
	return context.WithValue(ctx, ctxKey{}, l)
}

// From returns the logger stored in ctx, or the default logger.
func From(ctx context.Context) *slog.Logger {
	if l, ok := ctx.Value(ctxKey{}).(*slog.Logger); ok {
		return l
	}
	return slog.Default()
}
