## 1. Tier 1 — search & precise reads

- [x] 1.1 Add the `ignore` + `regex`/`grep` crates to `flows-tools`; add `flows-tools/src/search.rs` with a repo-scoped `search_code(repo_root, pattern, scope, cap)` using the file-tools path guard. _(landed as `flows-tools/src/code_search.rs` — `search.rs` was already taken by web search.)_
- [x] 1.2 Return bounded `path:line: match` results with a truncation indicator; explicit "no matches" (not an error); refuse a scope resolving outside the repo root. Unit tests for match, no-match, escape, and cap.
- [x] 1.3 Register a `search_code` tool in `agent.rs` `build_tool_defs` + `dispatch_tool` (available whenever `files` is on; unaffected by `read_only`; gated on `context.search_tool`).
- [x] 1.4 Extend `files::read_file` with optional `start_line`/`end_line` and a default byte/line cap (build on `read_file_limited`); label the returned range and whether more exists; error on empty/inverted ranges. Unit tests for range, cap, invalid range. _(added `read_file_range`.)_
- [x] 1.5 Add `start_line`/`end_line` to the `read_file` tool schema + dispatch in `agent.rs`.
- [x] 1.6 Rewrite `repo_context::build_repo_tree` to be gitignore-aware (`ignore`), depth-bounded, and size-capped; summarize past the cap and point at `search_code`/`list_files` instead of a truncated flat list. Update its tests. _(added `build_slim_tree` alongside the baseline tree so the flag can toggle cleanly.)_
- [x] 1.7 Update affected prompt templates to mention `search_code` and range reads; gate/verify (`cargo fmt`, `clippy -D warnings`, `cargo test`). _(guidance injected flag-aware from `agent.rs` instead of static markdown, so it stays in sync with tool availability.)_

## 2. Tier 2 — repo map, budgeting, scoping

- [ ] 2.1 Add tree-sitter + grammar crates (rust, python, js/ts, go) to `flows-runner`; add `flows-runner/src/repo_map.rs` extracting symbol signatures via tree-sitter into `(file, [symbol signatures])`.
- [ ] 2.2 Rank files by idea↔(path + symbol names) token overlap plus a git-recency boost; cap the map to a token budget; mark partial when clipped. Unit tests for ranking + budget.
- [ ] 2.3 Inject **both** a slim `repo_tree` and the ranked `repo_map` context variables in `agent.rs`; update prompt templates to consume the map (tree for layout, map for symbols).
- [ ] 2.4 Add context-budgeting to the agent tool loop: estimate request tokens (tiktoken-rs) pre-send; when > `budget_fraction * context_window` (or `unknown_window_tokens` when unknown), truncate oldest tool-result bodies to a `[truncated N tokens]` stub, preserving system prompt + task + most-recent turns.
- [ ] 2.5 Add config keys `budget_fraction` (default 0.6) and `unknown_window_tokens` (default 8000); wire through `flows-core` config. Tests: trim triggers past threshold; head + recent turns preserved; unknown window uses the default.
- [ ] 2.6 Fix per-phase scoping: expose prior artifacts exactly once (remove the full-map + per-key double-injection), injecting only what the phase template references. Update/verify prompt rendering tests.
- [ ] 2.7 Gate/verify (fmt, clippy -D warnings, tests).

## 3. Tier 3 — semantic retrieval (opt-in)

> **Design change:** embeddings run **locally, in-process** via `fastembed` (ONNX),
> not against a hosted `/v1/embeddings` server. The original server design assumed
> the user's inference endpoint also serves an embedding model — it does not, so a
> server path would leave retrieval inert. Local embedding needs zero hosting/zero
> config and works offline after a one-time model download. See `design.md` (D8).

- [x] 3.1 ~~Server embeddings path in `flows-llm`~~ → **local** embeddings: `fastembed`/ONNX in `flows-runner`, model downloaded once to the fastembed cache. No network at inference; no SSRF surface (no model-supplied URLs). _(server `flows-llm/embeddings.rs` was built then removed in favor of local.)_
- [~] 3.2 `flows-runner/src/retrieval.rs`: chunk repo source into **line windows** (~50 lines), gitignore + deny-list aware, capped at 600 chunks; embed in-process. _(First cut: line-window chunks, **not** tree-sitter symbols (needs 2.1); **in-memory** index, no on-disk cache / incremental re-embed yet — follow-ups.)_
- [~] 3.3 Cosine retrieval for the phase idea, capped by `retrieval_max_chunks` (default 8); injected into the phase context as a labeled block. Unit tests for cosine, ranked selection, chunking; ignored E2E test downloads the model and ranks a relevant chunk first. _(No `budget_fraction` / no incremental-reuse test — index is rebuilt per run for now.)_
- [~] 3.4 Config `retrieval` (default false), `retrieval_model` (local model name; empty → all-MiniLM-L6-v2), `retrieval_max_chunks` (default 8) in the `context` block (global + project). Disabled-by-default path tested. _(No `budget_fraction`; no endpoint — local.)_
- [x] 3.5 N/A under the local design — embeddings make no outbound request at inference (model fetch goes through `hf-hub` to its cache). The `http_get` SSRF guard still covers all model-supplied URLs; no new network surface for the agent.
- [x] 3.6 Gate/verify (fmt, clippy -D warnings, tests) — green; verified end-to-end on the reasoner fixtures (baseline vs oracle vs rag).

