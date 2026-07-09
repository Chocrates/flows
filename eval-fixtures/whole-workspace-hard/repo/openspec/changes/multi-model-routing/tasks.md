## 1. Config: model profiles

- [x] 1.1 Add a `models: HashMap<String, LLMConfig>`-style map to `GlobalConfig` and `ProjectConfig` in `flows-core` (each profile = model + optional api_base/api_key/thinking); parse + merge (project profiles override global). Round-trip tests.
- [x] 1.2 Add profile resolution: given a selection string + base `llm` + profiles, return the effective `LLMConfig` (profile match → profile merged over base; else literal model on base). Unit tests for profile-match, literal fallback, empty→default.
- [x] 1.3 Add per-profile key resolution: explicit `api_key` (with `${ENV}` expansion) → dialect-default env → empty. Tests.

## 2. Runner: per-phase provider routing

- [x] 2.1 Add a provider cache in `flows-runner` keyed by resolved config (model, api_base, api_key, dialect) that builds via `flows_llm::make_provider` and reuses across phases.
- [x] 2.2 In `execute_phase` (agent branch), resolve the phase's effective `LLMConfig` (via 1.2) and route to its cached provider + pass the effective `llm` to `run_agent_phase`.
- [x] 2.3 Replace `run_pipeline`'s `provider: &dyn Provider` parameter with config-owned provider creation (the cache); update all call sites (`cmd_run`, `run_tui`, worker, `eval`). Verify the single-model path is unchanged.
- [x] 2.4 Ensure `resolve_thinking` honors phase → profile → base precedence. Tests.

## 3. Estimation, usage, visibility

- [x] 3.1 `estimate.rs` / `dryrun`: price each agent phase by its resolved model. Test a mixed-model plan estimate.
- [x] 3.2 `flows plan`: show each agent phase's resolved model in the plan output.
- [x] 3.3 `flows doctor`: list configured profiles + their routing; pre-flight validates every profile a default run would use.
- [x] 3.4 Pre-flight (`preflight_llm`): validate all profiles a run will use (model present + key/endpoint), erroring by profile name for an unusable one.

## 4. Docs, template example, gate

- [x] 4.1 README config section: `models:` profiles + per-phase `model:` selection; note same-endpoint vs different-endpoint.
- [x] 4.2 Add a template example (or `software`/`feature` comment) showing `design: reasoner` / `implement: coder`.
- [x] 4.3 Full gate green (`cargo fmt --check`, `clippy -D warnings`, `cargo test`); `openspec validate multi-model-routing`.
