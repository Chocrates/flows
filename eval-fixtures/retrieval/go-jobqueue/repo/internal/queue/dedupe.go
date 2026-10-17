package queue

import (
	"context"
	"errors"
	"sync"
	"time"

	"github.com/acme/jobqueue/internal/job"
	"github.com/acme/jobqueue/internal/store"
)

// keyGuard makes repeated submissions carrying the same client-supplied key
// return the original job instead of creating a duplicate. Storage enforces
// uniqueness; the in-process map only collapses concurrent requests so the
// loser does not see a raw conflict error.
type keyGuard struct {
	st       store.Store
	mu       sync.Mutex
	inflight map[string]chan struct{}
	window   time.Duration
}

func newKeyGuard(st store.Store, window time.Duration) *keyGuard {
	return &keyGuard{st: st, inflight: map[string]chan struct{}{}, window: window}
}

// lookup returns a previously created job for (tenant, key) if it is still
// inside the replay window.
func (g *keyGuard) lookup(ctx context.Context, tenant, key string, now time.Time) (*job.Job, error) {
	if key == "" {
		return nil, nil
	}
	prev, err := g.st.FindByKey(ctx, tenant, key)
	if errors.Is(err, store.ErrNotFound) {
		return nil, nil
	}
	if err != nil {
		return nil, err
	}
	if g.window > 0 && now.Sub(prev.CreatedAt) > g.window {
		return nil, nil
	}
	return prev, nil
}

// acquire serialises work on one key. The returned func must be called.
func (g *keyGuard) acquire(ctx context.Context, tenant, key string) (func(), error) {
	if key == "" {
		return func() {}, nil
	}
	k := tenant + "/" + key
	for {
		g.mu.Lock()
		ch, busy := g.inflight[k]
		if !busy {
			ch = make(chan struct{})
			g.inflight[k] = ch
			g.mu.Unlock()
			return func() {
				g.mu.Lock()
				delete(g.inflight, k)
				g.mu.Unlock()
				close(ch)
			}, nil
		}
		g.mu.Unlock()
		select {
		case <-ch:
		case <-ctx.Done():
			return nil, ctx.Err()
		}
	}
}
