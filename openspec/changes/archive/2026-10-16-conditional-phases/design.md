## Context

Phases already carry one expression, `loop_condition`, evaluated in `flows-runner` with MiniJinja + `pycompat` against the phase's own content, where any error counts as false. Conditions need to be checked at validation time too, and validation lives in `flows-core` (`pipeline::validate_pipeline`, which both `flows plan` and the runner call).

## Goals / Non-Goals

**Goals:** run-if conditions on any phase type, in linear and DAG pipelines; skip visible in every UI; bad conditions caught early.

**Non-Goals:** else-branches or phase-level `goto`; conditions on `depends_on` edges; changing `loop_condition` semantics (its error → false behaviour stays).

## Decisions

- **`when:` run-if, not `skip_if:`.** Reads like GitHub Actions `if:`; chosen by the user.
- **Evaluator in `flows-core::condition`.** `minijinja` (+ `minijinja-contrib` pycompat) becomes a core dependency so validation can call `Environment::compile_expression`. The runner's `tuples_to_lists` rewrite moves there too and `eval_loop_condition` reuses it. Expressions are compiled with `UndefinedBehavior::Chainable`, so `artifacts.missing.field` is undefined (falsy) instead of an error; truthiness is MiniJinja's `Value::is_true`.
- **Context = `artifacts` (content only) + `idea`.** Content is what templates and loop conditions already reason about; exposing the envelope (version, run_id) adds noise.
- **Skip = completed.** Chosen by the user over cascade-skip. Dependents decide for themselves via `artifacts.X.skipped`. A skipped phase still writes an artifact so prompts and downstream conditions see a value and attach/resume see a record.
- **Errors fail the run.** Chosen by the user. Syntax errors surface at validation; evaluation errors (rare once syntax is checked, e.g. unknown filters/functions) stop the run.
- **New callback `on_phase_skipped(phase, reason)`**, default no-op, fired instead of `on_phase_start`/`on_phase_end` so UIs don't flash "running". `RunState.skipped_phases` (serde default) lets `attach` render the skip from state alone.
- **DAG scheduling.** Conditions are evaluated when a phase becomes ready (all deps recorded). A skip changes readiness without a future completing, so the scheduler loops again before awaiting in-flight work.
- **Cost estimate unchanged.** `flows plan` estimates every phase (an upper bound) and prints the condition next to it.

## Risks / Trade-offs

- Adding minijinja to `flows-core` increases its dependency footprint slightly; it's already in the workspace via runner/tools.
- A typo in a field name (`artifacts.review.verdikt`) is undefined, not an error, so `!=` comparisons turn true. Same as Jinja everywhere; documented.
