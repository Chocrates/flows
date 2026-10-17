// Package deadletter parks jobs that exhausted their attempts so an operator
// can inspect, replay or discard them.
package deadletter

import (
	"context"
	"time"

	"github.com/acme/jobqueue/internal/job"
	"github.com/acme/jobqueue/internal/store"
)

// Parker moves jobs into and out of the dead state.
type Parker struct {
	St  store.Store
	Now func() time.Time
}

// Park marks j dead with the final error. owner must hold the lease.
func (p *Parker) Park(ctx context.Context, j *job.Job, owner string, cause error) error {
	j.State = job.StateDead
	j.LastError = truncate(cause.Error(), 2048)
	j.LeaseOwner = ""
	j.LeasedUntil = time.Time{}
	j.UpdatedAt = p.Now()
	return p.St.Update(ctx, j, owner)
}

// Revive puts a dead job back to pending with a fresh attempt budget.
func (p *Parker) Revive(ctx context.Context, id string) (*job.Job, error) {
	j, err := p.St.Get(ctx, id)
	if err != nil {
		return nil, err
	}
	if j.State != job.StateDead {
		return nil, store.ErrConflict
	}
	now := p.Now()
	j.State = job.StatePending
	j.Attempts = 0
	j.RunAt = now
	j.UpdatedAt = now
	return j, p.St.Update(ctx, j, "")
}

// Purge deletes dead jobs older than cutoff and returns how many were removed.
func (p *Parker) Purge(ctx context.Context, tenant string, cutoff time.Time) (int, error) {
	dead, err := p.St.List(ctx, store.Filter{Tenant: tenant, State: job.StateDead})
	if err != nil {
		return 0, err
	}
	n := 0
	for _, j := range dead {
		if j.UpdatedAt.Before(cutoff) {
			if err := p.St.Delete(ctx, j.ID); err == nil {
				n++
			}
		}
	}
	return n, nil
}

func truncate(s string, n int) string {
	if len(s) <= n {
		return s
	}
	return s[:n] + "…"
}
