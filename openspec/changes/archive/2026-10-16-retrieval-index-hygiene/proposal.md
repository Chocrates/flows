## Why

The persisted retrieval index is all-or-nothing. Below `retrieval_reindex_threshold` (40% of files changed) it is reused as-is, so edited files keep stale vectors; at or above it the whole repo is re-embedded, which is what a branch switch usually triggers. And `~/.flows/index/` grows forever: one directory per repo path ever indexed, including deleted checkouts and temp dirs.

## What Changes

- **Content-addressed vectors.** Each stored chunk carries a hash of the text it was embedded from. On every flow start the index is re-synced: unchanged chunks keep their vectors, and only new or changed chunks are embedded. The index is never stale.
- **Spare pool.** Vectors for chunks that disappear (e.g. on a branch switch) are kept in a bounded pool, so switching back re-embeds nothing.
- **`retrieval_reindex_threshold` is deprecated.** It is still accepted but no longer read; incremental sync made it unnecessary.
- **Eviction.** After saving an index, flows deletes indexes whose repository no longer exists and then evicts the least-recently-used index files until `~/.flows/index/` is under 1 GiB. The index for the current repository is never evicted.

## Capabilities

### Modified Capabilities
- `semantic-retrieval`: the persisted-index requirement changes from threshold-based rebuilds to incremental sync, and gains eviction.

## Impact

`flows-runner/src/retrieval.rs` (index format gains fields, all serde-defaulted, so old indexes load and are upgraded on the next sync), `agent.rs` / eval callers drop the threshold argument, docs, config template.
