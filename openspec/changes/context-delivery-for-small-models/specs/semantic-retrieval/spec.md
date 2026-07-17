## ADDED Requirements

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

### Requirement: Cached, incremental index

The system SHALL persist the embedding index outside the model context (on disk)
and update it incrementally, re-embedding only content that changed since the
last index, so repeated runs do not re-embed the whole repository.

#### Scenario: Reuse across runs

- **WHEN** a repository is indexed and unchanged on a later run
- **THEN** the existing index is reused without re-embedding unchanged content.

#### Scenario: Incremental update

- **WHEN** some files changed since the last index
- **THEN** only the changed content is re-embedded and the index is updated.
