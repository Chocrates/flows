## Context

`flows-llm` has two providers (Anthropic, OpenAI-compatible), each with a
`complete` and a `complete_streaming` — four HTTP send sites. The agent loop
(`flows-runner/agent.rs`) builds a `CompletionRequest` (system, messages, tools,
max_tokens) per round, streams the response, dispatches tool calls, and finalizes
an artifact. File tools (`flows-tools/files.rs`) are repo-scoped via `safe_path`.

## Decisions

**D1 — Retry the send+status phase only.** `send_with_retry` wraps building and
sending the request and checking the status — *before* the streaming body is read.
This never double-emits streamed tokens: a failure after streaming started is not
retried. Retryable = network errors, `408`/`425`/`429`, `5xx`; not `4xx`
auth/bad-request or parse errors. Backoff is exponential with jitter, capped, and
env-tunable (`FLOWS_LLM_MAX_RETRIES`, `FLOWS_LLM_RETRY_BASE_MS`,
`FLOWS_LLM_RETRY_MAX_MS`). Defaults are tuned for a model-swap proxy: first wait
≥ the proxy's recovery window, several retries spanning a model-load.

**D2 — Tool capability is a model property, expressed positively-negated.**
`LLMConfig.supports_tools: Option<bool>` (`None` = assume capable). `Some(false)`
→ the runner sends no tool schemas and appends a "no tools" instruction telling
the model to return its artifact with any file writes in `content.files`. On the
final artifact, the runner writes those files. Rationale: many reasoning models
can't function-call; forcing tools on them wastes tokens and produces narrated,
never-executed "writes". A no-tools reasoner (plan phase) simply emits its plan
artifact — no files, matching the pipeline's plan→implement contract.

**D3 — Output-token cap is per-phase with a generous default.** `phase.max_tokens`
overrides a default high enough that reasoning models' hidden reasoning + answer
fit. It is a cap, not a target — short answers still cost less — so raising it is
safe.

**D4 — Root-path tolerance in `list_files`, not in `safe_path`.** Only `list_files`
normalizes `""` / `"."` / `"/"` (and strips a leading `/`) to the repo root.
`safe_path` (used by read/write) still rejects an empty or escaping path, so the
security invariant is unchanged — the tolerance is scoped to the one tool where
"list the root" is the natural, safe intent.

## Risks / Trade-offs

- Aggressive retries make a genuinely-down endpoint slow to fail (default ~minutes).
  Mitigation: env override to `0` for fast-fail (used by tests).
- A no-tools model that ignores the `content.files` convention writes nothing; the
  verifier then fails the trial — surfaced honestly rather than silently.

## Migration

Additive and backward compatible. Unset `supports_tools` and `max_tokens` keep
prior behavior; retries are transparent (only affect failing calls).
