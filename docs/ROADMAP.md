# flows — Roadmap

Status as of 2026-10. Shipped work is tracked in `openspec/specs/` and
`openspec/changes/archive/`; this file is the forward-looking list.

## Shipped

### v0.2 — Rust port, UX & reliability ✓

- Full port from Python to a six-crate Rust workspace, single `flows` binary (`docs/RUST_PORT.md`).
- Streaming output, `doctor`, `plan` (dry run + cost estimate), `runs` / `usage` / `stop` / `delete` / `clean`, `--directory/-C`, shell completions.
- TUI-first runs that always execute in a background worker, so you can quit and `attach` later; home/launcher screen.
- Cost guard (`max_cost_usd`), per-phase completion-token cap, transient-failure retry with backoff (`model-io`).
- DAG execution via `depends_on`.
- Shell sandbox: bubblewrap (Linux) / Seatbelt (macOS), fails closed; replaces the planned Docker sandbox.
- `http_get` SSRF guard with redirect revalidation.
- Named model profiles + per-phase model routing (`model-profiles`).
- Web search: DuckDuckGo, doc lookup, optional self-hosted SearXNG (`flows searxng`).

### v0.3 — Small-model context delivery + evaluation ✓

- `search_code` tool and line-ranged `read_file` (`code-search`).
- Slim gitignore-aware tree, tree-sitter ranked repo map, in-loop token budgeting (`repo-context`).
- Opt-in local embedding retrieval (fastembed), persisted index with churn-based rebuild (`semantic-retrieval`).
- Push context for models without tool calling.
- `flows eval`: fixture × model × context-config matrix, results store, report, deterministic retrieval recall/MRR (`eval-harness`, `context-eval`).
- Agent stall guards: no-op write streaks, empty-response nudges (fail after 3), max-round limit.
- Symbol-aware retrieval chunks and window-scaled injection (rag MRR 0.34 → 1.00 on the fixture corpus).
- `flows run --context map+rag` per-run override; global and project `context:` blocks merge key-by-key.

## Next

### Context delivery follow-ups

- **Git-recency ranking** — rank the repo map with git history instead of file mtime.
- **Broader retrieval eval corpus** — all six fixtures come from this repo and most tasks name their target file path, which inflates path-matching rankers. Add path-free tasks and fixtures from other repos/languages.
- **Retrieval index hygiene** — content-addressed vector cache so switching branches doesn't trigger a rebuild; eviction cap for `~/.flows/index/`.

### Sandbox & tools

- **macOS sandbox IPC parity** — the Seatbelt profile allows `network*` (including unix sockets) and all `mach-lookup`, so a sandboxed command may reach launchd/host services. Linux denies unix sockets via seccomp; macOS needs `(allow network* (remote ip))`-style rules and a mach-lookup allowlist, verified on a Mac.
- **Home-directory secrets are readable** — the sandbox binds `/` read-only, so `~/.ssh`, `~/.aws`, `~/.config/gh`, etc. can be read (and with shared network, exfiltrated). Consider masking known credential dirs with tmpfs.
- **Resource limits** in the sandbox (memory / pids / CPU via rlimits or cgroups).
- **Optional network isolation** for shell commands (opt-in `--unshare-net`).
- **Brave / Serper** keyed web-search backends.

### Pipelines

- **Pipeline composition** — `include` another pipeline as a phase.
- **Conditional phases** — skip a phase based on prior artifact content.
- **Template registry** — share/import community templates.

### Later / maybe

- **Web UI** — `flows serve` for remote management.

## Tech debt

- `DESIGN.md` still has Python-era details in places (e.g. `_run_linear` / `asyncio.wait` in "Runner architecture", `flows/prompts/` paths in "Prompt loading").
