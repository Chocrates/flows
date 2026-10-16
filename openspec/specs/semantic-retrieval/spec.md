# semantic-retrieval Specification

## Purpose
Opt-in push of semantically relevant code chunks into a phase's context, embedded locally in-process, so models that cannot call tools still get grounded context.
## Requirements
### Requirement: Optional embedding-based retrieval

The system SHALL support an opt-in retrieval mode that indexes repository content
as chunks and injects the most relevant chunks for a phase's idea into that
phase's context. Injection SHALL be bounded by a token budget equal to
`retrieval_budget_fraction` (default 0.25) of the phase model's context window —
or of `unknown_window_tokens` when the window is unknown — and by
`retrieval_max_chunks` as a hard cap, whichever is reached first. Chunks SHALL be
packed in relevance order; a chunk that does not fit the remaining budget SHALL be
skipped in favour of a smaller, lower-ranked one that does. Retrieval SHALL be
disabled by default and enabled via configuration, so default runs incur no
embedding cost or dependency.

#### Scenario: Retrieval disabled by default

- **WHEN** retrieval is not enabled in configuration
- **THEN** no embedding requests are made and prompt assembly is unchanged.

#### Scenario: Retrieval injects relevant chunks

- **WHEN** retrieval is enabled and a phase runs
- **THEN** the most relevant repository chunks for the idea are injected into that phase's context, up to the token budget and max chunk count.

#### Scenario: Injection stays within budget on a small window

- **WHEN** retrieval runs for a model with a small (or unknown, defaulting to `unknown_window_tokens`) context window
- **THEN** the estimated tokens of the injected chunks do not exceed `retrieval_budget_fraction` of that window, so fewer chunks are injected than on a large window.

#### Scenario: Chunk cap still applies on a large window

- **WHEN** retrieval runs for a model whose window budget could hold more than `retrieval_max_chunks` chunks
- **THEN** at most `retrieval_max_chunks` chunks are injected.

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

### Requirement: Symbol-aware chunking

The system SHALL split source files in languages the repo map supports (Rust,
Python, JavaScript, TypeScript, Go) into chunks aligned to top-level definition
boundaries found by tree-sitter, so a chunk never starts or ends in the middle of
a definition unless that definition alone exceeds the maximum chunk size. Adjacent
small definitions SHALL be merged up to the target chunk size; a definition larger
than the maximum SHALL be split into its member definitions (e.g. the methods of
an `impl` or class, keeping the enclosing header as embedding context) or, when
that does not reduce it, into consecutive size-bounded line windows; code between
definitions (imports, top-level statements) SHALL be chunked as well, so no
non-blank source line is left unindexed. Files in other languages, or that fail
to parse, SHALL fall back to fixed line windows.

#### Scenario: Function kept whole

- **WHEN** a supported-language file contains a function shorter than the maximum chunk size
- **THEN** the function's full definition appears within a single chunk.

#### Scenario: Oversized definition split

- **WHEN** a single definition is longer than the maximum chunk size
- **THEN** it is split into consecutive chunks covering all of its lines, none larger than the maximum.

#### Scenario: Unsupported language falls back

- **WHEN** a file is in a language without a tree-sitter grammar (e.g. Markdown, YAML) or fails to parse
- **THEN** it is chunked into fixed line windows as before.

#### Scenario: Full coverage

- **WHEN** a supported-language file is chunked
- **THEN** every non-blank line of the file is contained in exactly one chunk.

### Requirement: Bounded index that prefers code

The system SHALL cap the number of chunks embedded per index build (1500). When a
repository exceeds the cap, source code files SHALL be indexed before docs/config
files (Markdown, TOML, YAML, SQL), so the cap drops the least useful content
first, and the system SHALL surface a warning naming how many files were not
fully indexed rather than truncating silently.

#### Scenario: Docs dropped before code

- **WHEN** a repository has more chunks than the cap
- **THEN** all code chunks that fit are indexed before any docs/config chunk.

#### Scenario: Truncation is reported

- **WHEN** the cap leaves any file not fully indexed
- **THEN** a warning reports the number of affected files and is shown to the user through the run's notes.

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

