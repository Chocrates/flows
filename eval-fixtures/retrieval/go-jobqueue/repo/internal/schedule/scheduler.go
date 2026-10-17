package schedule

import (
	"context"
	"encoding/json"
	"log/slog"
	"sync"
	"time"

	"github.com/acme/jobqueue/internal/job"
)

// Submitter is the subset of the broker the ticker needs.
type Submitter interface {
	Enqueue(ctx context.Context, tenant string, s *job.Submission) (*job.Job, bool, error)
}

// Entry is one recurring job definition.
type Entry struct {
	Name    string          `json:"name"`
	Tenant  string          `json:"tenant"`
	Expr    string          `json:"expr"`
	Kind    string          `json:"kind"`
	Payload json.RawMessage `json:"payload"`

	spec Spec
	next time.Time
}

// Ticker fires entries when their time comes. Each firing carries an
// idempotency key derived from the entry name and slot, so two replicas
// running the ticker do not both submit the same occurrence.
type Ticker struct {
	Sub Submitter
	Log *slog.Logger
	Loc *time.Location

	mu      sync.Mutex
	entries []*Entry
}

// Add parses and registers e.
func (t *Ticker) Add(e Entry, now time.Time) error {
	spec, err := Parse(e.Expr)
	if err != nil {
		return err
	}
	e.spec = spec
	e.next = spec.Next(now.In(t.Loc))
	t.mu.Lock()
	t.entries = append(t.entries, &e)
	t.mu.Unlock()
	return nil
}

// Run checks entries every second until ctx ends. Missed slots during
// downtime are not back-filled; only the next future slot fires.
func (t *Ticker) Run(ctx context.Context) {
	tk := time.NewTicker(time.Second)
	defer tk.Stop()
	for {
		select {
		case <-ctx.Done():
			return
		case now := <-tk.C:
			t.fire(ctx, now.In(t.Loc))
		}
	}
}

func (t *Ticker) fire(ctx context.Context, now time.Time) {
	t.mu.Lock()
	defer t.mu.Unlock()
	for _, e := range t.entries {
		if now.Before(e.next) {
			continue
		}
		slot := e.next
		e.next = e.spec.Next(now)
		sub := &job.Submission{
			Kind:           e.Kind,
			Payload:        e.Payload,
			IdempotencyKey: e.Name + "@" + slot.UTC().Format(time.RFC3339),
		}
		if _, dup, err := t.Sub.Enqueue(ctx, e.Tenant, sub); err != nil {
			t.Log.Error("scheduled submit failed", "entry", e.Name, "err", err)
		} else if !dup {
			t.Log.Info("scheduled job submitted", "entry", e.Name, "slot", slot)
		}
	}
}
