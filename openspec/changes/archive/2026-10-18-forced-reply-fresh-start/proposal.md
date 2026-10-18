## Why

With context on, the go-jobqueue e2e plan phase stalled on a repeated `search_code "lastdom"`, was forced to reply without tools, and then wrote that same call as Qwen XML text three times, failing the phase. Its history was 60 messages of tool calls; told in words that tools were off, it kept producing tool calls.

## What Changes

- The first time a forced reply comes back as a tool call written as text, the agent restarts the reply in a fresh conversation: one user message with the task, the phase's tool results so far (deduplicated, errors dropped, newest kept within 60k characters), and the instruction to write the artifact. No tools are declared in that request or after it.
- Repeats still count toward the existing limit of three.

## Capabilities

### New Capabilities

### Modified Capabilities
- `agent-loop-guards`: forced replies restart without tool history.

## Impact

- `flows-runner::agent`.
