## Decisions

- **Binary append-only file**, one per model: records of `[32-byte key][u32 dim][dim × f32 LE]`. JSON would triple the size. Appends use `O_APPEND` with one `write` per batch; a concurrent writer can at worst interleave whole batches. Readers stop at the first incomplete record.
- **Wrap the embedder, not the sync.** `sync_index` already takes an embed closure; production passes one that splits texts into cache hits and misses, embeds the misses, appends them, and returns vectors in order. `sync_index` and its tests stay as they are.
- **Compaction**: after an append, if the file exceeds 256 MiB, rewrite the newest records (by position) up to 128 MiB via temp file + rename. Later duplicates win on load, so order = recency.
- **Location** `~/.flows/vectors/` (not under `index/`, whose LRU pruning is per repo).
- Model name sanitized for the file name (`default` when empty; `/` etc. replaced).
