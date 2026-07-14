## Why

Different phases want different models: a deep-reasoning model for `design` /
`spec`, a fast coder for `implement`. Today only the model *name* is per-phase
(`phase.model`); the provider — `api_base`, `api_key`, and the API dialect
(Anthropic vs OpenAI, chosen from the global model) — is fixed for the whole run.
So two models on the *same* endpoint already work, but a reasoner and a coder on
*different* endpoints (e.g. a cloud reasoner + a local llama.cpp coder) cannot.

## What Changes

- Add a **`models:` map** to config (global and project): each entry is a named
  profile with its own `model`, optional `api_base`, `api_key`, and `thinking`.
- `llm.model` (the run default) and `phase.model` may reference a **profile name**
  or a **literal model string**. Resolution: profile name → that profile; else a
  literal model on the default endpoint (today's behavior — **backward compatible**).
- The runner builds a **provider per distinct resolved profile** (cached by
  resolved config) and routes each agent phase to its own provider, so phases on
  different endpoints / dialects each reach the right server with the right key.
- Per-phase `thinking` continues to work and can come from the profile.
- **Visibility**: `flows plan` and `flows doctor` show the model each phase routes
  to (and flag an unresolved profile / unreachable endpoint); pre-flight validates
  every profile a run will use, not just the default.
- Cost estimation and `usage` account for the actual per-phase model.

Non-goals: no change to the phase/pipeline model, sandbox, or on-disk run
formats; no per-phase provider for `script`/`human` phases (they make no LLM call).

## Capabilities

### New Capabilities
- `model-profiles`: named model profiles in config and per-phase model selection that resolve to a full provider (model + endpoint + key + thinking), so different phases can run on different models and endpoints.

### Modified Capabilities
<!-- No existing spec's requirements change. security-model invariants are unaffected (providers still go through the guarded HTTP path). -->

## Impact

- **Code**: `flows-core` (config `models` map + profile resolution in `models.rs`/`config.rs`); `flows-runner` (`runner.rs` builds/caches a provider per resolved profile and routes each agent phase; `agent.rs` receives the resolved model/llm); `flows-llm` (`make_provider` reused per profile); `flows-cli` (`doctor`/`plan` show routing, pre-flight each profile); `flows-core` estimate/usage account per-phase model.
- **Config**: additive `models:` map; `llm.model` / `phase.model` gain profile-name semantics with literal fallback. No format break.
- **Security**: each profile's requests go through the existing SSRF-guarded HTTP path; per-profile keys are read the same way as the default (config / env). `security-model` invariants preserved.
- **Docs**: README config section (model profiles + per-phase routing) and a template example.
