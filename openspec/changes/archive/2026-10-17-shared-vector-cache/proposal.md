## Why

Index files are keyed by repository path, so every checkout of the same code embeds it again: each `flows eval` run (fresh temp dirs) re-embeds every fixture and corpus — the whole-workspace fixtures take minutes — and git worktrees or second clones pay the same cost. Chunk vectors are already content-addressed; they only need a store that isn't per-path.

## What Changes

- A per-model vector cache at `~/.flows/vectors/<model>.bin`: chunk key (blake3 of the embedded text) → vector. Index sync consults it before embedding and appends what it embeds.
- Bounded: past 256 MiB the file is compacted to its newest half. Corrupt or truncated tails are ignored. Cache failures never fail retrieval.
- Per-repo indexes and their spare pool are unchanged.

## Capabilities

### Modified Capabilities
- `semantic-retrieval`: adds a shared, bounded vector cache.

## Impact

`flows-runner/src/retrieval.rs` (cache read/append/compact, wired into index sync), docs.
