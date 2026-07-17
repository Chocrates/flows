## 1. Tier 1 — search & precise reads

- [x] 1.1 Add the `ignore` + `regex`/`grep` crates to `flows-tools`; add `flows-tools/src/search.rs` with a repo-scoped `search_code(repo_root, pattern, scope, cap)` using the file-tools path guard. _(landed as `flows-tools/src/code_search.rs` — `search.rs` was already taken by web search.)_
- [x] 1.2 Return bounded `path:line: match` results with a truncation indicator; explicit "no matches" (not an error); refuse a scope resolving outside the repo root. Unit tests for match, no-match, escape, and cap.
- [x] 1.3 Register a `search_code` tool in `agent.rs` `build_tool_defs` + `dispatch_tool` (available whenever `files` is on; unaffected by `read_only`; gated on `context.search_tool`).
- [x] 1.4 Extend `files::read_file` with optional `start_line`/`end_line` and a default byte/line cap (build on `read_file_limited`); label the returned range and whether more exists; error on empty/inverted ranges. Unit tests for range, cap, invalid range. _(added `read_file_range`.)_
- [x] 1.5 Add `start_line`/`end_line` to the `read_file` tool schema + dispatch in `agent.rs`.
- [x] 1.6 Rewrite `repo_context::build_repo_tree` to be gitignore-aware (`ignore`), depth-bounded, and size-capped; summarize past the cap and point at `search_code`/`list_files` instead of a truncated flat list. Update its tests. _(added `build_slim_tree` alongside the baseline tree so the flag can toggle cleanly.)_
- [x] 1.7 Update affected prompt templates to mention `search_code` and range reads; gate/verify (`cargo fmt`, `clippy -D warnings`, `cargo test`). _(guidance injected flag-aware from `agent.rs` instead of static markdown, so it stays in sync with tool availability.)_

## 2. Tier 2 — repo map, budgeting, scoping

- [x] 2.1 Added tree-sitter + rust/python/js/ts/go grammars to `flows-runner`; `flows-runner/src/repo_map.rs` extracts per-file symbol signatures (first line of each definition node) via tree-sitter.
- [x] 2.2 Rank files by idea↔(path + symbol names) token overlap (camelCase/snake_case-aware) plus a recency boost; cap the map to `repo_map_max_tokens` and append a partial marker when clipped. Unit tests for extraction, tokenize, ranking + budget. _(Recency uses file **mtime** as a proxy — true git-recency is a follow-up.)_
- [x] 2.3 Inject **both** the slim `repo_tree` and the ranked `repo_map` in `agent.rs` (gated on `context.repo_map`); templates (explain/plan/implement) consume it as "Repository symbol map".
- [x] 2.4 `flows-runner/src/budget.rs`: pre-send, estimate request tokens and, when > `budget_fraction * window` (`pricing::context_window` or `unknown_window_tokens`), truncate oldest tool-result bodies to a `[truncated N tokens]` stub, preserving system prompt + task + last `BUDGET_RECENT_KEEP` turns. Wired into the agent loop under `context.budgeting`. _(Estimator is a ~4-char/token **heuristic**, not tiktoken-rs — avoids a heavy tokenizer dep; errs high so it trims sooner.)_
- [x] 2.5 Config keys `budget_fraction` (0.6), `unknown_window_tokens` (8000), `repo_map_max_tokens` (1500) in the `context` block. Tests: no-trim under budget, trims oldest preserving head+recent, unknown-window default path.
- [x] 2.6 Per-phase scoping: `prior_artifacts` exposed **once** as the map; removed the redundant per-key top-level injection. Templates already self-select (`.get('plan')` etc.), so no phase is force-fed unrelated history.
- [x] 2.7 Gate/verify — fmt, clippy -D warnings, tests all green.

## 3. Tier 3 — semantic retrieval (opt-in)

> **Design change:** embeddings run **locally, in-process** via `fastembed` (ONNX),
> not against a hosted `/v1/embeddings` server. The original server design assumed
> the user's inference endpoint also serves an embedding model — it does not, so a
> server path would leave retrieval inert. Local embedding needs zero hosting/zero
> config and works offline after a one-time model download. See `design.md` (D8).

