// Command server runs the HTTP API, the scheduler and the worker pool.
package main

import (
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"net/http"
	"os"
	"os/signal"
	"syscall"
	"time"

	"github.com/acme/jobqueue/internal/api"
	"github.com/acme/jobqueue/internal/config"
	"github.com/acme/jobqueue/internal/deadletter"
	"github.com/acme/jobqueue/internal/job"
	"github.com/acme/jobqueue/internal/lifecycle"
	"github.com/acme/jobqueue/internal/logging"
	"github.com/acme/jobqueue/internal/metrics"
	"github.com/acme/jobqueue/internal/queue"
	"github.com/acme/jobqueue/internal/ratelimit"
	"github.com/acme/jobqueue/internal/retry"
	"github.com/acme/jobqueue/internal/schedule"
	"github.com/acme/jobqueue/internal/store"
	"github.com/acme/jobqueue/internal/worker"
)

func main() {
	if err := run(os.Args[1:]); err != nil {
		fmt.Fprintln(os.Stderr, "server:", err)
		os.Exit(1)
	}
}

func run(args []string) error {
	cfg, err := config.Load(args)
	if err != nil {
		return err
	}
	log := logging.New(os.Stderr, cfg.LogFormat, cfg.LogLevel)
	if cfg.DSN != "" {
		return errors.New("postgres storage requires building with -tags pg")
	}
	st := store.NewMemory()
	reg := metrics.NewRegistry()

	broker := queue.NewBroker(st, queue.Options{
		Owner: cfg.NodeID, LeaseFor: cfg.LeaseFor, BufferSize: cfg.Workers * 2,
		MaxAttempts: cfg.MaxAttempts, ReplayWindow: cfg.ReplayWindow,
	})
	parker := &deadletter.Parker{St: st, Now: time.Now}
	handlers := worker.NewRegistry()
	registerBuiltins(handlers)

	pool := &worker.Pool{
		Size: cfg.Workers, Broker: broker, Handlers: handlers, Parker: parker,
		Policy:   retry.Policy{MaxAttempts: cfg.MaxAttempts, Backoff: retry.NewBackoff(cfg.BackoffBase, cfg.BackoffMax)},
		LeaseFor: cfg.LeaseFor, Timeout: cfg.JobTimeout, Log: log, Metrics: reg,
	}
	reaper := &queue.Reaper{St: st, Every: cfg.LeaseFor / 2, Batch: 100, Log: log,
		OnReturn: func(j *job.Job) { reg.Inc("leases_expired_total", "kind", j.Kind) }}
	ticker := &schedule.Ticker{Sub: broker, Log: log, Loc: time.UTC}
	if err := loadSchedule(ticker, cfg.SchedulePath); err != nil {
		return err
	}

	srv := &http.Server{
		Addr: cfg.Listen,
		Handler: (&api.Server{Broker: broker, Parker: parker, Limiter: ratelimit.New(cfg.TenantRPS, cfg.TenantBurst),
			Metrics: reg, Log: log}).Routes(),
		ReadHeaderTimeout: 5 * time.Second,
	}

	var g lifecycle.Group
	g.Log = log
	g.Add(lifecycle.Component{Name: "http", Stage: 0, Run: func(ctx context.Context) {
		go func() {
			<-ctx.Done()
			sctx, cancel := context.WithTimeout(context.Background(), 10*time.Second)
			defer cancel()
			_ = srv.Shutdown(sctx)
		}()
		if err := srv.ListenAndServe(); err != nil && !errors.Is(err, http.ErrServerClosed) {
			log.Error("listen failed", "err", err)
		}
	}})
	g.Add(lifecycle.Component{Name: "schedule", Stage: 1, Run: ticker.Run})
	g.Add(lifecycle.Component{Name: "reaper", Stage: 1, Run: reaper.Run})
	g.Add(lifecycle.Component{Name: "pool", Stage: 2, Run: func(ctx context.Context) {
		pool.Run(ctx)
		for _, j := range broker.Unclaimed() {
			_ = queue.Release(context.Background(), st, j, broker.Owner())
		}
	}})

	stop := make(chan struct{})
	sig := make(chan os.Signal, 2)
	signal.Notify(sig, syscall.SIGINT, syscall.SIGTERM)
	go func() {
		s := <-sig
		log.Info("signal received, draining", "signal", s.String(), "grace", cfg.Grace)
		close(stop)
		<-sig
		log.Warn("second signal, exiting immediately")
		os.Exit(2)
	}()

	log.Info("server starting", "listen", cfg.Listen, "workers", cfg.Workers, "node", cfg.NodeID)
	return g.Run(stop, cfg.Grace)
}

func loadSchedule(t *schedule.Ticker, path string) error {
	if path == "" {
		return nil
	}
	raw, err := os.ReadFile(path)
	if err != nil {
		return err
	}
	var entries []schedule.Entry
	if err := json.Unmarshal(raw, &entries); err != nil {
		return fmt.Errorf("parse %s: %w", path, err)
	}
	now := time.Now()
	for _, e := range entries {
		if err := t.Add(e, now); err != nil {
			return fmt.Errorf("entry %s: %w", e.Name, err)
		}
	}
	return nil
}

func registerBuiltins(r *worker.Registry) {
	r.Register("noop", func(context.Context, *job.Job) error { return nil })
	r.Register("sleep", func(ctx context.Context, j *job.Job) error {
		var p struct{ Seconds int }
		if err := worker.Decode(j, &p); err != nil {
			return err
		}
		select {
		case <-time.After(time.Duration(p.Seconds) * time.Second):
			return nil
		case <-ctx.Done():
			return ctx.Err()
		}
	})
}
