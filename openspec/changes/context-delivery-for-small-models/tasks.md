## 1. Tier 1 — search & precise reads

- [ ] 1.1 Add the `ignore` + `regex`/`grep` crates to `flows-tools`; add `flows-tools/src/search.rs` with a repo-scoped `search_code(repo_root, pattern, scope, cap)` using the file-tools path guard.
- [ ] 1.2 Return bounded `path:line: match` results with a truncation indicator; explicit "no matches" (not an error); refuse a scope resolving outside the repo root. Unit tests for match, no-match, escape, and cap.
- [ ] 1.3 Register a `search_code` tool in `agent.rs` `build_tool_defs` + `dispatch_tool` (available whenever `files` is on; unaffected by `read_only`).
- [ ] 1.4 Extend `files::read_file` with optional `start_line`/`end_line` and a default byte/line cap (build on `read_file_limited`); label the returned range and whether more exists; error on empty/inverted ranges. Unit tests for range, cap, invalid range.
- [ ] 1.5 Add `start_line`/`end_line` to the `read_file` tool schema + dispatch in `agent.rs`.
- [ ] 1.6 Rewrite `repo_context::build_repo_tree` to be gitignore-aware (`ignore`), depth-bounded, and size-capped; summarize past the cap and point at `search_code`/`list_files` instead of a truncated flat list. Update its tests.
- [ ] 1.7 Update affected prompt templates to mention `search_code` and range reads; gate/verify (`cargo fmt`, `clippy -D warnings`, `cargo test`).

## 2. Tier 2 — repo map, budgeting, scoping

- [ ] 2.1 Add tree-sitter + grammar crates (rust, python, js/ts, go) to `flows-runner`; add `flows-runner/src/repo_map.rs` extracting symbol signatures via tree-sitter into `(file, [symbol signatures])`.
- [ ] 2.2 Rank files by idea↔(path + symbol names) token overlap plus a git-recency boost; cap the map to a token budget; mark partial when clipped. Unit tests for ranking + budget.
- [ ] 2.3 Inject **both** a slim `repo_tree` and the ranked `repo_map` context variables in `agent.rs`; update prompt templates to consume the map (tree for layout, map for symbols).
- [ ] 2.4 Add context-budgeting to the agent tool loop: estimate request tokens (tiktoken-rs) pre-send; when > `budget_fraction * context_window` (or `unknown_window_tokens` when unknown), truncate oldest tool-result bodies to a `[truncated N tokens]` stub, preserving system prompt + task + most-recent turns.
- [ ] 2.5 Add config keys `budget_fraction` (default 0.6) and `unknown_window_tokens` (default 8000); wire through `flows-core` config. Tests: trim triggers past threshold; head + recent turns preserved; unknown window uses the default.
- [ ] 2.6 Fix per-phase scoping: expose prior artifacts exactly once (remove the full-map + per-key double-injection), injecting only what the phase template references. Update/verify prompt rendering tests.
- [ ] 2.7 Gate/verify (fmt, clippy -D warnings, tests).

## 3. Tier 3 — semantic retrieval (opt-in)

- [ ] 3.1 Add an embeddings request path in `flows-llm` (`/v1/embeddings`, OpenAI-compatible) routed through the SSRF-guarded HTTP layer; no key required for a local endpoint.
- [ ] 3.2 Add `flows-runner` (or a new `flows-index`) module: chunk by tree-sitter symbol (reuse 2.1), splitting any symbol over ~350 tokens; embed; persist an on-disk vector index under `~/.flows` keyed by a repo hash; incremental re-embed by per-chunk content hash.
- [ ] 3.3 Cosine retrieval for the phase idea, **budget-bounded**: fill highest-scoring chunks up to `retrieval.budget_fraction` (0.25) of the phase context budget, capped by `retrieval.max_chunks` (8); inject into the phase context. Tests for ranked selection, budget cap, and incremental update (unchanged repo reuses index).
- [ ] 3.4 Add config `retrieval.enabled` (default false), `retrieval.budget_fraction` (0.25), `retrieval.max_chunks` (8), and endpoint; when disabled, no embedding requests and prompt assembly unchanged. Test the disabled-by-default path.
- [ ] 3.5 Verify embeddings honor the network guard (blocked endpoint refused, same as `http_get`).
- [ ] 3.6 Gate/verify (fmt, clippy -D warnings, tests).

## 4. Gating (do first — every feature lands behind a flag)

- [ ] 4.1 Add a `context` config block to `flows-core` with independent flags `search_tool`, `range_reads`, `slim_tree`, `repo_map`, `budgeting`, and `retrieval.enabled` — all defaulting to the pre-change baseline (off). Tests: all-off round-trips baseline.
- [ ] 4.2 Add a per-run override (a `run --context k=v,…` flag or run-level config) that sets `context.*` for one run without touching global/project config. Test the override is scoped to the run.
- [ ] 4.3 Gate each feature from Tiers 1–3 on its flag at the assembly/tool-registration site (search_code tool, range args, tree vs slim tree, repo_map injection, loop budgeting, retrieval). Test: enabling exactly one flag activates only that feature.

## 5. Evaluation harness

- [ ] 5.1 Define a labeled fixtures format: `(repo snapshot, task, relevant files/symbols, verifier command)`; add a small starter corpus.
- [ ] 5.2 Deterministic retrieval eval (no model call): for search / repo_map / retrieval, compute recall@k and MRR of the feature's ranking against the labeled targets. Tests over a fixture.
- [ ] 5.3 `flows eval` subcommand (or `flows-eval` crate): run `(fixture × config × trial)` headless, capture success (verifier), tokens, peak context size, cost, tool-rounds, loop-backs from the run records.
- [ ] 5.4 Report: per-config table (baseline vs each flag, ablated + cumulative) with mean ± spread across trials; run at temperature 0/low for comparability.
- [ ] 5.5 Verify: run the harness on the starter corpus for baseline vs +search vs +search+map vs +…+budgeting vs +…+retrieval; capture the table.

## 6. Docs & wrap-up

- [ ] 6.1 Update README (agent context section: `search_code`, range reads, repo map, retrieval config, the `context.*` flags + `flows eval`) and DESIGN.
- [ ] 6.2 Update the `security-model` spec only if a new surface warrants it (search/read stay repo-scoped; embeddings SSRF-guarded — confirm no new invariant needed).
- [ ] 6.3 Final full gate green; `openspec validate context-delivery-for-small-models`.
