# eval-harness Specification

## Purpose
TBD - created by archiving change eval-harness. Update Purpose after archive.
## Requirements
### Requirement: Context feature-flag configuration

The system SHALL provide a `context` configuration block with independent boolean
flags for each planned context-delivery feature (code search, range reads, slim
tree, repo map, context budgeting, semantic retrieval), each defaulting to the
pre-change baseline (off), and SHALL provide a named-configuration parser that
maps a configuration name to a set of these flags.

#### Scenario: Baseline name yields all flags off

- **WHEN** the configuration named `baseline` is resolved
- **THEN** every context feature flag is off, matching pre-change behavior.

#### Scenario: Named configuration enables only its features

- **WHEN** a configuration name selecting one or more features (e.g. `+search+map`) is resolved
- **THEN** exactly those features' flags are on and all others are off.

#### Scenario: Unknown configuration token is rejected

- **WHEN** a configuration name contains an unrecognized token
- **THEN** resolution fails with an error rather than silently ignoring it.

### Requirement: Fixture suites and size tiers

The system SHALL let each eval fixture declare a suite (`coder` or `reasoner`) and
a size tier (`small`, `medium`, or `large`), SHALL reject fixtures with an unknown
suite or size, and SHALL allow the eval commands to filter fixtures by suite and
by size.

#### Scenario: Coder fixture requires a verifier

- **WHEN** a fixture declares `suite: coder`
- **THEN** it SHALL define a shell `verify` command, and loading a coder fixture without one fails.

#### Scenario: Reasoner fixture requires a rubric

- **WHEN** a fixture declares `suite: reasoner`
- **THEN** it SHALL define a `rubric`, and loading a reasoner fixture without one fails.

#### Scenario: Filtering by suite and size

- **WHEN** the harness is run with a suite and/or size filter
- **THEN** only fixtures matching the filter are evaluated.

### Requirement: Evaluation granularity

The system SHALL let each fixture declare an evaluation granularity — `flow` (the
whole pipeline), `phase` (a single agent phase), or `turn` (a single model turn:
one prompt → one completion, no tool loop) — SHALL validate a `phase` fixture's
template resolves to exactly one agent phase, and SHALL treat granularity as part
of the report grouping so results at different granularities are never aggregated
together.

#### Scenario: Phase granularity requires a single agent phase

- **WHEN** a fixture declares `granularity: phase` but its template resolves to more than one agent phase
- **THEN** the fixture is rejected with an error rather than run.

#### Scenario: Turn runs one completion and auto-grades a coder turn

- **WHEN** a `granularity: turn` coder fixture is run
- **THEN** the harness sends a single completion request (no tool loop, no repo) and marks success by whether the completion contains all of the fixture's `expect` substrings.

#### Scenario: Turn coder requires expectations

- **WHEN** a `granularity: turn` coder fixture defines no `expect` substrings
- **THEN** it is rejected (a turn has no repo to shell-verify).

#### Scenario: Granularities are not mixed in the report

- **WHEN** the store contains trials at more than one granularity
- **THEN** the report groups by granularity so `flow` and `phase` results are never combined into one row.

#### Scenario: Filtering by granularity

- **WHEN** the harness is run or reported with a granularity filter
- **THEN** only trials at that granularity are evaluated or shown.

### Requirement: Model × configuration evaluation matrix

The system SHALL provide `flows eval run` that evaluates a set of models against a
set of context configurations over the selected fixtures for a number of trials,
where each model is a profile name from the `models:` map or a literal model
string, forcing every phase of a run to the selected model. It SHALL preflight
each model before running and report models that fail preflight without aborting
the others.

#### Scenario: Each model evaluated under each configuration

- **WHEN** `eval run` is given multiple models and multiple configurations
- **THEN** it evaluates every (model, configuration, fixture, trial) cell and records a result for each.

#### Scenario: Model resolved from a profile or literal

- **WHEN** a model token names a profile in the `models:` map
- **THEN** that profile's model/endpoint/key/thinking is used; otherwise the token is treated as a literal model on the default endpoint.

#### Scenario: A model failing preflight is skipped

- **WHEN** a selected model fails preflight (e.g. no endpoint/key)
- **THEN** that model is reported as skipped and the remaining models are still evaluated.

### Requirement: Pairing models to suites

The system SHALL let a model be restricted to a set of fixture suites so it only
evaluates those fixtures. A model profile MAY declare its suites (a `suites` tag);
a per-run `model:suite` selector SHALL override the tag; a model with neither runs
all suites.

#### Scenario: Profile suite tag auto-pairs

