// Package config resolves settings from flags and JQ_* environment variables.
package config

import (
	"flag"
	"fmt"
	"os"
	"strconv"
	"time"
)

// Config holds every tunable of the server.
type Config struct {
	Listen       string
	DSN          string
	Workers      int
	LeaseFor     time.Duration
	JobTimeout   time.Duration
	MaxAttempts  int
	BackoffBase  time.Duration
	BackoffMax   time.Duration
	TenantRPS    float64
	TenantBurst  int
	ReplayWindow time.Duration
	Grace        time.Duration
	LogFormat    string
	LogLevel     string
	SchedulePath string
	NodeID       string
}

func envOr(key, def string) string {
	if v, ok := os.LookupEnv(key); ok {
		return v
	}
	return def
}

func envDur(key string, def time.Duration) time.Duration {
	if d, err := time.ParseDuration(envOr(key, "")); err == nil {
		return d
	}
	return def
}

func envInt(key string, def int) int {
	if n, err := strconv.Atoi(envOr(key, "")); err == nil {
		return n
	}
	return def
}

// Load parses args on top of environment defaults. Flags win.
func Load(args []string) (*Config, error) {
	host, _ := os.Hostname()
	c := &Config{}
	fs := flag.NewFlagSet("server", flag.ContinueOnError)
	fs.StringVar(&c.Listen, "listen", envOr("JQ_LISTEN", ":8080"), "API listen address")
	fs.StringVar(&c.DSN, "dsn", envOr("JQ_DSN", ""), "Postgres DSN (empty = in-memory)")
	fs.IntVar(&c.Workers, "workers", envInt("JQ_WORKERS", 8), "concurrent handlers")
	fs.DurationVar(&c.LeaseFor, "lease", envDur("JQ_LEASE", 30*time.Second), "claim visibility period")
	fs.DurationVar(&c.JobTimeout, "job-timeout", envDur("JQ_JOB_TIMEOUT", 5*time.Minute), "max handler runtime")
	fs.IntVar(&c.MaxAttempts, "max-attempts", envInt("JQ_MAX_ATTEMPTS", 5), "attempts before parking")
	fs.DurationVar(&c.BackoffBase, "backoff-base", envDur("JQ_BACKOFF_BASE", time.Second), "first retry delay")
	fs.DurationVar(&c.BackoffMax, "backoff-max", envDur("JQ_BACKOFF_MAX", 10*time.Minute), "retry delay cap")
	fs.Float64Var(&c.TenantRPS, "tenant-rps", 50, "per-tenant submissions per second")
	fs.IntVar(&c.TenantBurst, "tenant-burst", envInt("JQ_TENANT_BURST", 100), "per-tenant burst")
	fs.DurationVar(&c.ReplayWindow, "replay-window", envDur("JQ_REPLAY_WINDOW", 24*time.Hour), "how long repeated keys return the original")
	fs.DurationVar(&c.Grace, "grace", envDur("JQ_GRACE", 30*time.Second), "shutdown drain period")
	fs.StringVar(&c.LogFormat, "log-format", envOr("JQ_LOG_FORMAT", "json"), "json or text")
	fs.StringVar(&c.LogLevel, "log-level", envOr("JQ_LOG_LEVEL", "info"), "debug|info|warn|error")
	fs.StringVar(&c.SchedulePath, "schedule", envOr("JQ_SCHEDULE", ""), "JSON file of recurring entries")
	fs.StringVar(&c.NodeID, "node-id", envOr("JQ_NODE_ID", host), "lease owner identity")
	if err := fs.Parse(args); err != nil {
		return nil, err
	}
	return c, c.validate()
}

func (c *Config) validate() error {
	switch {
	case c.Workers < 1:
		return fmt.Errorf("workers must be >= 1")
	case c.LeaseFor < 3*time.Second:
		return fmt.Errorf("lease must be at least 3s")
	case c.JobTimeout > c.Grace*10:
		// long handlers are fine, but warn-worthy; not an error
	}
	return nil
}
