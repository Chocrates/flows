## MODIFIED Requirements

### Requirement: Local, in-process embeddings

The system SHALL compute embeddings **locally, in-process** (via a bundled ONNX
embedder) using a sentence-embedding model selected by `retrieval_model`
(default all-MiniLM-L6-v2; also `bge-small`, `bge-base`, `jina-code`, `gemma`),
with no embeddings server and no cloud dependency.
The model SHALL be fetched once to a local cache outside the repository (`~/.flows/models`, or `FASTEMBED_CACHE_DIR` when set) and reused across runs.
Queries and chunks SHALL be embedded with the instruction prefixes the selected
model was trained with, and vectors embedded with different prefixes SHALL NOT be
reused. Embedding SHALL run in small fixed-size batches so peak memory does not
grow with the size of the repository. An unrecognised `retrieval_model` SHALL
fall back to the default with a warning naming the known models.

#### Scenario: Model cache stays out of the repo

- **WHEN** retrieval downloads its embedding model during a run
- **THEN** nothing is written to the repository directory.

#### Scenario: Works with no hosted embeddings model

- **WHEN** retrieval is enabled and the user runs no `/v1/embeddings` server
- **THEN** embeddings are still computed locally and retrieval works, requiring no endpoint and no API key.

#### Scenario: No new network surface at inference

- **WHEN** a phase computes retrieval
- **THEN** no outbound request is made from a model-supplied URL, so the `http_get` SSRF guard's invariants are unaffected (the only network access is the one-time model download via the embedder's own cache mechanism).

#### Scenario: Model prefixes applied

- **WHEN** `retrieval_model` is a model trained with query/document instructions (e.g. `gemma`, `bge-small`)
- **THEN** the idea and the chunks are embedded with those instructions, and the index is stored apart from vectors embedded without them.

#### Scenario: Mistyped model name

- **WHEN** `retrieval_model` names no known model
- **THEN** retrieval uses the default model and the run shows a warning naming the known models.
