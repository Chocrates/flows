## Why

An e2e run on the go-jobqueue corpus (task: "L" = last day of month in timetable expressions, context off) failed its plan phase at the 43-round cap. After two real lookups the model walked the alphabet backwards — `lookup crates_io "x"`, `"qq"`, `"q"`, `"p"` … `"b"` — 31 calls. Every answer differed, so each counted as new information and no stall guard fired. The same run also lost a round to `read_file "/internal/schedule/cron.go"`, refused with "Path escapes repo root" and no hint.

## What Changes

- `lookup` and `web_search` refuse a query with fewer than 3 letters or digits, with an error telling the model to describe what it needs or stop searching. The refusal counts as a failed fetch, so repeated ones trip the existing stall guard (nudge at 3, tool-less reply at 5 / abort at 8).
- When a refused absolute path names an existing repo file, the error says to use the repo-relative form.

## Capabilities

### New Capabilities

### Modified Capabilities
- `agent-loop-guards`: vague search queries are refused and count as stalls.

## Impact

- `flows-runner::agent` (dispatch), `flows-tools::files` (`safe_path` message).
