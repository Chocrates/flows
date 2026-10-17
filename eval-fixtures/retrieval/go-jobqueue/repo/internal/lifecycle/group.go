// Package lifecycle starts long-running components and stops them in order.
package lifecycle

import (
	"context"
	"errors"
	"log/slog"
	"sync"
	"time"
)

// Component is something with a blocking Run that returns when ctx ends.
type Component struct {
	Name string
	Run  func(ctx context.Context)
	// Stage controls stop order: lower stages are cancelled first. The HTTP
	// listener is stage 0 so no new work arrives, producers like the
	// scheduler are stage 1, and the worker pool is stage 2 so in-flight
	// jobs finish last.
	Stage int
}

// Group runs components and tears them down stage by stage.
type Group struct {
	Log        *slog.Logger
	components []Component
}

// Add registers c.
func (g *Group) Add(c Component) { g.components = append(g.components, c) }

// ErrDrainTimeout means some stage did not finish within the grace period.
var ErrDrainTimeout = errors.New("lifecycle: drain timed out")

// Run starts everything, waits for stop to close, then cancels each stage
// and waits up to grace for it before moving to the next.
func (g *Group) Run(stop <-chan struct{}, grace time.Duration) error {
	stages := map[int][]Component{}
	maxStage := 0
	for _, c := range g.components {
		stages[c.Stage] = append(stages[c.Stage], c)
		if c.Stage > maxStage {
			maxStage = c.Stage
		}
	}
	cancels := make([]context.CancelFunc, maxStage+1)
	waits := make([]*sync.WaitGroup, maxStage+1)
	for s := 0; s <= maxStage; s++ {
		ctx, cancel := context.WithCancel(context.Background())
		cancels[s], waits[s] = cancel, &sync.WaitGroup{}
		for _, c := range stages[s] {
			waits[s].Add(1)
			go func(c Component) {
				defer waits[s].Done()
				c.Run(ctx)
				g.Log.Info("component stopped", "name", c.Name)
			}(c)
		}
	}

	<-stop
	deadline := time.Now().Add(grace)
	for s := 0; s <= maxStage; s++ {
		cancels[s]()
		done := make(chan struct{})
		go func() { waits[s].Wait(); close(done) }()
		select {
		case <-done:
		case <-time.After(time.Until(deadline)):
			g.Log.Error("stage did not stop in time", "stage", s)
			return ErrDrainTimeout
		}
	}
	return nil
}
