## MODIFIED Requirements

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

## ADDED Requirements

### Requirement: Symbol-aware chunking

The system SHALL split source files in languages the repo map supports (Rust,
Python, JavaScript, TypeScript, Go) into chunks aligned to top-level definition
boundaries found by tree-sitter, so a chunk never starts or ends in the middle of
a definition unless that definition alone exceeds the maximum chunk size. Adjacent
small definitions SHALL be merged up to the target chunk size; a definition larger
than the maximum SHALL be split into consecutive line windows; code between
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
