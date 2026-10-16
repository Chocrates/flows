## Context

`retrieval.rs` chunks every source file into fixed 50-line windows, embeds them
locally (fastembed, all-MiniLM-L6-v2 by default), and injects the top
`retrieval_max_chunks` (default 8) by cosine similarity. Two problems:

1. **Chunk quality.** Windows cut definitions in half and glue unrelated code
   together. They are also too long for the embedder: fastembed truncates input
   at 512 word-piece tokens (MiniLM was trained at 256), and a 50-line window of
   code is often 700+ word pieces, so the tail of many chunks is never embedded.
   On our fixtures rag scores below the much cheaper repo map (MRR 0.34 vs 0.47).
2. **Budget.** Injection ignores the model's window. 8 windows is ~3–4k
   estimated tokens — about half of an 8k local window, which is exactly the kind
   of model the push path exists for.

`repo_map.rs` already parses Rust/Python/JS/TS/Go with tree-sitter and knows
each language's definition node kinds. `budget.rs` has the ~4 chars/token
estimator, and `agent.rs` already resolves the phase model's window
(`pricing::context_window`, falling back to `context.unknown_window_tokens`) for
in-loop budgeting.

## Goals / Non-Goals

**Goals:**
- Chunks aligned to definitions, sized to fit the embedder.
- Injected retrieval fits 25% of the phase model's window; `retrieval_max_chunks` stays a hard cap.
- Ship the new chunker as the default only on a measured eval win.

**Non-Goals:**
- Incremental (per-file) re-embedding or a content-addressed vector cache (still a roadmap item).
- New languages beyond the five the repo map supports.
- Changing ranking (still cosine over chunk vectors) or the repo map itself.
- A real tokenizer for budgeting.

## Decisions

### D1. Segment by top-level syntax nodes, not just definition kinds

Use the root node's named children as the unit boundaries. Each segment runs from
the line after the previous segment to the end of its node, so leading doc
comments and attributes attach to the definition below them, and imports or
top-level statements become segments of their own. Trailing lines go to the last
segment. This covers every line by construction (the "full coverage" requirement)
without per-language rules for comments.

*Alternative:* collect only `def_kinds` nodes (what the repo map does) and fill
the gaps. Rejected: gap handling is where coverage bugs hide, and top-level
statements (e.g. Python module code) matter for retrieval.

### D2. Oversized segments recurse, then window

If a segment is over the max size, segment the node's own named children the same
way (an `impl` block or `class` splits into its methods). If that doesn't help (a
single huge function), split it into line windows of at most the max size. The
enclosing header (e.g. `impl Foo for Bar {`) is prefixed to the *embedded* text
of child chunks, so a method chunk still says what it belongs to; the injected
text stays the real source lines.

### D3. Size by characters; merge small neighbours

