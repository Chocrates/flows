## ADDED Requirements

### Requirement: Repo-scoped code search tool

The system SHALL provide agents a `search_code` tool that searches the run's
repository for a text or regex pattern and returns matching locations as
`path:line: <matched line>`, scoped to the repository root and honoring the same
path guards as the file tools (no traversal or symlink escape). Results SHALL be
capped to a bounded number of matches with an indication when truncated.

#### Scenario: Pattern match returns locations

- **WHEN** an agent calls `search_code` with a pattern that occurs in the repo
- **THEN** the tool returns a bounded list of `path:line: match` entries within the repository.

#### Scenario: No matches

- **WHEN** the pattern occurs nowhere in the repository
- **THEN** the tool returns an explicit "no matches" result (not an error).

#### Scenario: Search cannot escape the repository

- **WHEN** `search_code` is given a path scope that resolves outside the repository root
- **THEN** the tool refuses and searches nothing outside the root.

#### Scenario: Result set is bounded

- **WHEN** a pattern matches more than the match cap
- **THEN** the tool returns the capped set and indicates results were truncated.

### Requirement: Range-aware file reads

The system SHALL allow `read_file` to return a line range via optional
`start_line` / `end_line` arguments, and SHALL apply a default size cap when no
range is given so a single read cannot dump an unbounded file into the context
window. When output is capped or ranged, the result SHALL indicate the returned
range and that more content exists.

#### Scenario: Read a line range

- **WHEN** an agent calls `read_file` with `start_line` and `end_line`
- **THEN** only that line range is returned, labeled with the range.

#### Scenario: Large file without a range is capped

- **WHEN** an agent reads a file larger than the default cap without a range
- **THEN** the tool returns a capped prefix and indicates the file was truncated and how to read more.

#### Scenario: Invalid range

- **WHEN** the requested range is empty or inverted (e.g. `end_line` < `start_line`)
- **THEN** the tool returns an error explaining the valid range form, and reads nothing.
