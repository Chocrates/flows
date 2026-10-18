## Why

Two e2e runs on a 27B local model, after loop-backs started working:

- A plan phase sent 32 `lookup` calls that were rewordings of one question ("quartz cron L last day of month", "go cron L last day of month", ...). Each returned different hits, so the stall guard saw progress every round, and the phase ran to its round cap.
- After review looped back to implement, the second implement pass only re-ran tests over code that was already written. The "files_written but nothing written" guard failed it, and with it the run.

## What Changes

- A `web_search` or `lookup` call is checked against the phase's earlier queries. When at least half of its words, measured as the share of the combined word set (Jaccard), match an earlier query's, it counts as bringing nothing new for the stall guard. The call still runs.
- The `files_written` guard fails a phase only when the working tree also has no uncommitted changes, so a verify-only pass over existing work succeeds.

## Capabilities

### New Capabilities

### Modified Capabilities
- `agent-loop-guards`: rephrased searches count as stalls; verify-only passes are allowed.

## Impact

- `flows-runner::agent` only.
