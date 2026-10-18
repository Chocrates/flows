## ADDED Requirements

### Requirement: Forced reply restarts without tool history

When a phase whose tools were switched off answers with a tool call written as text, the next request SHALL contain no tool-call history and declare no tools: a single user message carrying the task, the phase's unique non-error tool results so far (bounded in size), and the instruction to reply with the artifact.

#### Scenario: Tool call after tools are off

- **WHEN** a read-only phase, forced to reply, answers with a tool call written as text
- **THEN** the next request declares no tools and its only message contains the task, the earlier tool results once each, and the instruction to write the artifact.
