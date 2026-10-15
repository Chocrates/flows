# model-io Specification

## Purpose
Resilient model I/O: retry transient failures with backoff, degrade cleanly for models without tool calling, cap completion tokens per phase, and tolerate common model path mistakes.
## Requirements
### Requirement: Retry transient LLM failures with backoff

The system SHALL retry a language-model request on transient failure — network
errors and retryable HTTP statuses (request-timeout, too-early, rate-limit, and
5xx) — using exponential backoff, and SHALL NOT retry non-transient failures
(4xx auth/bad-request) or a failure that occurs after streamed output has begun.
The retry policy SHALL be configurable via environment variables.

#### Scenario: Transient failure is retried

- **WHEN** a model request fails with a network error or a retryable status and retries remain
- **THEN** the request is retried after an exponentially increasing delay before the failure is surfaced.

#### Scenario: Non-transient failure is not retried

- **WHEN** a model request returns a 4xx auth/bad-request status
- **THEN** it is returned immediately without retrying.

#### Scenario: Streaming is not double-emitted

- **WHEN** a streaming request fails after tokens have already been streamed
- **THEN** it is not retried (so previously emitted tokens are never repeated).

### Requirement: Models without tool support

The system SHALL let a model be marked as not supporting tool/function calling.
For such a model the runner SHALL send no tool schemas and SHALL instruct the
model to return any file writes inline in its artifact (a map of repository path →
full contents), which the runner then writes to disk. A phase that produces no
files (e.g. a plan) SHALL simply emit its artifact.

#### Scenario: No tool schemas sent to a no-tools model

- **WHEN** a phase runs on a model marked no-tools
- **THEN** the request carries no tool definitions.

#### Scenario: Inlined files are written to disk

- **WHEN** a no-tools model returns an artifact containing a map of file paths to contents
- **THEN** the runner writes each file into the repository, and the phase's verifier then runs against them.

### Requirement: Per-phase completion-token cap

The system SHALL allow a phase to set the maximum completion tokens per model
call, defaulting to a value large enough that a reasoning model's output is not
truncated.

#### Scenario: A reasoning phase is not truncated

- **WHEN** a phase configures a large `max_tokens` (or relies on the generous default)
- **THEN** the model's completion is capped at that value rather than a small fixed limit.

### Requirement: Repo-root tolerance for directory listing

The directory-listing file tool SHALL treat an empty path, `"."`, and `"/"` as the
repository root (and a leading `/` on a subpath as repo-relative) instead of
erroring, while the path resolution used by file read/write SHALL still reject an
empty or escaping path.

#### Scenario: Listing the root by any common spelling

- **WHEN** the listing tool is called with `""`, `"."`, or `"/"`
- **THEN** it lists the repository root instead of returning a path error.

#### Scenario: Escapes still refused

- **WHEN** a file read or write is given an empty path or one that escapes the repository root
- **THEN** it is refused.

