## Context

Agent prompts today are assembled in `flows-runner/src/agent.rs`: the context map
gets `idea`, `repo_tree` (`repo_context::build_repo_tree(repo_root, 500)` — a flat
500-entry list), `phase_name`, and `prior_artifacts` (injected as the whole map
*and* each key individually). The only navigation tools are `read_file`
(whole-file) and `list_files`; there is no search. On a large repo with a small
model this fails: the tree is truncated noise, whole-file reads overflow the
window, and the model can't locate code. We already measure the live
context-window size (`UsageReport.last_input_tokens`, `pricing::context_window`),
which Tier 2 budgeting builds on. The sandbox / path-scoping / SSRF invariants in
`openspec/specs/security-model` must be preserved by everything here.

## Goals / Non-Goals

**Goals:**
- Let a small model *find* and read exactly the code it needs (search + range reads).
- Give it a bounded, high-signal view of a large repo (slim tree, then a ranked symbol map).
- Never overflow the context window (loop-level budgeting off the measured size).
- Keep it local-first: no required cloud dependency; Tier 3 is opt-in and llama.cpp-capable.

**Non-Goals:**
- Changing the phase/pipeline model, sandbox, or on-disk run formats.
- Perfect symbol extraction (a heuristic map is enough for v1; tree-sitter is a later upgrade).
- Cross-repo / multi-repo indexing.

## Decisions

**D1 — Search via the `ignore` + `regex` crates, not shelling to ripgrep.**
Use ripgrep's library (`ignore` for gitignore-aware walking, `regex`/`grep`) in a
new `flows-tools/src/search.rs`, reusing the file tools' repo-root path guard.
Rationale: no external runtime binary, cross-platform, same confinement as the
other tools. Alternative (shell `rg` in the sandbox) rejected: adds a runtime
dependency and sandbox interplay for no gain.

**D2 — Range reads extend the existing limited reader.**
`read_file` gains optional `start_line`/`end_line`; with no range it uses a
default cap (extend `files::read_file_limited`). The tool result labels the
returned range and whether more exists. Cheap; the plumbing already exists.

**D3 — Repo tree becomes bounded + summarizing.**
Rewrite `build_repo_tree` to be gitignore-aware (`ignore` crate), depth-bounded,
and size-capped; past the cap it summarizes (dir + counts) and points at
`search_code`/`list_files` instead of emitting a truncated flat list.

**D4 — Repo map from tree-sitter symbols, ranked by idea overlap.** *(resolves Q1)*
Extract symbol signatures with **tree-sitter** grammars (start with the `@auto`
language set: rust, python, js/ts, go) into a `repo_map` variable; rank files by
token overlap between the idea and file path + symbol names, plus a git-recency
boost; cap to a token budget. Rationale: accurate symbols (macros, nesting,
generics) and a reusable parsing layer that Tier 3 chunking (D7) also uses.
Accepted cost: a larger dependency tree (per-language grammar crates) and slower
builds. Alternative (heuristic per-language regex) was lighter but imprecise and
not reusable — rejected. The map is still a *hint*; the model reads real code via
range reads / `search_code`.

**D5 — Budgeting trims old tool results, preserving head + recent turns.** *(defaults per Q2)*
In the agent loop, estimate the request's token size (tiktoken-rs, already a dep)
before sending; when it exceeds `budget_fraction * context_window` (or
`unknown_window_tokens` when the window is unknown), replace the oldest
tool-result message bodies with a `[truncated N tokens — re-read if needed]` stub,
keeping the system prompt, the task, and the most recent turns intact. Both knobs
are config, with defaults `budget_fraction = 0.6` and `unknown_window_tokens =
8000`.

**D6 — Per-phase scoping removes the double-injection, keeps map + slim tree.** *(resolves Q3)*
Expose prior artifacts exactly once (drop either the full-map insert or the
per-key inserts, keeping whatever the prompt templates actually reference) and
inject only what a phase's template needs. Agent prompts carry **both** the slim
`repo_tree` (layout, kept intentionally small) **and** the ranked `repo_map`
(symbol detail); the tree gives orientation, the map gives what's inside the
relevant files.

