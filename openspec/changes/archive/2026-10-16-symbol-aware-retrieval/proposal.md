## Why

Embedding retrieval is the push path for small, no-tool models, but it currently
loses to the cheaper tree-sitter repo map on our own fixtures (`flows eval
retrieval`: rag MRR 0.34 vs map 0.47). Its chunks are fixed 50-line windows that
cut functions in half and mix unrelated code, and it always injects up to 8
chunks (~3–4k tokens) regardless of the model's window — roughly half of an 8k
local window. The `semantic-retrieval` spec already promises budget-scaled
injection; the code doesn't do it.

## What Changes

- Chunk source files at **symbol boundaries** (tree-sitter definitions, reusing
  the repo map's language support) instead of fixed line windows. Small adjacent
  definitions are merged, oversized ones are split, code between definitions
  (imports, top-level statements) becomes its own chunk, and unsupported
  languages keep line windows.
- The new chunker **replaces** line windows only if `flows eval retrieval` shows
  rag recall@k and MRR at or above the baseline (r@1 0.08, r@3 0.33, r@5 0.50,
  MRR 0.34). Otherwise it is not shipped as the default and the result is
  reported back.
- **Window-scaled injection:** retrieved code is packed best-first into 25% of
  the phase model's context window (falling back to `unknown_window_tokens` when
  the window is unknown). `retrieval_max_chunks` remains a hard cap. New config
  key `retrieval_budget_fraction` (default 0.25).
- `CHUNKER_VERSION` bumps, so persisted indexes rebuild once automatically.
- Spec cleanup: drop the "Incremental update" scenario from `semantic-retrieval`
  — the implementation (by design D8/3.8) rebuilds in full or reuses, never
  patches per file.

## Capabilities

### New Capabilities

_None._

### Modified Capabilities

- `semantic-retrieval`: chunking becomes symbol-aware; injection is bounded by a
  window-derived token budget plus the chunk cap; the persisted-index requirement
  loses the incorrect incremental-update scenario.

## Impact

- **Code:** `crates/flows-runner/src/retrieval.rs` (chunker, packing),
  `crates/flows-runner/src/repo_map.rs` (expose definition spans),
  `crates/flows-runner/src/agent.rs` (pass the phase model's window),
  `crates/flows-core` (`ContextConfig.retrieval_budget_fraction` + parsing).
- **On-disk format:** the retrieval index (`~/.flows/index/<repo>/<model>.json`)
  gets a new `CHUNKER_VERSION`; old indexes are rebuilt on first use. No change to
  `state.json`, artifacts, or `llm.jsonl`.
- **Security invariants:** unchanged. Chunking reads the same gitignore/deny-list
  file set through `source_files`; embedding stays local and in-process with no
  model-supplied URLs, so path scoping and the SSRF guard are untouched.
- **Dependencies:** none new (tree-sitter grammars already present).
