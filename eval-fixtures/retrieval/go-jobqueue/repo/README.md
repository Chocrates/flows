# jobqueue

A small background job service. Producers submit work over HTTP, workers
pick it up, and failed attempts are retried with backoff before being parked
for manual inspection.

## Components

- `cmd/server` — the HTTP API plus the worker pool and scheduler, in one process.
- `cmd/qctl` — admin CLI: inspect, requeue and purge jobs.
- `internal/store` — storage interface with an in-memory implementation and a
  Postgres implementation (`SELECT ... FOR UPDATE SKIP LOCKED`).

## Running

```sh
JQ_LISTEN=:8080 JQ_WORKERS=16 go run ./cmd/server
go run ./cmd/qctl -addr http://localhost:8080 stats
```

## Configuration

Every flag has an environment variable counterpart (`-workers` / `JQ_WORKERS`).
Flags win over the environment.

| Variable | Default | Meaning |
|----------|---------|---------|
| `JQ_LISTEN` | `:8080` | API listen address |
| `JQ_WORKERS` | `8` | concurrent handlers |
| `JQ_LEASE` | `30s` | how long a claimed job is hidden from others |
| `JQ_MAX_ATTEMPTS` | `5` | attempts before parking a job |
| `JQ_TENANT_RPS` | `50` | per-tenant submit rate |
| `JQ_DSN` | (empty) | Postgres DSN; empty means in-memory |