- **WHEN** models are run whose profiles declare suites (e.g. a coder tagged `coder`, a reasoner tagged `reasoner`)
- **THEN** each model evaluates only its tagged suites' fixtures, not the full cross-product.

#### Scenario: Per-run selector overrides the tag

- **WHEN** a model is selected as `model:suite`
- **THEN** that model is restricted to that suite for the run regardless of its profile tag.

### Requirement: Store maintenance

The system SHALL provide `flows eval clean` to delete stored results — all of them
when unfiltered, or the subset matching a model / fixture / suite / granularity
filter — removing both the trial records and their saved outputs, and SHALL
confirm before deleting unless told to skip confirmation.

#### Scenario: Filtered deletion

- **WHEN** `eval clean` is run with a filter
- **THEN** only trials matching the filter are removed from the store (records and saved outputs) and the rest are kept.

#### Scenario: Unfiltered wipe

- **WHEN** `eval clean` is run with no filter and confirmation
- **THEN** the entire results store is deleted.

### Requirement: Suite-driven success signal

The system SHALL determine each trial's success from its suite: a `coder` trial's
success is the shell verifier outcome; a `reasoner` trial's success is its manual
grade. A reasoner trial without a manual grade SHALL be reported as ungraded rather
than counted as pass or fail.

#### Scenario: Coder success from the verifier

- **WHEN** a coder trial completes
- **THEN** its success is the exit status of the fixture's `verify` command.

#### Scenario: Reasoner success from the manual grade

- **WHEN** a reasoner trial has a recorded manual grade
- **THEN** its success is that grade; if no grade exists it is counted as ungraded.

### Requirement: Persistent results store

The system SHALL persist every trial to an append-only results store, including its
model, configuration, fixture, suite, size, verifier outcome, efficiency metrics
(tokens, peak context size, cost, tool-rounds, duration), and any manual grade, and
SHALL save each trial's produced output (diff / artifacts) for later review. A
manual grade recorded later SHALL update the trial's result without rewriting prior
records, with readers taking the most recent record per trial.

#### Scenario: Trial recorded on completion

- **WHEN** a trial finishes
- **THEN** a record with its model, configuration, fixture, suite, size, verifier outcome, and efficiency metrics is appended to the store and its output is saved.

#### Scenario: Turn trial saves a reviewable transcript

- **WHEN** a `granularity: turn` trial finishes
- **THEN** the trial's saved output includes the exact input context (system + prompt) and the model's response, so the input and response can be reviewed offline.

#### Scenario: Phase/flow trial captures the assembled prompt

- **WHEN** a `granularity: phase` or `flow` trial runs
- **THEN** the agent loop's assembled prompt for each round (system, tools, input messages, and response) is saved to the trial so the context the model actually received can be reviewed offline.

#### Scenario: Later grade overlays the trial

- **WHEN** a manual grade is recorded for an existing trial
- **THEN** reading the store reflects that grade for the trial while preserving the earlier metric record.

### Requirement: Manual validation

The system SHALL provide `flows eval review` that presents each ungraded trial's
task, rubric, and saved transcript (input context + response) and records a manual
rating across five dimensions — correctness, efficiency, tool use, grounding, and
instruction-following — each on a 1–5 scale, deriving the trial's pass/fail from
the correctness rating. Every dimension SHALL be required (no skipping); tool use
SHALL be recorded as not-applicable for a `turn` trial (which uses no tools).

#### Scenario: Rating an ungraded trial

- **WHEN** `eval review` is run and an ungraded trial exists
- **THEN** it shows the trial's task, rubric, and transcript and requires a 1–5 rating for each applicable dimension, recording the ratings and a pass/fail derived from correctness.

#### Scenario: Tool-use not applicable for a turn

- **WHEN** a `granularity: turn` trial is reviewed
- **THEN** the tool-use dimension is recorded as not-applicable and the remaining dimensions are still required.

### Requirement: Comparison report

The system SHALL provide `flows eval report` that reads the results store without
invoking any model and prints a table grouped by model, configuration, suite, and
size, showing success rate and efficiency metrics aggregated across trials with a
measure of spread, and SHALL support filtering the report by model and by
configuration.

#### Scenario: Grouped comparison across models and configurations

- **WHEN** `eval report` is run after trials for multiple models and configurations
- **THEN** it prints per-(model, configuration, suite, size) success and efficiency metrics with mean and spread, making no model request.

#### Scenario: Repeated trials aggregated with spread

- **WHEN** a cell has more than one trial
- **THEN** the report aggregates them with a mean and a spread so model non-determinism is visible.