## 4. Gating (do first — every feature lands behind a flag)

- [x] 4.1 Add a `context` config block to `flows-core` with independent flags `search_tool`, `range_reads`, `slim_tree`, `repo_map`, `budgeting`, and `retrieval.enabled` — all defaulting to the pre-change baseline (off). Tests: all-off round-trips baseline.
- [x] 4.2 Add a per-run override (a `run --context k=v,…` flag or run-level config) that sets `context.*` for one run without touching global/project config. Test the override is scoped to the run.
- [~] 4.3 Gate each feature from Tiers 1–3 on its flag at the assembly/tool-registration site (search_code tool, range args, tree vs slim tree, repo_map injection, loop budgeting, retrieval). Test: enabling exactly one flag activates only that feature. _(Done + tested: `search_tool`/`range_reads`/`slim_tree`, `retrieval` (rag), and the eval-only `oracle` push; `repo_map`/`budgeting` pending Tier 2.)_

## 5. Evaluation harness

- [ ] 5.1 Define a labeled fixtures format: `(repo snapshot, task, relevant files/symbols, verifier command)`; add a small starter corpus.
- [ ] 5.2 Deterministic retrieval eval (no model call): for search / repo_map / retrieval, compute recall@k and MRR of the feature's ranking against the labeled targets. Tests over a fixture.
- [ ] 5.3 `flows eval` subcommand (or `flows-eval` crate): run `(fixture × config × trial)` headless, capture success (verifier), tokens, peak context size, cost, tool-rounds, loop-backs from the run records.
- [ ] 5.4 Report: per-config table (baseline vs each flag, ablated + cumulative) with mean ± spread across trials; run at temperature 0/low for comparability.
- [ ] 5.5 Verify: run the harness on the starter corpus for baseline vs +search vs +search+map vs +…+budgeting vs +…+retrieval; capture the table.

## 7. No-tool models & push context (this iteration)

Small **reasoning models often can't function-call** (e.g. `deepseek-r1-70b`), so
the Tier 1 pull tools (search/range reads) are inert for them. Added a **push**
path and made prompts honest about tool availability.

- [x] 7.1 Gate tool sections on `tools_enabled` (`llm.supports_tools != Some(false)`): a no-tool model gets no tool schemas, no `## Tools available` section (plan/implement/gather/explain), and a tool-neutral repo tree (footers no longer name `search_code`/`list_files`). Tests over the real templates.
- [x] 7.2 Add an **oracle push** (eval stopgap): the eval sets `FLOWS_ORACLE_CONTEXT` from a fixture's labeled `relevant` files; the runner injects it as up-front reference code. Config flag `oracle` (token, kept out of `all`). Verified: baseline hallucinates → oracle is grounded.
- [x] 7.3 Add semantic **retrieval push** injected into the same `pushed_context` variable — works for no-tool models (Tier 3, local embeddings). Verified: rag reaches ~oracle grounding at ~45% of the tokens.
- [x] 7.4 Add an **`explain`** template + prompt (`feature/explain.md`) that asks for a prose-answer artifact (`content.answer`) instead of a build plan, so reasoner "explain" fixtures produce the shape their rubric grades. Point reasoner fixtures at it.

## 8. Eval-harness enhancements (this iteration)

- [x] 8.1 Per-fixture context config axis: a fixture declares `configs: [baseline, oracle, rag]`; absent → the run's `--configs`. Reasoner fixtures A/B automatically.
- [x] 8.2 `--models <suite>` (e.g. `coder`/`reasoner`) expands to every profile tagged that suite; `model:suite` still overrides. 0-run guard errors clearly when suite-restricted models don't cover the selected fixtures.
- [x] 8.3 Config carries context settings on **global** `GlobalConfig` too (shared `parse_context_block`); eval `pin_config` preserves `retrieval_model`/`retrieval_max_chunks` while overlaying token flags.

## 6. Docs & wrap-up

- [ ] 6.1 Update README (agent context section: `search_code`, range reads, repo map, retrieval config, the `context.*` flags + `flows eval`) and DESIGN.
- [ ] 6.2 Update the `security-model` spec only if a new surface warrants it (search/read stay repo-scoped; embeddings SSRF-guarded — confirm no new invariant needed).
- [ ] 6.3 Final full gate green; `openspec validate context-delivery-for-small-models`.
