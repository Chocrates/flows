## ADDED Requirements

### Requirement: Round cap ends with a forced reply

When a phase has 6 or more rounds in its budget and only 3 remain, the agent loop SHALL switch tools off and ask the model for its final artifact, as for a stalled read-only phase. It SHALL do this for phases that can write files too. A phase with a smaller budget SHALL fail at the cap as before.

#### Scenario: Review reaches its cap

- **WHEN** a phase with an 8-round budget makes progress in each of its first 5 rounds
- **THEN** its sixth request offers no tools and asks for the artifact, and the reply becomes the phase artifact.
