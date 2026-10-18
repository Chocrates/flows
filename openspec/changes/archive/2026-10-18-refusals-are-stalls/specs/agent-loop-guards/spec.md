## ADDED Requirements

### Requirement: Policy refusals are stalls

A tool call refused by policy before it ran (command allowlist, safety block, shell network block, path outside the repository, tool not enabled for the phase) SHALL count as a round that brought nothing new for the stall guard. An allowlist refusal of a common file command SHALL name the file tool that does the same job.

#### Scenario: Refused commands abort a write-capable phase

- **WHEN** a write-capable phase issues a different refused shell command every round
- **THEN** the stall guard aborts it as stalled rather than letting it run to the round cap.

#### Scenario: Refused ls

- **WHEN** `ls -la` is refused by the allowlist
- **THEN** the error names the `list_files` tool.
