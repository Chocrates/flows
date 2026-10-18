# agent-loop-guards Specification

## Purpose
Keep an agent phase from spending its round budget once it has stopped making progress: replies cut off at the output limit, tool rounds that bring nothing new, and a search backend that cannot be reached.
## Requirements
### Requirement: Stall guard

The agent loop SHALL count consecutive tool rounds in which nothing was written and every tool call returned nothing new: a re-read of an unchanged file, a disabled or result-less `web_search`, a failed `http_get` / `lookup` (including an `http_get` with status 400 or above), or a read-only call (`list_files`, `search_code`, `web_search`, `http_get`, `lookup`) repeating an earlier call's arguments and result exactly. After 3 such rounds it SHALL tell the model to stop gathering and produce its output. After 5, a phase that cannot write files SHALL be sent its next and later requests with no tools, together with a request for the final artifact; a phase that can write files SHALL fail after 8 with a message naming the stall. Any other tool result, or a write, SHALL reset the count.

#### Scenario: Nudged after three empty rounds

- **WHEN** an agent's tool calls return nothing new three rounds in a row
- **THEN** the next request carries a user message telling it to stop gathering and produce its output.

#### Scenario: Read-only phase forced to answer

- **WHEN** a phase without write access stalls for five rounds
- **THEN** its next request offers no tools and asks for the final artifact, and the reply becomes the phase artifact.

#### Scenario: Tool call written as text while tools are off

- **WHEN** a phase whose tools were switched off replies with a tool call written as text (JSON or Qwen-style XML)
- **THEN** the reply is not taken as the artifact; the model is told the call was not run and asked for the artifact, and three such replies fail the phase.

#### Scenario: Write-capable phase aborted

- **WHEN** a phase with write access stalls for eight rounds
- **THEN** the phase fails with a stall message.

#### Scenario: New information resets

- **WHEN** a stalled round is followed by a round that reads a file not read before
- **THEN** the stall count starts again from zero.

#### Scenario: Identical repeat

- **WHEN** an agent repeats a `list_files` or `search_code` call with the same arguments and gets the same result
- **THEN** that call counts as bringing nothing new.

### Requirement: Exploration that finds nothing new

The agent loop SHALL also count consecutive tool rounds in which nothing was written and every call was a file exploration (`read_file`, `list_files`, `search_code`) that neither read new content nor mentioned a path the phase had not already read, listed or seen in a search hit. After 6 such rounds it SHALL tell the model the repository has nothing more to show and to produce its output; after 10, a phase that cannot write files SHALL be sent its later requests with no tools, as for the stall guard. Any other tool call, a new path, or a write SHALL reset the count.

#### Scenario: Searching files already read

- **WHEN** a read-only phase has read every file a search can find and spends ten rounds listing and searching them with varied arguments
- **THEN** it is nudged after six rounds and its eleventh request offers no tools.

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

### Requirement: Vague search queries

The agent SHALL refuse a `lookup` or `web_search` call whose query has fewer than three letters or digits, without making a request, returning an error that asks for a descriptive query. A refused query SHALL count as a round that brought nothing new for the stall guard.

#### Scenario: Alphabet walk

- **WHEN** a read-only phase issues lookups for "x", "q", "p", "o", "n" in consecutive rounds
- **THEN** each is refused, the model is nudged after the third, and tools are switched off after the fifth.

#### Scenario: Absolute form of a repo path

- **WHEN** a file tool is given "/src/a.rs" and "src/a.rs" exists in the repository
- **THEN** the call is still refused and the error names "src/a.rs" as the path to use.

