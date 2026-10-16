## 1. Window-scaled injection

- [x] 1.1 Add `retrieval_budget_fraction` (default 0.25) to `ContextConfig`, its `context:` block parsing, the starter config comment, and `docs/configuration.md`; test default + parse.
- [x] 1.2 Expose a shared token estimator from `budget.rs` and add best-first packing to `retrieval.rs` (skip chunks that don't fit, stop at `max_chunks` or an exhausted budget); unit-test budget-bound, cap-bound, and skip-oversized cases.
- [x] 1.3 Thread the token budget into `retrieve()` from `agent.rs` using the phase model's window (`pricing::context_window`, else `unknown_window_tokens`); update callers and tests.

## 2. Symbol-aware chunker

- [x] 2.1 Make `repo_map::Lang` and parsing reusable from retrieval (`pub(crate)`), without changing repo-map output.
- [x] 2.2 Implement top-level segmentation (D1), recursion into oversized nodes with enclosing-header context (D2), char-sized merge/split (D3), and line-window fallback (D5) in a new `chunker` module; unit tests for: function kept whole, oversized split, unsupported-language fallback, parse-failure fallback, full non-blank-line coverage, small-definition merging.
- [x] 2.3 Embed `path\n[header\n]source` (D4) while injecting real source; bump `CHUNKER_VERSION` to 2; existing index tests still pass.

## 3. Evaluate and decide

- [x] 3.1 Run `flows eval retrieval --fixtures eval-fixtures` with the new chunker; tune target/max sizes; record before/after tables and per-fixture chunk counts in `design.md`.
- [x] 3.2 Apply the ship gate (D7): if rag MRR and every recall@k ≥ baseline, keep symbol chunks as the default; otherwise revert the chunker switch, keep task 1, and report back to the user.

## 4. Docs & wrap-up

- [x] 4.1 Update `docs/configuration.md`, `DESIGN.md` (context delivery), `AGENTS.md` if needed, and remove the finished items from `docs/ROADMAP.md`.
- [x] 4.2 Full gate green (`make check`); `openspec validate symbol-aware-retrieval` passes.