- [x] 3.1 ~~Server embeddings path in `flows-llm`~~ → **local** embeddings: `fastembed`/ONNX in `flows-runner`, model downloaded once to the fastembed cache. No network at inference; no SSRF surface (no model-supplied URLs). _(server `flows-llm/embeddings.rs` was built then removed in favor of local.)_
- [~] 3.2 `flows-runner/src/retrieval.rs`: chunk repo source into **line windows** (~50 lines), gitignore + deny-list aware, capped at 600 chunks; embed in-process. Persisted index added in 3.7. _(Remaining follow-up: line-window chunks are **not** tree-sitter symbols — the repo map from 2.1 is the natural upgrade for chunk boundaries.)_
- [~] 3.3 Cosine retrieval for the phase idea, capped by `retrieval_max_chunks` (default 8); injected into the phase context as a labeled block. Unit tests for cosine, ranked selection, chunking; ignored E2E ranks a relevant chunk first; persisted-index reuse proven in 3.8. _(Remaining follow-up: `budget_fraction`-style auto-scaling of chunk count to the window.)_
- [~] 3.4 Config `retrieval` (default false), `retrieval_model` (local model name; empty → all-MiniLM-L6-v2), `retrieval_max_chunks` (default 8) in the `context` block (global + project). Disabled-by-default path tested. _(No `budget_fraction`; no endpoint — local.)_
- [x] 3.5 N/A under the local design — embeddings make no outbound request at inference (model fetch goes through `hf-hub` to its cache). The `http_get` SSRF guard still covers all model-supplied URLs; no new network surface for the agent.
- [x] 3.6 Gate/verify (fmt, clippy -D warnings, tests) — green; verified end-to-end on the reasoner fixtures (baseline vs oracle vs rag).
- [x] 3.7 `retrieval::RepoIndex`: persist at `~/.flows/index/<repo-hash>/<model>.json` (`chunker_version`, per-file blake3 hashes, chunks + vectors; atomic temp+rename). Built **once per flow** and memoized per process (keyed by canonical repo path + model), so every phase reuses it (only the per-idea query embeds). _(JSON not bincode — reuses the existing `serde_json` dep; no new dep beyond `blake3`.)_
- [x] 3.8 Rebuild decision (binary, no per-file patching): `churn = (changed+new+deleted)/max(prev_total, curr_total)` from blake3 file hashes; rebuild when the index is absent, the model/`CHUNKER_VERSION` changed, or `churn >= retrieval_reindex_threshold`; else reuse as-is. Config key `retrieval_reindex_threshold` (default 0.4). Tests: `churn` math; ignored E2E proves build → unchanged reuse (query-only embed) → high-churn rebuild.
- [x] 3.9 Memoized `load_or_build_index` used from `build_pushed_context` via `spawn_blocking` — no `Ctx` threading needed (process memo gives once-per-flow; disk persistence gives cross-flow reuse for a stable repo path). _(Follow-ups still open: tree-sitter symbol chunks; content-addressed vector cache to avoid branch-switch thrash; eviction cap on the on-disk index.)_

## 4. Gating (do first — every feature lands behind a flag)

- [x] 4.1 Add a `context` config block to `flows-core` with independent flags `search_tool`, `range_reads`, `slim_tree`, `repo_map`, `budgeting`, and `retrieval.enabled` — all defaulting to the pre-change baseline (off). Tests: all-off round-trips baseline.
- [x] 4.2 Add a per-run override (a `run --context k=v,…` flag or run-level config) that sets `context.*` for one run without touching global/project config. Test the override is scoped to the run.
- [x] 4.3 Gate each feature on its flag at the assembly/tool-registration site: `search_tool`/`range_reads`/`slim_tree`, `repo_map` injection, loop `budgeting`, `retrieval` (rag), and the eval-only `oracle` push — each activates only when its flag is set.

## 5. Evaluation harness

- [x] 5.1 Labeled fixture format: `fixture.yml` (task/suite/size/granularity/verify/rubric/**relevant**/configs) + `repo/` snapshot; starter corpus embedded, `--fixtures` for external corpora.
- [x] 5.2 Deterministic retrieval eval (`flows eval retrieval`, no LLM): ranks files by the **repo map** and by **embedding retrieval**, computes recall@1/3/5 + MRR vs each fixture's labeled `relevant` files, plus a MEAN row. Exposed `repo_map::rank_files` + `retrieval::rank_paths`; recall/MRR unit-testable helpers. _(search_code is a query tool, not a ranker, so it's excluded from this ranking eval.)_
- [x] 5.3 `flows eval run` runs `(fixture × config × trial)` headless and stores per-trial tokens, peak context, cost, tool-rounds, pass/verify (from run records).
- [x] 5.4 `flows eval report` prints a grouped per-config comparison table from the store.
- [x] 5.5 Verified on the corpus: end-to-end baseline vs oracle vs rag (reasoner) captured + graded; deterministic `eval retrieval` table captured (map vs rag recall/MRR).

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

- [x] 6.1 README: new "Context delivery for small models" section (the `context.*` block + each feature, push vs pull, local embeddings) + `flows eval retrieval`; eval `--configs` doc de-staled. DESIGN: agent-phase context-delivery subsection.
- [x] 6.2 No `security-model` change — search/range reads keep the file-tool repo-root guard; retrieval reads repo files locally and embeds in-process (no network at inference, no model-supplied URL); oracle is eval-only. No new invariant.
- [x] 6.3 Final full gate green (fmt, clippy -D warnings, all tests); `openspec validate context-delivery-for-small-models` passes.
