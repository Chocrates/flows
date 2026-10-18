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
- Pipeline composition: `include: <template>` splices a template's phases (`as:` alias, `with:` vars, include-level `when`/`depends_on`; prompts resolve under the included template).
- Retrieval ranks chunks by embedding + BM25 reciprocal-rank fusion (path-free corpus MRR 0.65 → 0.70).
- Template registry: `flows templates install <git>[//path][@ref]` pins to a commit, reviews before writing, and confines untrusted templates (scripts sandboxed, no credential exposure).
- Shared per-model vector cache (`~/.flows/vectors/`): worktrees, clones and eval temp dirs reuse embeddings.
- Agent stall guards: replies cut off at `max_tokens` are nudged, then fail; rounds that bring nothing new nudge, then force a tool-less reply (read-only) or abort; an unreachable search backend drops `web_search` for the phase.
- Agent stall guards: `lookup` / `web_search` refuse queries under 3 letters or digits (a model walking the alphabet looked productive forever); refused absolute paths that exist in the repo get a repo-relative hint; policy refusals (allowlist, sandbox scope) count as stalls, and refused `ls`/`cat`/`grep` name the file tool to use.
- Artifact JSON with unbalanced brackets (a dropped `}` or an array left open) is repaired before parsing.
- A forced reply that comes back as a tool call restarts from the phase's findings, with no tool history and no tools declared.
- Phases without a prompt file (custom templates) now see earlier phases' artifacts, the repo map and retrieved code; the repo tree marks directories as `name/`.
- `feature` review and `software` verify loop back to implement at most 3 times (was 30), matching the 3 fix cycles their prompts promise.
- Fix: `loop_condition` never fired in installed builds — minijinja 2.24 renders booleans as `True` and the condition was compared as text; it is now evaluated as an expression. `make install` builds `--locked`.
- `templates sync` records what it wrote (`~/.flows/synced.json`), updates unedited copies and keeps edited ones; `doctor` flags copies that hide a newer builtin.
- Agent phases reaching their round cap reply with their artifact (tools off for the last 3 rounds) instead of failing; prompts get `changes` (new files + uncommitted diff), shown to the feature review and on implement loop-backs; feature review may `rm` its scratch files.
- `thinking: false` turns reasoning off on local OpenAI-compatible servers (`chat_template_kwargs.enable_thinking`).
- Repo map ranks files by BM25 over path + full source (path-free corpus MRR 0.34 → 0.72; fixtures 0.47 → 1.00).
- `retrieval_model: gemma` (EmbeddingGemma-300M, path-free MRR 0.83 vs 0.71 default) and `jina-code`; models embed with their trained prefixes, in batches of 8 (gemma 3.6 → 1.2 GB peak); unknown names warn.
- `flows doctor` probes local model servers (`GET /models`) and warns when one is down or doesn't list its configured model.

## Next

### Context delivery follow-ups

- **Improve path-free retrieval** — on the path-free corpus the repo map scores MRR 0.72, retrieval 0.71 with the default model and 0.83 with `gemma` (68 queries). Open: whether to make a larger model the default; real-repo corpora.

### Sandbox & tools

- **macOS sandbox IPC parity** — the Seatbelt profile allows `network*` (including unix sockets) and all `mach-lookup`, so a sandboxed command may reach launchd/host services. Linux denies unix sockets via seccomp; macOS needs `(allow network* (remote ip))`-style rules and a mach-lookup allowlist, verified on a Mac.

### Later / maybe

- **Web UI** — `flows serve` for remote management.

## Tech debt

None tracked.
