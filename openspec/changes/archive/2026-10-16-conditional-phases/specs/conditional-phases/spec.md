## ADDED Requirements

### Requirement: Phase `when` condition

A phase SHALL accept an optional `when` field holding a boolean MiniJinja expression, in the same dialect as `loop_condition` (Python-style `.get()` / `.strip()` and tuple literals after `in` are accepted). The expression SHALL be evaluated immediately before the phase would run, against a context containing `artifacts` (a map from each already-recorded phase name to that phase's artifact content) and `idea`. The phase SHALL run only when the result is truthy. Referencing a phase or field that does not exist SHALL yield an undefined value, which is falsy, rather than an error. A phase without `when` SHALL always run.

#### Scenario: Condition true runs the phase

- **WHEN** a phase has `when: "artifacts.review.verdict != 'approved'"` and the `review` artifact's verdict is `rejected`
- **THEN** the phase runs normally.

#### Scenario: Condition false skips the phase

- **WHEN** the same phase runs after a `review` artifact whose verdict is `approved`
- **THEN** the phase is not executed (no LLM call, script, or human gate) and the pipeline moves on.

#### Scenario: Missing data is falsy

- **WHEN** a `when` expression references a phase that has not produced an artifact, e.g. `artifacts.missing.flag`
- **THEN** the reference evaluates as undefined and, used alone, the phase is skipped without an error.

#### Scenario: Re-evaluated on every pass

- **WHEN** a phase with `when` sits inside a `loop_back_to` range and the loop re-runs it
- **THEN** its condition is evaluated again against the artifacts current at that point.

### Requirement: Skipped phase recording

A skipped phase SHALL record an artifact whose content is `{"skipped": true, "reason": <text naming the condition>}`, SHALL be counted as completed for scheduling, SHALL be listed in the run state's `skipped_phases`, and SHALL be reported through a `RunnerCallbacks::on_phase_skipped` event. User interfaces SHALL show a skipped phase distinctly from a completed one. In a DAG pipeline, phases depending on a skipped phase SHALL still become ready; they can test `artifacts.<phase>.skipped` in their own `when`.

#### Scenario: Skipped artifact recorded

- **WHEN** a phase is skipped
- **THEN** its artifact file exists with `skipped: true`, it appears in both `completed_phases` and `skipped_phases`, and `on_phase_skipped` fires instead of `on_phase_start`.

#### Scenario: DAG dependents still run

- **WHEN** in a DAG pipeline phase `b` depends on phase `a` and `a` is skipped
- **THEN** `b` is scheduled and runs.

### Requirement: Condition errors fail loudly

Pipeline validation SHALL reject a `when` expression that does not parse, naming the phase, so `flows plan` and run start report it before any phase executes. An expression that parses but fails during evaluation SHALL fail the run with an error naming the phase and the condition; the phase SHALL be neither run nor skipped.

#### Scenario: Syntax error caught at validation

- **WHEN** a pipeline contains a phase with `when: "artifacts.a.x =="`
- **THEN** validation fails with an error naming that phase, and no phase runs.

#### Scenario: Runtime error fails the run

- **WHEN** a `when` expression parses but raises at evaluation (e.g. calling an unknown function)
- **THEN** the run fails at that phase with an error that quotes the condition.
