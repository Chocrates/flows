## Why

In the go-jobqueue e2e run (context off) the implement phase spent 18 rounds and 52 minutes without editing the target file. About a third of its bash calls were refused by the template's allowlist (`ls`, `grep`, `head`, `echo`, `cat`, `mkdir`, `true`). Each refusal had different text, so the stall guard counted it as new information, and the error listed allowed patterns but not the tool that does the job.

## What Changes

- A tool call refused by policy before anything ran (allowlist, safety block, shell network block, path outside the repo, tool not enabled) counts as bringing nothing new for the stall guard. Failing builds and tests still count as new information.
- An allowlist refusal of a common file command names the tool to use instead: `list_files` for `ls`/`find`, `read_file` for `cat`/`head`/`tail`/`sed`, `search_code` for `grep`/`rg`, `write_file` for `echo`/`mkdir`/`touch`.

## Capabilities

### New Capabilities

### Modified Capabilities
- `agent-loop-guards`: policy refusals are stalls.

## Impact

- `flows-runner::agent` (freshness), `flows-tools::bash` (refusal message).
