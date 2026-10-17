// Package worker runs job handlers.
package worker

import (
	"context"
	"encoding/json"
	"fmt"
	"sync"

	"github.com/acme/jobqueue/internal/job"
	"github.com/acme/jobqueue/internal/retry"
)

// HandlerFunc processes one job. Returning retry.Permanent(err) skips retries.
type HandlerFunc func(ctx context.Context, j *job.Job) error

// Registry maps a job kind to the code that runs it.
type Registry struct {
	mu       sync.RWMutex
	handlers map[string]HandlerFunc
}

// NewRegistry returns an empty registry.
func NewRegistry() *Registry { return &Registry{handlers: map[string]HandlerFunc{}} }

// Register binds kind to fn. Registering the same kind twice panics.
func (r *Registry) Register(kind string, fn HandlerFunc) {
	r.mu.Lock()
	defer r.mu.Unlock()
	if _, dup := r.handlers[kind]; dup {
		panic("worker: duplicate handler for " + kind)
	}
	r.handlers[kind] = fn
}

// Lookup returns the handler for kind, or one that fails permanently.
func (r *Registry) Lookup(kind string) HandlerFunc {
	r.mu.RLock()
	fn, ok := r.handlers[kind]
	r.mu.RUnlock()
	if ok {
		return fn
	}
	return func(context.Context, *job.Job) error {
		return retry.Permanent(fmt.Errorf("no handler registered for kind %q", kind))
	}
}

// Kinds lists registered kinds.
func (r *Registry) Kinds() []string {
	r.mu.RLock()
	defer r.mu.RUnlock()
	out := make([]string, 0, len(r.handlers))
	for k := range r.handlers {
		out = append(out, k)
	}
	return out
}

// Decode unmarshals a job payload into v, marking decode failures permanent.
func Decode(j *job.Job, v any) error {
	if err := json.Unmarshal(j.Payload, v); err != nil {
		return retry.Permanent(fmt.Errorf("decode %s payload: %w", j.Kind, err))
	}
	return nil
}
