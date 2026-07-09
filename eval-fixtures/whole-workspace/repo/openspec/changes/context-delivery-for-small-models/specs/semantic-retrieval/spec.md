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

### Requirement: Local-capable, guarded embeddings

The system SHALL obtain embeddings from an OpenAI-compatible `/v1/embeddings`
endpoint (allowing a local llama.cpp server, no cloud required), and all
embedding requests SHALL go through the existing SSRF-guarded HTTP path so
retrieval cannot reach blocked addresses.

#### Scenario: Local embeddings endpoint

- **WHEN** retrieval is configured against a local `/v1/embeddings` endpoint
- **THEN** embeddings are computed with no cloud dependency and no API key required.

#### Scenario: Embeddings honor network guards

- **WHEN** the configured embeddings endpoint resolves to a blocked address
- **THEN** the request is refused by the same guard that protects `http_get`.

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
