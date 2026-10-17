package queue

import (
	"context"
	"log/slog"
	"time"

	"github.com/acme/jobqueue/internal/job"
	"github.com/acme/jobqueue/internal/store"
)

// Heartbeat keeps a claimed job invisible to other nodes while a handler is
// still working on it. It extends the deadline every third of the lease.
// Cancel the returned func when the handler finishes.
func Heartbeat(ctx context.Context, st store.Store, j *job.Job, owner string, leaseFor time.Duration, onLost func()) context.CancelFunc {
	ctx, cancel := context.WithCancel(ctx)
	go func() {
		t := time.NewTicker(leaseFor / 3)
		defer t.Stop()
		for {
			select {
			case <-ctx.Done():
				return
			case now := <-t.C:
				if err := st.Extend(ctx, j.ID, owner, now.Add(leaseFor)); err != nil {
					if err == store.ErrLeaseLost && onLost != nil {
						onLost()
					}
					return
				}
			}
		}
	}()
	return cancel
}

// Reaper returns jobs whose holder vanished (crash, OOM kill, network
// partition) back to pending so another node can pick them up.
type Reaper struct {
	St       store.Store
	Every    time.Duration
	Batch    int
	Log      *slog.Logger
	OnReturn func(*job.Job)
}

// Run loops until ctx is cancelled.
func (r *Reaper) Run(ctx context.Context) {
	t := time.NewTicker(r.Every)
	defer t.Stop()
	for {
		select {
		case <-ctx.Done():
			return
		case now := <-t.C:
			r.sweep(ctx, now)
		}
	}
}

func (r *Reaper) sweep(ctx context.Context, now time.Time) {
	stale, err := r.St.Expired(ctx, now, r.Batch)
	if err != nil {
		r.Log.Warn("lease sweep failed", "err", err)
		return
	}
	for _, j := range stale {
		prevOwner := j.LeaseOwner
		j.State = job.StatePending
		j.LeaseOwner = ""
		j.LeasedUntil = time.Time{}
		j.RunAt = now
		j.UpdatedAt = now
		if err := r.St.Update(ctx, j, prevOwner); err != nil {
			continue // owner came back and wrote first
		}
		r.Log.Info("lease expired, job returned", "job", j.ID, "owner", prevOwner, "attempts", j.Attempts)
		if r.OnReturn != nil {
			r.OnReturn(j)
		}
	}
}

// Release hands a claimed-but-unstarted job back immediately.
func Release(ctx context.Context, st store.Store, j *job.Job, owner string) error {
	j.State = job.StatePending
	j.Attempts--
	j.LeaseOwner = ""
	j.LeasedUntil = time.Time{}
	return st.Update(ctx, j, owner)
}
