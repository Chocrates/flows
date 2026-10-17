package store

import (
	"context"
	"sort"
	"sync"
	"time"

	"github.com/acme/jobqueue/internal/job"
)

// Memory is an in-process Store used in tests and single-node deployments.
type Memory struct {
	mu   sync.Mutex
	jobs map[string]*job.Job
	keys map[string]string // tenant/key -> id
}

// NewMemory returns an empty Memory store.
func NewMemory() *Memory {
	return &Memory{jobs: map[string]*job.Job{}, keys: map[string]string{}}
}

func keyOf(tenant, key string) string { return tenant + "\x00" + key }

func (m *Memory) Insert(_ context.Context, j *job.Job) error {
	m.mu.Lock()
	defer m.mu.Unlock()
	if _, ok := m.jobs[j.ID]; ok {
		return ErrConflict
	}
	if j.IdempotencyKey != "" {
		k := keyOf(j.Tenant, j.IdempotencyKey)
		if _, ok := m.keys[k]; ok {
			return ErrConflict
		}
		m.keys[k] = j.ID
	}
	m.jobs[j.ID] = j.Clone()
	return nil
}

func (m *Memory) Get(_ context.Context, id string) (*job.Job, error) {
	m.mu.Lock()
	defer m.mu.Unlock()
	j, ok := m.jobs[id]
	if !ok {
		return nil, ErrNotFound
	}
	return j.Clone(), nil
}

func (m *Memory) FindByKey(ctx context.Context, tenant, key string) (*job.Job, error) {
	m.mu.Lock()
	id, ok := m.keys[keyOf(tenant, key)]
	m.mu.Unlock()
	if !ok {
		return nil, ErrNotFound
	}
	return m.Get(ctx, id)
}

func (m *Memory) List(_ context.Context, f Filter) ([]*job.Job, error) {
	m.mu.Lock()
	defer m.mu.Unlock()
	var out []*job.Job
	for _, j := range m.jobs {
		if (f.Tenant != "" && j.Tenant != f.Tenant) || (f.State != "" && j.State != f.State) || (f.Kind != "" && j.Kind != f.Kind) {
			continue
		}
		out = append(out, j.Clone())
	}
	sort.Slice(out, func(a, b int) bool { return out[a].CreatedAt.Before(out[b].CreatedAt) })
	if f.Limit > 0 && len(out) > f.Limit {
		out = out[:f.Limit]
	}
	return out, nil
}

func (m *Memory) Claim(_ context.Context, owner string, n int, now, leaseUntil time.Time) ([]*job.Job, error) {
	m.mu.Lock()
	defer m.mu.Unlock()
	var ready []*job.Job
	for _, j := range m.jobs {
		if j.Runnable(now) {
			ready = append(ready, j)
		}
	}
	sort.Slice(ready, func(a, b int) bool {
		if ready[a].Priority != ready[b].Priority {
			return ready[a].Priority > ready[b].Priority
		}
		return ready[a].RunAt.Before(ready[b].RunAt)
	})
	if len(ready) > n {
		ready = ready[:n]
	}
	out := make([]*job.Job, 0, len(ready))
	for _, j := range ready {
		j.State = job.StateRunning
		j.LeaseOwner = owner
		j.LeasedUntil = leaseUntil
		j.Attempts++
		j.UpdatedAt = now
		out = append(out, j.Clone())
	}
	return out, nil
}

func (m *Memory) Extend(_ context.Context, id, owner string, until time.Time) error {
	m.mu.Lock()
	defer m.mu.Unlock()
	j, ok := m.jobs[id]
	if !ok {
		return ErrNotFound
	}
	if j.State != job.StateRunning || j.LeaseOwner != owner {
		return ErrLeaseLost
	}
	j.LeasedUntil = until
	return nil
}

func (m *Memory) Update(_ context.Context, j *job.Job, owner string) error {
	m.mu.Lock()
	defer m.mu.Unlock()
	cur, ok := m.jobs[j.ID]
	if !ok {
		return ErrNotFound
	}
	if owner != "" && cur.LeaseOwner != owner {
		return ErrLeaseLost
	}
	m.jobs[j.ID] = j.Clone()
	return nil
}

func (m *Memory) Expired(_ context.Context, now time.Time, limit int) ([]*job.Job, error) {
	m.mu.Lock()
	defer m.mu.Unlock()
	var out []*job.Job
	for _, j := range m.jobs {
		if j.State == job.StateRunning && j.LeasedUntil.Before(now) {
			out = append(out, j.Clone())
			if limit > 0 && len(out) == limit {
				break
			}
		}
	}
	return out, nil
}

func (m *Memory) Delete(_ context.Context, id string) error {
	m.mu.Lock()
	defer m.mu.Unlock()
	j, ok := m.jobs[id]
	if !ok {
		return ErrNotFound
	}
	if j.IdempotencyKey != "" {
		delete(m.keys, keyOf(j.Tenant, j.IdempotencyKey))
	}
	delete(m.jobs, id)
	return nil
}

func (m *Memory) Counts(_ context.Context) (map[job.State]int, error) {
	m.mu.Lock()
	defer m.mu.Unlock()
	out := map[job.State]int{}
	for _, j := range m.jobs {
		out[j.State]++
	}
	return out, nil
}
