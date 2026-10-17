## Why

A real end-to-end run (`e2e` template, scratch `durations` crate) completed, but its `plan` phase took 14 rounds, 93k output tokens and 107 minutes to write a one-function plan. The model re-reasoned from scratch every round and then made a call that could not help: re-reading an unchanged file (served as a dedup pointer), `web_search` against a SearXNG that was down (connection refused, four times), and `http_get` on a 404. None of the existing guards trip on this pattern: the calls are varied, not all errors, and no file is written. An earlier run (f481019e) died after three implement responses that each hit the 16k output cap mid-reasoning; it was reported as "empty response" after 57 minutes, with no hint that the cap was the cause.

## What Changes

- **Stall guard**: count consecutive tool rounds where every call came back with nothing new (a deduplicated re-read, a disabled or failed search, a failed or 4xx/5xx `http_get`/`lookup`). After 3, nudge the model to stop gathering and produce its output. After 5, a read-only phase gets one more request with no tools offered, so it must reply with its artifact; a write-capable phase is aborted after 8 with a stall message.
- **Output-cap handling**: a response whose stop reason is the output cap (`length` / `max_tokens`) with no tool call gets a targeted nudge (reason briefly, then act) instead of being taken as an empty or raw artifact. A second consecutive cap hit fails the phase with a message naming `max_tokens`.
- **Unreachable search backend**: a `web_search` that fails to connect disables search for the rest of the phase at once, and a disabled `web_search` is dropped from the tool list offered to the model.

## Capabilities

### New Capabilities
- `agent-loop-guards`: the agent loop's stall, output-cap and dead-backend guards.

### Modified Capabilities

## Impact

- `crates/flows-runner/src/agent.rs` (loop state, tool list per request).
- `crates/flows-runner/tests/agent.rs` (mock-provider tests).
- `docs/architecture` / DESIGN.md loop-guard notes.
