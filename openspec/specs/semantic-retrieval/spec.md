# semantic-retrieval Specification

## Purpose
Opt-in push of semantically relevant code chunks into a phase's context, embedded locally in-process, so models that cannot call tools still get grounded context.
## Requirements
### Requirement: Optional embedding-based retrieval

The system SHALL support an opt-in retrieval mode that indexes repository content
as chunks and injects the most relevant chunks for a phase's idea into that
phase's context, bounded by a configurable token budget and a maximum chunk count
(so the injected set never exceeds the available context budget). Retrieval SHALL
be disabled by default and enabled via configuration, so default runs incur no
embedding cost or dependency.

#### Scenario: Retrieval disabled by default

- **WHEN** retrieval is not enabled in configuration
- **THEN** no embedding requests are made and prompt assembly is unchanged.

#### Scenario: Retrieval injects relevant chunks

- **WHEN** retrieval is enabled and a phase runs
- **THEN** the most relevant repository chunks for the idea are injected into that phase's context, up to the configured token budget and max chunk count.

#### Scenario: Injection stays within budget on a small window

- **WHEN** retrieval runs against a small context window
- **THEN** fewer chunks are injected so the retrieved set fits the configured fraction of the context budget (retrieval auto-scales rather than overflowing).

### Requirement: Local, in-process embeddings

The system SHALL compute embeddings **locally, in-process** (via a bundled ONNX
embedder) using a small sentence-transformer model selected by `retrieval_model`
(default all-MiniLM-L6-v2), with no embeddings server and no cloud dependency.
The model SHALL be fetched once to a local cache and reused across runs.

#### Scenario: Works with no hosted embeddings model

- **WHEN** retrieval is enabled and the user runs no `/v1/embeddings` server
- **THEN** embeddings are still computed locally and retrieval works, requiring no endpoint and no API key.

#### Scenario: No new network surface at inference

- **WHEN** a phase computes retrieval
- **THEN** no outbound request is made from a model-supplied URL, so the `http_get` SSRF guard's invariants are unaffected (the only network access is the one-time model download via the embedder's own cache mechanism).

### Requirement: Persisted index, embedded once per flow, rebuilt on high churn

The system SHALL compute the repository embedding index **once per flow** (not per
phase) and hold it for the duration of the run so every phase reuses it. The index
SHALL be persisted outside the model context (on disk, keyed by repository and
embedding model). At flow start the system SHALL reuse the persisted index unless
it is absent, the embedding model or chunker version changed, or the fraction of
files changed since it was built meets a configured threshold
(`retrieval_reindex_threshold`, default 0.4) — in which case it SHALL rebuild the
whole index. Below the threshold the persisted index is reused as-is (recently
edited files may carry slightly stale vectors until the next rebuild).

#### Scenario: One embed pass per flow

- **WHEN** a multi-phase flow runs with retrieval enabled
- **THEN** the repository is embedded at most once for the flow and every phase reuses that index.

#### Scenario: Reuse across flows when little changed

- **WHEN** a repository was indexed and, on a later flow, fewer than the threshold fraction of files changed
- **THEN** the persisted index is reused without re-embedding.

#### Scenario: Rebuild when churn is high or the model/chunker changed

- **WHEN** the fraction of changed files meets the threshold, or the embedding model or chunker version differs from the persisted index
- **THEN** the index is rebuilt in full and re-persisted.

#### Scenario: Incremental update

- **WHEN** some files changed since the last index
- **THEN** only the changed content is re-embedded and the index is updated.

