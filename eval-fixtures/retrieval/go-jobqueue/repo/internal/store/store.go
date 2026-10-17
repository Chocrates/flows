// Package store persists jobs. Implementations must be safe for concurrent use.
package store

import (
	"context"
	"errors"
	"time"

	"github.com/acme/jobqueue/internal/job"
)

var (
	// ErrNotFound is returned when no job has the requested ID.
	ErrNotFound = errors.New("store: job not found")
	// ErrConflict is returned when a write races another writer.
	ErrConflict = errors.New("store: conflicting update")
	// ErrLeaseLost is returned when a worker acts on a job it no longer owns.
	ErrLeaseLost = errors.New("store: lease no longer held")
)

// Filter narrows a listing.
type Filter struct {
	Tenant string
	State  job.State
	Kind   string
	Limit  int
}

// Store is the persistence boundary. The Postgres implementation maps each
// method to a single statement or short transaction.
type Store interface {
	Insert(ctx context.Context, j *job.Job) error
	Get(ctx context.Context, id string) (*job.Job, error)
	FindByKey(ctx context.Context, tenant, key string) (*job.Job, error)
	List(ctx context.Context, f Filter) ([]*job.Job, error)

	// Claim atomically picks up to n runnable jobs, marks them running and
	// hides them from other claimers until leaseUntil.
	Claim(ctx context.Context, owner string, n int, now, leaseUntil time.Time) ([]*job.Job, error)
	// Extend pushes the lease deadline of a job held by owner.
	Extend(ctx context.Context, id, owner string, until time.Time) error
	// Update writes back a job; owner must still hold the lease if non-empty.
	Update(ctx context.Context, j *job.Job, owner string) error
	// Expired returns running jobs whose lease ended before now.
	Expired(ctx context.Context, now time.Time, limit int) ([]*job.Job, error)

	Delete(ctx context.Context, id string) error
	Counts(ctx context.Context) (map[job.State]int, error)
}
