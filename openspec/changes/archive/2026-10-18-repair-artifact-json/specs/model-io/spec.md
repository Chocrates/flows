## ADDED Requirements

### Requirement: Repair unbalanced artifact JSON

When a reply's JSON does not parse, the system SHALL retry after balancing its brackets outside strings — closing brackets left open, dropping closers with no opener, and closing an array before a `"key":` that follows it — and SHALL accept the result only if it parses as an artifact or questions envelope.

#### Scenario: Last brace missing

- **WHEN** an artifact reply ends one `}` short
- **THEN** it is parsed as the artifact.

#### Scenario: Array left open before the next key

- **WHEN** an artifact has `"files": [{...}, "risks": [...]}` with the array never closed
- **THEN** it is parsed with `files` and `risks` as sibling keys.
