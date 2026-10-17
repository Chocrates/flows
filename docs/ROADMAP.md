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
- Sandbox hardening: unix sockets denied via seccomp (closes a session-bus escape); per-command memory/process caps via systemd scopes.
- Symbol-aware retrieval chunks and window-scaled injection (rag MRR 0.34 → 1.00 on the fixture corpus).
- `flows run --context map+rag` per-run override; global and project `context:` blocks merge key-by-key.

### Unreleased

- Conditional phases: `when:` run-if expressions over prior artifacts; skipped phases are recorded, shown as `⊘`, and count as done for DAG dependents.
- `web_search` backends `brave` (Brave Search API) and `serper` (Google via Serper), keyed by config or `BRAVE_API_KEY` / `SERPER_API_KEY`.
- `tools.bash.offline: true` runs a phase's shell commands without network (loopback only).
- Sandbox hides credential stores under `~` (ssh, gpg, cloud CLIs, forge tokens, registry creds, browsers, flows' own API-key config); `tools.bash.expose_credentials` opts out per phase.
- Repo map breaks relevance ties by git recency (uncommitted files, then recent commits), read inside the sandbox; mtime fallback outside git.
- Retrieval index re-embeds only new/changed chunks (content-addressed, spare pool for branch switches); `~/.flows/index/` capped at 1 GiB with orphan + LRU eviction. `retrieval_reindex_threshold` deprecated.
- Path-free retrieval eval corpus (`eval-fixtures/retrieval/`: flows + synthetic Python / TypeScript / Go, 68 queries) via `flows eval retrieval --corpus`.

## Next

### Context delivery follow-ups

- **Improve path-free retrieval** — on the path-free corpus the repo map scores MRR 0.34 and embeddings 0.65 (68 queries). Candidates: hybrid map+embedding ranking, a stronger default embedding model, real-repo corpora.

### Sandbox & tools

- **macOS sandbox IPC parity** — the Seatbelt profile allows `network*` (including unix sockets) and all `mach-lookup`, so a sandboxed command may reach launchd/host services. Linux denies unix sockets via seccomp; macOS needs `(allow network* (remote ip))`-style rules and a mach-lookup allowlist, verified on a Mac.

### Pipelines

- **Pipeline composition** — `include` another pipeline as a phase.
- **Template registry** — share/import community templates.

### Later / maybe

- **Web UI** — `flows serve` for remote management.

## Tech debt

- `DESIGN.md`'s TUI section still describes the Textual implementation (`RichLog`, `_follow`, `CopyableRichLog`, compositor offsets); the Rust `OutputPane` (`crates/flows-tui/src/output.rs`) works differently and the section needs a rewrite.
