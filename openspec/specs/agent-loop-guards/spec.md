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

### Requirement: Policy refusals are stalls

A tool call refused by policy before it ran (command allowlist, safety block, shell network block, path outside the repository, tool not enabled for the phase) SHALL count as a round that brought nothing new for the stall guard. An allowlist refusal of a common file command SHALL name the file tool that does the same job.

#### Scenario: Refused commands abort a write-capable phase

- **WHEN** a write-capable phase issues a different refused shell command every round
- **THEN** the stall guard aborts it as stalled rather than letting it run to the round cap.

#### Scenario: Refused ls

- **WHEN** `ls -la` is refused by the allowlist
- **THEN** the error names the `list_files` tool.

### Requirement: Forced reply restarts without tool history

When a phase whose tools were switched off answers with a tool call written as text, the next request SHALL contain no tool-call history and declare no tools: a single user message carrying the task, the phase's unique non-error tool results so far (bounded in size), and the instruction to reply with the artifact.

#### Scenario: Tool call after tools are off

- **WHEN** a read-only phase, forced to reply, answers with a tool call written as text
- **THEN** the next request declares no tools and its only message contains the task, the earlier tool results once each, and the instruction to write the artifact.

### Requirement: Round cap ends with a forced reply

When a phase has 6 or more rounds in its budget and only 3 remain, the agent loop SHALL switch tools off and ask the model for its final artifact, as for a stalled read-only phase. It SHALL do this for phases that can write files too. A phase with a smaller budget SHALL fail at the cap as before.

#### Scenario: Review reaches its cap

- **WHEN** a phase with an 8-round budget makes progress in each of its first 5 rounds
- **THEN** its sixth request offers no tools and asks for the artifact, and the reply becomes the phase artifact.

### Requirement: Rephrased searches are stalls

A `web_search` or `lookup` call whose query shares at least half of its words (Jaccard over lowercased words) with an earlier query in the same phase SHALL count as a call that brought nothing new for the stall guard. The call SHALL still be made.

#### Scenario: Reworded lookups

- **WHEN** a plan phase looks up "Quartz cron L last day of month semantics" and then "quartz cron L last day of month" and "cron L last day of month meaning" in separate rounds
- **THEN** the later two count toward the stall guard even though their results differ.

### Requirement: Verify-only passes

A write-capable phase whose artifact lists `files_written` without having written any file SHALL fail only when the repository's working tree has no uncommitted changes either.

#### Scenario: Implement after a review loop-back

- **WHEN** implement runs again over files it already changed, only re-runs tests, and reports those files
- **THEN** its artifact is accepted.

