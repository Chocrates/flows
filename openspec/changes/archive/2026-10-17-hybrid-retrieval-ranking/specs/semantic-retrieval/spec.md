## MODIFIED Requirements

### Requirement: Optional embedding-based retrieval

The system SHALL support an opt-in retrieval mode that indexes repository content
as chunks and injects the most relevant chunks for a phase's idea into that
phase's context. Injection SHALL be bounded by a token budget equal to
`retrieval_budget_fraction` (default 0.25) of the phase model's context window —
or of `unknown_window_tokens` when the window is unknown — and by
`retrieval_max_chunks` as a hard cap, whichever is reached first. Chunks SHALL be
packed in relevance order, where relevance is the reciprocal-rank fusion of the
chunk's embedding similarity rank and its lexical (BM25) rank for the idea; a chunk that does not fit the remaining budget SHALL be
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

#### Scenario: Exact terms count

- **WHEN** a chunk shares rare terms with the idea that the embedding ranks low
- **THEN** the lexical ranking lifts it above chunks favoured by the embedding alone.
