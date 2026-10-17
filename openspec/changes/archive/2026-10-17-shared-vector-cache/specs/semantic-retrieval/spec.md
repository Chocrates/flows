## ADDED Requirements

### Requirement: Shared vector cache

The system SHALL keep a per-embedding-model cache of chunk vectors keyed by the hash of the embedded text, shared by every repository path, and SHALL consult it before embedding a chunk during index sync, adding newly embedded vectors to it. The cache SHALL be bounded in size by discarding its oldest entries, SHALL ignore unreadable or truncated entries, and its failures SHALL NOT fail retrieval.

#### Scenario: Second checkout of the same code

- **WHEN** a repository is indexed and then the same files are indexed under a different path
- **THEN** the second index embeds nothing.

#### Scenario: Cache over its cap

- **WHEN** appending to the cache makes it exceed its size cap
- **THEN** it is rewritten keeping only its newest entries.

#### Scenario: Damaged cache

- **WHEN** the cache file is truncated mid-entry
- **THEN** the intact entries are still used and indexing succeeds.
