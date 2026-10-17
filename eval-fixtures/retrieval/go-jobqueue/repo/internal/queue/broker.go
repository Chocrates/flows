// Package queue moves jobs between producers, storage and the worker pool.
package queue

import (
	"context"
	"crypto/rand"
	"encoding/hex"
	"time"

	"github.com/acme/jobqueue/internal/job"
	"github.com/acme/jobqueue/internal/store"
)

// Broker accepts new jobs and feeds claimed jobs to workers.
type Broker struct {
	st         store.Store
	buf        *Buffer
	guard      *keyGuard
	owner      string
	leaseFor   time.Duration
	defAttempt int
	now        func() time.Time
}

// Options configures a Broker.
type Options struct {
	Owner          string
	LeaseFor       time.Duration
	BufferSize     int
	MaxAttempts    int
	ReplayWindow   time.Duration
}

// NewBroker wires a broker over st.
func NewBroker(st store.Store, o Options) *Broker {
	return &Broker{
		st:         st,
		buf:        NewBuffer(o.BufferSize),
		guard:      newKeyGuard(st, o.ReplayWindow),
		owner:      o.Owner,
		leaseFor:   o.LeaseFor,
		defAttempt: o.MaxAttempts,
		now:        time.Now,
	}
}

func newID() string {
	var b [12]byte
	_, _ = rand.Read(b[:])
	return hex.EncodeToString(b[:])
}

// Enqueue stores a validated submission for tenant. The bool result is true
// when an existing job was returned because of a repeated key.
func (b *Broker) Enqueue(ctx context.Context, tenant string, s *job.Submission) (*job.Job, bool, error) {
	release, err := b.guard.acquire(ctx, tenant, s.IdempotencyKey)
	if err != nil {
		return nil, false, err
	}
	defer release()

	now := b.now()
	if prev, err := b.guard.lookup(ctx, tenant, s.IdempotencyKey, now); err != nil || prev != nil {
		return prev, prev != nil, err
	}

	j := &job.Job{
		ID:             newID(),
		Tenant:         tenant,
		Kind:           s.Kind,
		Payload:        s.Payload,
		Priority:       job.PriorityNormal,
		State:          job.StatePending,
		MaxAttempts:    b.defAttempt,
		IdempotencyKey: s.IdempotencyKey,
		RunAt:          now,
		CreatedAt:      now,
		UpdatedAt:      now,
	}
	if s.Priority != nil {
		j.Priority = job.Priority(*s.Priority)
	}
	if s.MaxAttempts > 0 {
		j.MaxAttempts = s.MaxAttempts
	}
	if s.Delay != "" {
		d, _ := time.ParseDuration(s.Delay)
		j.RunAt = now.Add(d)
	}
	if err := b.st.Insert(ctx, j); err != nil {
		return nil, false, err
	}
	return j, false, nil
}

// Fill claims as many jobs as the buffer has room for.
func (b *Broker) Fill(ctx context.Context) (int, error) {
	free := b.buf.Free()
	if free <= 0 {
		return 0, nil
	}
	now := b.now()
	jobs, err := b.st.Claim(ctx, b.owner, free, now, now.Add(b.leaseFor))
	if err != nil {
		return 0, err
	}
	b.buf.Add(jobs...)
	return len(jobs), nil
}

// Take blocks until a job is available or ctx ends.
func (b *Broker) Take(ctx context.Context) (*job.Job, error) {
	for {
		if j := b.buf.Next(); j != nil {
			return j, nil
		}
		select {
		case <-b.buf.Wait():
		case <-time.After(250 * time.Millisecond):
			if _, err := b.Fill(ctx); err != nil {
				return nil, err
			}
		case <-ctx.Done():
			return nil, ctx.Err()
		}
	}
}

// Owner is the identity this broker claims leases under.
func (b *Broker) Owner() string { return b.owner }

// Store exposes the underlying store to collaborators.
func (b *Broker) Store() store.Store { return b.st }

// Unclaimed returns buffered jobs that never reached a worker.
func (b *Broker) Unclaimed() []*job.Job { return b.buf.Drain() }
