# agent-loop-guards Specification

## Purpose
Keep an agent phase from spending its round budget once it has stopped making progress: replies cut off at the output limit, tool rounds that bring nothing new, and a search backend that cannot be reached.
## Requirements
### Requirement: Stall guard

The agent loop SHALL count consecutive tool rounds in which nothing was written and every tool call returned nothing new: a re-read of an unchanged file, a disabled or result-less `web_search`, or a failed `http_get` / `lookup` (including an `http_get` with status 400 or above). After 3 such rounds it SHALL tell the model to stop gathering and produce its output. After 5, a phase that cannot write files SHALL be sent its next and later requests with no tools, together with a request for the final artifact; a phase that can write files SHALL fail after 8 with a message naming the stall. Any other tool result, or a write, SHALL reset the count.

#### Scenario: Nudged after three empty rounds

- **WHEN** an agent's tool calls return nothing new three rounds in a row
- **THEN** the next request carries a user message telling it to stop gathering and produce its output.

#### Scenario: Read-only phase forced to answer

- **WHEN** a phase without write access stalls for five rounds
- **THEN** its next request offers no tools and asks for the final artifact, and the reply becomes the phase artifact.

#### Scenario: Write-capable phase aborted

- **WHEN** a phase with write access stalls for eight rounds
- **THEN** the phase fails with a stall message.

#### Scenario: New information resets

- **WHEN** a stalled round is followed by a round that reads a file not read before
- **THEN** the stall count starts again from zero.

### Requirement: Output-cap replies

When a response ends because it reached the output token limit (stop reason `length` or `max_tokens`) and contains no tool call, the agent loop SHALL NOT treat it as the artifact or as an empty reply. It SHALL tell the model its reply was cut off at the limit and to reason briefly before acting. A second consecutive capped reply SHALL fail the phase with a message that names the `max_tokens` limit.

#### Scenario: Capped once

- **WHEN** a response hits the output limit with no tool call
- **THEN** the phase continues with a nudge naming the limit, and the truncated text is not taken as the artifact.

#### Scenario: Capped twice

- **WHEN** two consecutive responses hit the output limit with no tool call
- **THEN** the phase fails with a message naming `max_tokens`.

### Requirement: Unreachable search backend

When `web_search` fails because its backend cannot be reached, the agent loop SHALL disable search for the rest of the phase at once. While search is disabled, `web_search` SHALL NOT be offered in the tool list, and a call to it SHALL be answered with a message telling the model to proceed without searching.

#### Scenario: Backend down

- **WHEN** a `web_search` call fails to connect to its backend
- **THEN** later requests in the phase do not offer `web_search`, and no further search request is sent.

