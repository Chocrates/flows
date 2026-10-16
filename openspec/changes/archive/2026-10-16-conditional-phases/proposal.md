## Why

Pipelines can loop back and continue on failure, but they can't decide to skip a phase. Common shapes need it: run `fix` only when `review` rejected, run `migrate` only when `plan` says a schema change is needed, skip the human gate when an agent reports nothing risky. Today that takes a wasted LLM call that says "nothing to do", or a separate template.

## What Changes

- New optional phase field `when:`, a MiniJinja boolean expression (same dialect as `loop_condition`). The phase runs only when it evaluates true.
- Expression context: `artifacts.<phase>` (that phase's artifact content) and `idea`. A missing phase or field is undefined, which is falsy.
- A skipped phase records an artifact with content `{"skipped": true, "reason": …}`, counts as completed (so DAG dependents still run, and can test `artifacts.X.skipped`), and fires a new `on_phase_skipped` callback. TUI, foreground output and attach show it as skipped (`⊘`).
- Pipeline validation (`flows plan`, run start) rejects a `when:` that does not parse. A runtime evaluation error fails the run instead of silently running or skipping the phase.
- `flows plan` shows each phase's condition.

## Capabilities

### New Capabilities
- `conditional-phases`: per-phase `when:` conditions, skip semantics in linear and DAG pipelines, validation and error behaviour.

### Modified Capabilities
<!-- none -->

## Impact

- `flows-core`: `PhaseConfig.when`, new `condition` module (compile + evaluate; minijinja moves into core), validation, `RunState.skipped_phases`, plan lines.
- `flows-runner`: linear and DAG loops evaluate `when` before running a phase; `RunnerCallbacks::on_phase_skipped`.
- `flows-tui` / `flows-cli`: skipped status and glyph.
- Docs: template reference.
