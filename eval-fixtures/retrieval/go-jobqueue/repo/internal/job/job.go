// Package job defines the unit of work moved through the system.
package job

import (
	"encoding/json"
	"time"
)

// State is the lifecycle position of a job.
type State string

const (
	StatePending   State = "pending"
	StateRunning   State = "running"
	StateSucceeded State = "succeeded"
	StateFailed    State = "failed"
	StateDead      State = "dead"
)

// Priority orders jobs within a tenant; higher runs first.
type Priority int

const (
	PriorityLow    Priority = 0
	PriorityNormal Priority = 5
	PriorityHigh   Priority = 10
)

// Job is a single unit of background work.
type Job struct {
	ID             string          `json:"id"`
	Tenant         string          `json:"tenant"`
	Kind           string          `json:"kind"`
	Payload        json.RawMessage `json:"payload"`
	Priority       Priority        `json:"priority"`
	State          State           `json:"state"`
	Attempts       int             `json:"attempts"`
	MaxAttempts    int             `json:"max_attempts"`
	IdempotencyKey string          `json:"idempotency_key,omitempty"`
	RunAt          time.Time       `json:"run_at"`
	LeasedUntil    time.Time       `json:"leased_until,omitempty"`
	LeaseOwner     string          `json:"lease_owner,omitempty"`
	LastError      string          `json:"last_error,omitempty"`
	CreatedAt      time.Time       `json:"created_at"`
	UpdatedAt      time.Time       `json:"updated_at"`
}

// Runnable reports whether the job may be picked up at now.
func (j *Job) Runnable(now time.Time) bool {
	if j.State != StatePending {
		return false
	}
	return !now.Before(j.RunAt)
}

// Terminal reports whether the job will never run again.
func (j *Job) Terminal() bool {
	return j.State == StateSucceeded || j.State == StateDead
}

// Clone returns a deep copy so callers cannot mutate stored state.
func (j *Job) Clone() *Job {
	c := *j
	if j.Payload != nil {
		c.Payload = append(json.RawMessage(nil), j.Payload...)
	}
	return &c
}
