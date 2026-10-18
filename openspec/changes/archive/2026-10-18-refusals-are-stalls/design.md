## Context

`fresh` excludes deduplicated reads, dry searches, failed fetches and identical repeats. Errors are otherwise fresh, which is right for compiler and test output.

## Decisions

- **Match refusals by message** (`Command not in allowlist`, `Command blocked by safety policy`, `Network access from shell commands is blocked`, `cd is only allowed`, `Could not parse command`, `Path escapes repo root`, `tool not enabled for this phase`). They are produced by flows itself, so the strings are stable; a typed error would need a tool-result type through dispatch for no other gain.
- **Hints only for coreutils with a tool equivalent**; other refusals keep the allowed-pattern list alone.

## Risks / Trade-offs

- [A refused command after real progress pushes toward the nudge sooner] → intended; three refusals in a row is already a stall.
