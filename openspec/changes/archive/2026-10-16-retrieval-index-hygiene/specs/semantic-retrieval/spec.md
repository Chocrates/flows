## REMOVED Requirements

### Requirement: Persisted index, embedded once per flow, rebuilt on high churn

**Reason**: Replaced by incremental, content-addressed sync, which keeps the index fresh without whole-repo rebuilds.
**Migration**: `retrieval_reindex_threshold` is still accepted but ignored; remove it from configs at leisure.

## ADDED Requirements

### Requirement: Persisted index, synced incrementally once per flow

The system SHALL compute the repository embedding index **once per flow** (not per phase) and hold it for the duration of the run so every phase reuses it. The index SHALL be persisted outside the model context (on disk, keyed by repository and embedding model). At flow start the system SHALL sync the persisted index with the repository: chunks whose embedded text is unchanged SHALL keep their stored vectors, and only new or changed chunks SHALL be embedded. Vectors of chunks that no longer exist SHALL be kept in a bounded spare pool and reused if the same text reappears. A change of embedding model or chunker version SHALL discard all stored vectors.

#### Scenario: One embed pass per flow

- **WHEN** a multi-phase flow runs with retrieval enabled
- **THEN** the repository is synced at most once for the flow and every phase reuses that index.

#### Scenario: Unchanged repository

- **WHEN** a later flow starts on a repository whose files have not changed
- **THEN** the persisted index is reused and no chunk is embedded.

#### Scenario: Edited file

- **WHEN** one file changed since the index was saved
- **THEN** only that file's new or changed chunks are embedded, and retrieval sees the current content.

#### Scenario: Switching back to a branch

- **WHEN** the repository returns to content it had at an earlier sync (e.g. checking a branch out again)
- **THEN** the chunks are restored from the spare pool without re-embedding, as long as the pool still holds them.

#### Scenario: Model or chunker changed

- **WHEN** the embedding model or chunker version differs from the persisted index
- **THEN** every chunk is embedded again.

### Requirement: Bounded index cache

After saving an index the system SHALL delete persisted indexes whose repository path no longer exists, then delete the least-recently-used index files until the index directory is within its size cap (1 GiB). An index SHALL count as used when it is loaded or saved. The current repository's index SHALL NOT be evicted, and eviction failures SHALL NOT fail the run.

#### Scenario: Deleted checkout

- **WHEN** an index belongs to a repository directory that no longer exists
- **THEN** it is deleted at the next index save.

#### Scenario: Cache over its cap

- **WHEN** the index directory exceeds its cap after a save
- **THEN** the least recently used index files of other repositories are deleted until it fits.
