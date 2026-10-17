package worker

import (
	"context"
	"fmt"
	"log/slog"
	"runtime/debug"
	"sync"
	"time"

	"github.com/acme/jobqueue/internal/deadletter"
	"github.com/acme/jobqueue/internal/job"
	"github.com/acme/jobqueue/internal/metrics"
	"github.com/acme/jobqueue/internal/queue"
	"github.com/acme/jobqueue/internal/retry"
)

// Pool runs up to Size handlers at once, pulling work from a broker.
type Pool struct {
	Size     int
	PerKind  map[string]int // optional per-kind concurrency caps
	Broker   *queue.Broker
	Handlers *Registry
	Policy   retry.Policy
	Parker   *deadletter.Parker
	LeaseFor time.Duration
	Timeout  time.Duration
	Log      *slog.Logger
	Metrics  *metrics.Registry

	wg    sync.WaitGroup
	slots map[string]chan struct{}
}

// Run starts the workers and blocks until ctx is cancelled and every
// in-flight handler has returned.
func (p *Pool) Run(ctx context.Context) {
	p.slots = map[string]chan struct{}{}
	for kind, n := range p.PerKind {
		p.slots[kind] = make(chan struct{}, n)
	}
	for i := 0; i < p.Size; i++ {
		p.wg.Add(1)
		go p.loop(ctx, i)
	}
	p.wg.Wait()
}

func (p *Pool) loop(ctx context.Context, id int) {
	defer p.wg.Done()
	for {
		j, err := p.Broker.Take(ctx)
		if err != nil {
			return
		}
		if slot, ok := p.slots[j.Kind]; ok {
			select {
			case slot <- struct{}{}:
			case <-ctx.Done():
				return
			}
			p.execute(j, id)
			<-slot
			continue
		}
		p.execute(j, id)
	}
}

// execute runs one job. It deliberately uses a background context so a
// shutdown signal does not abort a handler half-way; Timeout still bounds it.
func (p *Pool) execute(j *job.Job, workerID int) {
	ctx, cancel := context.WithTimeout(context.Background(), p.Timeout)
	defer cancel()
	stop := queue.Heartbeat(ctx, p.Broker.Store(), j, p.Broker.Owner(), p.LeaseFor, cancel)
	defer stop()

	start := time.Now()
	err := p.safeCall(ctx, j)
	p.Metrics.Observe("job_duration_seconds", time.Since(start).Seconds(), "kind", j.Kind)
	p.finish(j, err, workerID)
}

func (p *Pool) safeCall(ctx context.Context, j *job.Job) (err error) {
	defer func() {
		if r := recover(); r != nil {
			err = fmt.Errorf("handler panic: %v\n%s", r, debug.Stack())
		}
	}()
	return p.Handlers.Lookup(j.Kind)(ctx, j)
}

func (p *Pool) finish(j *job.Job, err error, workerID int) {
	ctx := context.Background()
	st, owner, now := p.Broker.Store(), p.Broker.Owner(), time.Now()
	log := p.Log.With("job", j.ID, "kind", j.Kind, "tenant", j.Tenant, "attempt", j.Attempts, "worker", workerID)

	switch {
	case err == nil:
		j.State, j.LeaseOwner, j.UpdatedAt = job.StateSucceeded, "", now
		if uerr := st.Update(ctx, j, owner); uerr != nil {
			log.Warn("could not record success", "err", uerr)
		}
		p.Metrics.Inc("jobs_succeeded_total", "kind", j.Kind)
	case p.Policy.ShouldRetry(err, j.Attempts):
		j.State, j.LeaseOwner, j.LastError = job.StatePending, "", err.Error()
		j.RunAt, j.UpdatedAt = p.Policy.Next(err, j.Attempts, now), now
		_ = st.Update(ctx, j, owner)
		log.Info("job failed, will retry", "err", err, "next", j.RunAt)
		p.Metrics.Inc("jobs_retried_total", "kind", j.Kind)
	default:
		if perr := p.Parker.Park(ctx, j, owner, err); perr != nil {
			log.Error("could not park job", "err", perr)
		}
		log.Error("job parked", "err", err)
		p.Metrics.Inc("jobs_dead_total", "kind", j.Kind)
	}
}
