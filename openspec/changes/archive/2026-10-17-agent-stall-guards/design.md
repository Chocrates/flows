## Context

`run_agent_phase` already has guards for identical failing calls, no-op writes, rounds without a write (after the first write), dry searches (6 in a row) and empty replies (3). The e2e plan phase slipped past all of them: each round's calls differed, some "succeeded" (a dedup pointer, a 404 body), and a read-only phase never writes.

## Goals / Non-Goals

**Goals:** end a deliberation loop in a few rounds instead of the round cap; name the output cap when it is the cause; stop offering a tool whose backend is down.

**Non-Goals:** controlling reasoning effort per profile (server-specific, separate change); time or token budgets per phase.

## Decisions

- **What counts as "nothing new"**: a `read_file` served as the dedup pointer; a `web_search` that was refused (disabled) or returned no result URL; `http_get` / `lookup` returning an error, or an `http_get` with status ≥ 400. A round is stalled only when *every* call in it is one of these and nothing was written. A fresh read, a bash run, or any write resets the streak. Narrow on purpose: a failing build or test is information for the model.
- **Thresholds**: nudge at 3, force at 5, abort write-capable phases at 8. The e2e plan phase would have been nudged at its 8th round and forced at its 10th instead of running 14.
- **Forcing a reply**: the forced request has `tools: []` and appends a user turn saying tools are off and the final artifact JSON is expected. The mode is sticky for the rest of the phase. Text-leaked tool calls are ignored while forced. Write-capable phases are not forced, because a tool-less implement reply would inline code instead of writing it (the empty-output guard would then fail it anyway); they get the nudge and then a clear abort.
- **Output cap**: detected from `stop_reason` `length` (OpenAI-compatible) or `max_tokens` (Anthropic) when the response has no tool call. The truncated text is kept in history as the assistant turn, followed by the nudge. Two consecutive cap hits fail. A productive tool round resets the count.
- **Backend down**: a `web_search` error containing `request failed` (connect/send failure, not an HTTP status) sets the dry-search count to its limit. When search becomes disabled, `web_search` is removed from the tool list sent on later rounds; the existing "search disabled" result still answers any call the model makes anyway.

## Risks / Trade-offs

- [A model legitimately re-reading for a different reason is nudged] → only after 3 consecutive rounds of nothing new; the nudge does not end the phase.
- [Forced tool-less reply returns prose, not JSON] → accepted as `raw_output`, as any unparsed reply is today.
