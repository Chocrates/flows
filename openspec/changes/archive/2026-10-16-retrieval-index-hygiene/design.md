## Context

`RepoIndex` stores `files` (path → blake3) and `chunks` (path, lines, text, vector). `load_or_build_index` reuses it when churn < threshold, else rebuilds every vector.

## Decisions

- **Chunk key = blake3(embed_text).** `embed_text` includes the path and any enclosing header, so the key changes exactly when the embedded input changes. Stored as `key` on `StoredChunk` (serde default empty: legacy chunks never match and are re-embedded once).
- **Sync replaces build.** Chunk the repo (cheap: tree-sitter, no model), look each key up in `old.chunks ∪ old.spare`, embed the misses in one batch. Skip everything when `files` hashes are identical to the stored ones (common case: no chunking at all). Embedding is injected as a closure so sync is unit-testable without downloading a model.
- **Spare pool.** `spare: Vec<{key, vector}>`, most recently dropped first, deduplicated, capped at `MAX_CHUNKS_EMBEDDED` (1500). Bounded memory/disk, covers one typical branch delta.
- **Threshold.** Kept in `ContextConfig` for config compatibility, no longer passed to retrieval.
- **Eviction.** Each repo dir gets a `repo` marker file holding the canonical repo path (cheap to read; JSON indexes can be MBs). Prune orphans (marker present, path missing), then LRU by file mtime across `*.json` (load touches mtime via `File::set_modified`), excluding the current repo's dir; remove empty dirs. Cap is a constant (1 GiB); no config knob until someone needs one. Best effort: errors ignored.

## Risks / Trade-offs

- Index files grow by the spare pool (≤ ~1500 vectors).
- LRU by mtime is approximate on filesystems mounted `noatime`-style for mtime updates failing; worst case evicts a used index, which is rebuilt.
