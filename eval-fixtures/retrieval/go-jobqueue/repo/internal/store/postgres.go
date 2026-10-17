package store

import (
	"context"
	"database/sql"
	"errors"
	"time"

	"github.com/acme/jobqueue/internal/job"
)

// Postgres stores jobs in a single `jobs` table. The driver is registered by
// the binary (blank import), keeping this package free of vendor imports.
type Postgres struct {
	db *sql.DB
}

// NewPostgres wraps an open database handle.
func NewPostgres(db *sql.DB) *Postgres { return &Postgres{db: db} }

const claimSQL = `
UPDATE jobs SET state = 'running', lease_owner = $1, leased_until = $3,
       attempts = attempts + 1, updated_at = $2
WHERE id IN (
    SELECT id FROM jobs
    WHERE state = 'pending' AND run_at <= $2
    ORDER BY priority DESC, run_at
    LIMIT $4
    FOR UPDATE SKIP LOCKED)
RETURNING id, tenant, kind, payload, priority, attempts, max_attempts,
          coalesce(idempotency_key, ''), run_at, created_at`

func (p *Postgres) Claim(ctx context.Context, owner string, n int, now, leaseUntil time.Time) ([]*job.Job, error) {
	rows, err := p.db.QueryContext(ctx, claimSQL, owner, now, leaseUntil, n)
	if err != nil {
		return nil, err
	}
	defer rows.Close()
	var out []*job.Job
	for rows.Next() {
		j := &job.Job{State: job.StateRunning, LeaseOwner: owner, LeasedUntil: leaseUntil, UpdatedAt: now}
		if err := rows.Scan(&j.ID, &j.Tenant, &j.Kind, &j.Payload, &j.Priority, &j.Attempts,
			&j.MaxAttempts, &j.IdempotencyKey, &j.RunAt, &j.CreatedAt); err != nil {
			return nil, err
		}
		out = append(out, j)
	}
	return out, rows.Err()
}

func (p *Postgres) Extend(ctx context.Context, id, owner string, until time.Time) error {
	res, err := p.db.ExecContext(ctx,
		`UPDATE jobs SET leased_until = $3 WHERE id = $1 AND lease_owner = $2 AND state = 'running'`,
		id, owner, until)
	if err != nil {
		return err
	}
	if n, _ := res.RowsAffected(); n == 0 {
		return ErrLeaseLost
	}
	return nil
}

func (p *Postgres) Get(ctx context.Context, id string) (*job.Job, error) {
	j := &job.Job{}
	err := p.db.QueryRowContext(ctx,
		`SELECT id, tenant, kind, payload, priority, state, attempts, max_attempts, run_at, coalesce(last_error, '')
		 FROM jobs WHERE id = $1`, id).
		Scan(&j.ID, &j.Tenant, &j.Kind, &j.Payload, &j.Priority, &j.State, &j.Attempts, &j.MaxAttempts, &j.RunAt, &j.LastError)
	if errors.Is(err, sql.ErrNoRows) {
		return nil, ErrNotFound
	}
	return j, err
}

func (p *Postgres) Delete(ctx context.Context, id string) error {
	_, err := p.db.ExecContext(ctx, `DELETE FROM jobs WHERE id = $1`, id)
	return err
}

// Remaining Store methods follow the same pattern and are omitted from this
// file for brevity; see migrations/001_jobs.sql for the schema.
