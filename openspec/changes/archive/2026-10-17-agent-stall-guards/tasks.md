## 1. Agent loop

- [x] 1.1 Treat a capped reply (`length` / `max_tokens`, no tool call) as its own case: nudge once, fail on the second in a row
- [x] 1.2 Disable `web_search` on a connection failure and drop it from the tool list once disabled
- [x] 1.3 Stall counter over "nothing new" rounds: nudge at 3, force a tool-less reply at 5 (read-only), abort at 8 (write-capable)

## 2. Tests

- [x] 2.1 Mock-provider tests: capped once then artifact, capped twice fails, search backend down drops the tool, stall nudge, read-only forced reply with no tools, write-capable abort, reset on new read

## 3. Docs

- [x] 3.1 DESIGN.md loop-guard notes
