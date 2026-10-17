package queue

import (
	"context"
	"io"
	"log/slog"
	"testing"
	"time"

	"github.com/acme/jobqueue/internal/job"
	"github.com/acme/jobqueue/internal/store"
)

func TestReaperReturnsAbandonedJobs(t *testing.T) {
	ctx := context.Background()
	st := store.NewMemory()
	now := time.Unix(1_700_000_000, 0)
	_ = st.Insert(ctx, &job.Job{ID: "a", Kind: "email.send", State: job.StatePending, RunAt: now, MaxAttempts: 3})

	claimed, err := st.Claim(ctx, "node-1", 1, now, now.Add(10*time.Second))
	if err != nil || len(claimed) != 1 {
		t.Fatalf("claim: %v %d", err, len(claimed))
	}

	r := &Reaper{St: st, Batch: 10, Log: slog.New(slog.NewTextHandler(io.Discard, nil))}
	r.sweep(ctx, now.Add(5*time.Second))
	if got, _ := st.Get(ctx, "a"); got.State != job.StateRunning {
		t.Fatalf("reaped too early: %s", got.State)
	}

	r.sweep(ctx, now.Add(11*time.Second))
	got, _ := st.Get(ctx, "a")
	if got.State != job.StatePending || got.LeaseOwner != "" {
		t.Fatalf("expected pending without owner, got %s/%q", got.State, got.LeaseOwner)
	}
}

func TestExtendRejectsForeignOwner(t *testing.T) {
	ctx := context.Background()
	st := store.NewMemory()
	now := time.Now()
	_ = st.Insert(ctx, &job.Job{ID: "b", State: job.StatePending, RunAt: now})
	_, _ = st.Claim(ctx, "node-1", 1, now, now.Add(time.Second))
	if err := st.Extend(ctx, "b", "node-2", now.Add(time.Minute)); err != store.ErrLeaseLost {
		t.Fatalf("want ErrLeaseLost, got %v", err)
	}
}
