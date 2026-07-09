## Context

`PhaseConfig` already has `model: Option<String>` and `thinking: Option<bool>`,
and `agent.rs` sends `phase.model` (falling back to `llm.model`) in the request.
But the **provider** is built once by the caller from the global `llm` config
(`make_provider(&llm)`) and threaded through `run_pipeline(provider: &dyn
Provider)` — so `api_base`, `api_key`, and the Anthropic-vs-OpenAI dialect are
fixed for the whole run. Two models on one endpoint work; a reasoner and a coder
on different endpoints do not. Provider creation (`flows_llm::make_provider`)
picks the dialect from the model name and reads keys from config/env.

## Goals / Non-Goals

**Goals:**
- Per-phase model *and endpoint*, so `design` can use a cloud reasoner while `implement` uses a local coder.
- Define each model once (DRY) and reference it by name from phases.
- Backward compatible: existing bare-model configs keep working.
- Correct per-phase cost, plan display, and pre-flight.

**Non-Goals:**
- Per-phase providers for `script`/`human` phases (no LLM call).
- Changing the pipeline model or on-disk formats.
- Automatic model selection — the user maps phases to models explicitly.

## Decisions

**D1 — Named profiles, not a per-phase `llm:` block.** Add `models: { <name>:
{model, api_base?, api_key?, thinking?} }` to config; phases reference by name.
Rationale: endpoint/key defined once, readable templates. Alternative (a full
`llm` block on each phase) repeats endpoint config and is noisier — rejected.

**D2 — Resolution: name → profile → literal.** A model selection (`llm.model` or
`phase.model`) first matches a profile name; otherwise it is a literal model on
the default endpoint (the run's base `llm`). This preserves today's behavior for
bare names. Empty/absent on a phase → the run default.

**D3 — Provider cache keyed by resolved config.** The runner resolves each agent
phase to an effective `LLMConfig` (model + api_base + api_key + thinking) and
builds a provider per distinct key via `flows_llm::make_provider`, cached in a
`HashMap` so phases sharing a config reuse one provider. The dialect is chosen per
provider from its own model name (existing `make_provider` behavior), so a
`claude*` profile speaks Anthropic and a local `qwen` profile speaks OpenAI.

**D4 — Provider creation moves into the runner.** `run_pipeline` takes the
resolved `global`/`project` config (it already does) and owns the provider cache,
replacing the single `provider: &dyn Provider` parameter. Callers (`cmd_run`,
`run_tui`, worker, `eval`) stop building/passing a provider. Rationale: per-phase
routing requires provider creation inside the loop anyway; centralizing it removes
duplicated setup. The default provider is just the cache entry for `llm.model`.

**D5 — Keys per profile.** A profile's `api_key` is used if set (with `${ENV}`
expansion, as today); else the dialect default env (`ANTHROPIC_API_KEY` for
`claude*`, else `OPENAI_API_KEY`); else empty (local endpoint, no key). This lets
two cloud providers carry different keys.

**D6 — Thinking + cost + visibility follow the resolved model.** `resolve_thinking`
takes phase → profile → base precedence. Cost estimation (`estimate.rs`/`dryrun`)
and `usage` use the per-phase resolved model for pricing. `plan` and `doctor` list
each agent phase's resolved model and validate every profile a run will touch
(pre-flight each, not just the default).

## Risks / Trade-offs

- **Provider proliferation** → cache by resolved config; a single-model run holds exactly one provider (unchanged cost).
- **Mixed-provider cost estimate** → each phase priced by its own model via existing `lookup_pricing`; unknown local models price at 0 (as today).
- **Key confusion across providers** → explicit per-profile `api_key` wins; dialect-default env only as fallback; pre-flight reports a missing key by profile name.
- **Signature change ripples to all callers** → mechanical; covered by the existing CLI/TUI/eval call sites and tests.

## Migration Plan

Additive and backward compatible: no `models:` map ⇒ one profile (the base
`llm`), identical to today; a bare `phase.model` ⇒ literal on the default
endpoint. Ship config + resolution first, then runner provider-cache + routing,
then estimate/plan/doctor visibility, then docs + a template example.

## Open Questions

- None blocking. (Per-profile request timeouts/retries can reuse the global HTTP
  settings for v1.)
