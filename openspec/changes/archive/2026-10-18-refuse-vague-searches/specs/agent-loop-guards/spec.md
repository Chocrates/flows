## ADDED Requirements

### Requirement: Vague search queries

The agent SHALL refuse a `lookup` or `web_search` call whose query has fewer than three letters or digits, without making a request, returning an error that asks for a descriptive query. A refused query SHALL count as a round that brought nothing new for the stall guard.

#### Scenario: Alphabet walk

- **WHEN** a read-only phase issues lookups for "x", "q", "p", "o", "n" in consecutive rounds
- **THEN** each is refused, the model is nudged after the third, and tools are switched off after the fifth.

#### Scenario: Absolute form of a repo path

- **WHEN** a file tool is given "/src/a.rs" and "src/a.rs" exists in the repository
- **THEN** the call is still refused and the error names "src/a.rs" as the path to use.
