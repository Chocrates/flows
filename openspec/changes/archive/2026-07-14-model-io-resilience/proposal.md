## Why

Running against real local inference servers surfaced gaps in how flows talks to
models. Model-swapping proxies drop connections mid-load (transient 502 /
connection-refused) and fail a whole phase. Many reasoning models don't do
function calling at all — they emit prose, never a tool call — so any agent phase
flails. Those same models emit thousands of tokens of hidden reasoning and get
truncated under a fixed 4k output cap, coming back blank. And models spell "the
repo root" inconsistently (`""`, `"."`, `"/"`), which the file tools rejected as
an escape, burning a round on an error.

## What Changes

- **Retry with exponential backoff** on every LLM HTTP call (both providers,
  streaming and not): transient network errors, `408`/`425`/`429`, and `5xx` are
  retried before any streamed token is read, so a swapping backend gets time to
  come up. Env-tunable; not applied to `4xx` auth/bad-request.
- **Model tool-capability flag** `supports_tools: false` on a profile: the runner
  sends no tool schemas to that model and instead instructs it to inline any file
  writes in its artifact (`content.files` = path → contents), which the runner
  writes to disk. A reasoner just emits its plan artifact (no writes).
- **Per-phase completion-token cap** (`phase.max_tokens`) with a generous default,
  so reasoning models aren't truncated mid-reasoning.
- **Repo-root argument tolerance** in `list_files`: `""`, `"."`, and `"/"` all mean
  the repository root (and a leading `/` on a subpath is treated as repo-relative),
  instead of erroring.

Non-goals: changing the provider dialects, the sandbox, or on-disk formats. No
retry of non-idempotent streamed output after tokens have been emitted.

## Capabilities

### New Capabilities
- `model-io`: resilient, capability-aware model I/O — retry/backoff on transient
  failures, a no-tools execution path for models that can't function-call, a
  per-phase output-token cap, and tolerant repo-root tool arguments.

### Modified Capabilities
<!-- No existing spec's requirements change. Security invariants preserved: file
tools stay repo-scoped (the root-path tolerance still refuses real escapes); the
HTTP layer's SSRF guard is unchanged. -->

## Impact

- **Code**: `flows-llm` (retry module + all four send sites); `flows-core`
  (`LLMConfig.supports_tools`, `PhaseConfig.max_tokens`); `flows-runner`
  (`agent.rs` no-tools prompt + `content.files` write-back, token cap);
  `flows-tools` (`files::list_files` root normalization).
- **Config**: `models.<name>.supports_tools`, `phases[].max_tokens`.
- **On-disk**: none.
