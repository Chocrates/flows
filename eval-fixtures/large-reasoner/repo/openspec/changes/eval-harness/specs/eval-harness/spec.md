## ADDED Requirements

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

#### Scenario: Later grade overlays the trial

- **WHEN** a manual grade is recorded for an existing trial
- **THEN** reading the store reflects that grade for the trial while preserving the earlier metric record.

### Requirement: Manual validation

The system SHALL provide `flows eval review` that presents each ungraded trial's
task, rubric, and saved output and records a human pass/fail decision with an
optional score and note into the store.

#### Scenario: Grading an ungraded trial

- **WHEN** `eval review` is run and an ungraded trial exists
- **THEN** it shows the trial's task, rubric, and saved output and records the entered pass/fail (and optional score/note) for that trial.

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
