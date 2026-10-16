## 1. Incremental sync

- [x] 1.1 `StoredChunk.key`, `RepoIndex.spare`; `sync_index` with injected embedder; unchanged-files fast path
- [x] 1.2 Drop the threshold from `retrieve` / `rank_paths` and callers; deprecate the config key in docs
- [x] 1.3 Tests: unchanged → 0 embeds, edit → only changed chunks, branch switch back → 0 embeds via spare, model change → all, legacy chunk without key re-embedded

## 2. Eviction

- [x] 2.1 Repo marker file; prune orphans then LRU to cap, sparing the current repo; touch on load
- [x] 2.2 Tests: orphan removed, LRU order, current repo kept, empty dirs removed

## 3. Wrap-up

- [x] 3.1 Docs (configuration, DESIGN, ROADMAP), config template comment; `make check`; ignored e2e test updated