**D7 — Tier 3 retrieval: symbol chunks, budget-bounded, `/v1/embeddings`, opt-in.** *(resolves Q4)*
Add an embeddings request path in `flows-llm`; chunk source files **by tree-sitter
symbol (reusing D4), splitting any symbol over ~350 tokens** so no chunk
dominates; embed via an OpenAI-compatible `/v1/embeddings` endpoint through the
SSRF-guarded HTTP layer; persist vectors under `~/.flows` keyed by a repo hash;
incremental re-embed by per-chunk content hash. Retrieve highest-scoring chunks
**until a token budget fills** (`retrieval.budget_fraction`, default 0.25 of the
phase's context budget) rather than a fixed k, capped by `retrieval.max_chunks`
(default 8) — so retrieval auto-scales (~8 chunks on a large window, 2–3 on a
tiny local one) and never overflows. Gated by `retrieval.enabled` (default off) so
default runs are unaffected.

**D8 — Gate every feature independently, default to baseline, and evaluate.**
Each feature sits behind a `context.*` flag defaulting to the pre-change baseline
(off), with a per-run override so a harness can A/B without editing config. Prove
usefulness in two layers: (1) a **deterministic retrieval eval** — labeled
`task → relevant files/symbols` fixtures scored by recall@k / MRR, no model call,
so retrieval quality is isolated from model variance; (2) an **end-to-end eval**
(`flows eval`) running `(repo, task, verifier)` fixtures under each config,
pulling success + tokens + peak context + cost + tool-rounds from the run records,
at temperature 0 with N trials for comparability. Rationale: "usefulness" must be
a measured delta vs baseline, per model, not an assumption — small models may
benefit from different tiers than large ones. Defaults flip to the proven-good set
in a follow-up once the eval table justifies it.

## Risks / Trade-offs

- **Heuristic map misses some symbols** → it's a navigation hint, not ground truth; the model verifies via range reads. Upgrade to tree-sitter later without a spec change.
- **Search/read caps hide content** → always indicate truncation and how to fetch more (next range, refine pattern), so the model can iterate.
- **Budget summarization drops needed detail** → only old *tool results* are trimmed (never the task or recent turns), and trimmed files remain re-readable via `search_code` + range reads.
- **RAG index staleness / cost / disk** → incremental by content hash, opt-in, local-capable; index lives outside the run so it's cache, not state.
- **New dependency (`ignore`)** → small, ubiquitous (ripgrep core); tree-sitter deliberately deferred to avoid a heavy dep now.

## Migration Plan

Ship in tiers; each is independently revertible and adds no on-disk format
changes. Tier 1 (search, range reads, slim tree) → Tier 2 (repo map, budgeting,
scoping cleanup) → Tier 3 (retrieval, config-gated, default off). Rollback for
Tier 3 is flipping the config default; Tiers 1–2 are behavioral and revert with
the code. Prompt templates are updated alongside the variables they consume.

## Resolved Decisions

All four open questions were resolved with the user:

- **Symbol extractor** → tree-sitter now (D4), accepting the heavier dep for accuracy + reuse.
- **Budgeting** → configurable, defaults `budget_fraction = 0.6`, `unknown_window_tokens = 8000` (D5).
- **Map vs tree** → inject both; slim tree for layout + ranked map for symbols (D6).
- **RAG chunking** → tree-sitter symbol chunks split at ~350 tokens, budget-bounded retrieval (`retrieval.budget_fraction` 0.25, `retrieval.max_chunks` 8) instead of a fixed k (D7).

## Open Questions

- None outstanding. (Chunking/index cache location is fixed under `~/.flows`; embedding model is whatever the configured `/v1/embeddings` endpoint serves.)