Target ≈ 1,200 chars and max ≈ 2,400 chars per chunk (≈ 300 / 600 estimated
tokens, comfortably under the embedder's 512 word-piece cap for typical code).
Adjacent segments are merged greedily while the result stays ≤ target, so a file
of one-line constants doesn't explode into dozens of chunks. Characters rather
than lines because both the embedder limit and the injection budget are about
text size. The exact constants are tuned against `flows eval retrieval`
(task 3) and recorded there.

### D4. Embed with a path header

The embedded text is `path\n[enclosing header\n]source`. The repo map's edge
comes partly from matching idea words to file paths; giving the embedder the path
lets rag use the same signal. Injected text is unchanged (`### path:start-end`
plus the source block).

### D5. Fallback = today's line windows

Unsupported extensions (md, toml, yaml, sql, …) and parse failures use the
existing line-window chunker, so the indexed file set is identical to today's.
The `MAX_CHUNKS_EMBEDDED` cap (600) stays.

### D6. Token-budgeted packing

`retrieve()` takes a token budget alongside `max_chunks`. It walks chunks in
relevance order, adds each one whose estimated tokens (shared `budget` estimator,
including the `### path:a-b` header and fences) fit the remaining budget, skips
ones that don't, and stops at `max_chunks` or when the budget is used up. Budget =
`retrieval_budget_fraction × window`, where window is the **phase's** resolved
model window (`pricing::context_window`) or `unknown_window_tokens`.

*Alternative:* stop at the first chunk that doesn't fit. Rejected: one oversized
top hit would starve the injection.

### D7. Ship gate

Run `flows eval retrieval --fixtures eval-fixtures` before (baseline already
recorded: rag r@1 0.08, r@3 0.33, r@5 0.50, MRR 0.34) and after. Ship symbol
chunks as the default only if rag MRR and every recall@k are ≥ baseline. If it
loses, stop and report back rather than adding a flag on our own. Window scaling
ships either way (it's independent of the chunker and fixes a spec violation).

## Risks / Trade-offs

- **Small eval corpus (6 fixtures, mostly this repo)** → a "win" may not
  generalize. Mitigation: require no regression on any recall@k, not just a mean
  gain; note the corpus size in the results.
- **More, smaller chunks** could hit the 600-chunk cap on mid-size repos where
  50-line windows didn't. Mitigation: merging small neighbours keeps the count
  close; record the chunk counts per fixture in the eval notes. If counts rise
  past the cap, raise it in the same change.
- **Index rebuild on upgrade** (`CHUNKER_VERSION` 1 → 2) costs one re-embed per
  repo. Acceptable; it's automatic.
- **Budget estimator is heuristic** (~4 chars/token) → may over- or under-fill
  slightly. It errs high, so it under-fills, which is the safe direction.
- **Smaller default injection on unknown windows** (8000 × 0.25 = 2,000 tokens vs
  ~3–4k today) → slightly less grounding for local models with no window entry.
  Intended, and users can raise `retrieval_budget_fraction` or set the window.

## Migration Plan

No user action. The first retrieval run after upgrade sees a chunker-version
mismatch and rebuilds the index. Rollback = revert; the old binary sees version 2
as a mismatch and rebuilds with line windows.

## Results (task 3)

`flows eval retrieval`, all-MiniLM-L6-v2, 6 fixtures. Sizes kept at the design
values (target 1,200 / max 2,400 chars); no tuning was needed.

| rag ranker | r@1 | r@3 | r@5 | MRR |
|---|---|---|---|---|
| Baseline (50-line windows, no path) | 0.08 | 0.33 | 0.50 | 0.34 |
| **Symbol chunks + path header (shipped)** | **0.72** | **0.89** | **0.89** | **1.00** |
| Symbol chunks, no path header | 0.50 | 0.67 | 0.67 | 0.71 |

Every fixture task names its target file, which could flatter a path-aware
embedding. So the same configs were re-run with paths stripped from the task text:

| rag ranker (path-stripped tasks) | r@1 | r@3 | r@5 | MRR |
|---|---|---|---|---|
| Line windows, no path (old) | 0.47 | 0.81 | 0.89 | 0.81 |
| Line windows + path header | 0.47 | 0.81 | 0.89 | 0.81 |
| Symbol chunks, no path header | 0.67 | 0.89 | 0.89 | 0.89 |
| **Symbol chunks + path header (shipped)** | **0.72** | **0.89** | **0.89** | **1.00** |

Symbol chunking is the main gain. The path header only helps on top of it (with
line windows it changes nothing). No fixture regresses vs. the old chunker in
either set. **Ship gate (D7) passed.** Weakest case: `whole-workspace-hard`, three
target files, r@5 0.33 — only one of three lands in the top 5.

Chunk counts: whole-workspace fixtures grow from ~492 line windows to 699 symbol
chunks, and this repo from 600 (capped) to 1,106 — over the old 600 cap, which
silently dropped late-path files. The cap is now 1,500, code is indexed before
docs/config, and truncation produces a warning (user decision). Eval wall time:
235s vs 253s baseline (smaller chunks embed faster despite there being more).
