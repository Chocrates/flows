## ADDED Requirements

### Requirement: Independent feature gating

The system SHALL gate each context-delivery feature (code search, range reads,
slim tree, repo map, context budgeting, semantic retrieval) behind an independent
configuration flag under a `context` block, each defaulting to the pre-change
baseline behavior, so any single feature can be enabled or disabled in isolation.

#### Scenario: All flags off reproduces the baseline

- **WHEN** every `context.*` flag is at its default (off)
- **THEN** context assembly and the available tools match the pre-change baseline behavior.

#### Scenario: A single feature enabled in isolation

- **WHEN** exactly one `context.*` flag is enabled
- **THEN** only that feature is active and all others remain at baseline.

### Requirement: Per-run configuration override

The system SHALL allow the `context.*` flags to be overridden for a single run
(without editing global or project config), so different configurations can be
compared on the same task.

#### Scenario: Override for one run

- **WHEN** a run is started with a per-run context configuration
- **THEN** that run uses the overridden flags and other runs / global config are unaffected.

### Requirement: Deterministic retrieval evaluation

The system SHALL provide a retrieval evaluation that, given labeled fixtures
mapping a task to the repository files/symbols that are actually relevant,
measures whether the enabled retrieval features (search, repo map, semantic
retrieval) surface those targets — as recall@k / MRR — without invoking the
language model, so retrieval quality is measured deterministically.

#### Scenario: Recall measured without a model call

- **WHEN** the retrieval eval runs over a labeled fixture with a feature enabled
- **THEN** it reports recall@k (and/or MRR) for that feature's ranking against the labeled targets, making no language-model request.

### Requirement: End-to-end configuration evaluation

The system SHALL provide an eval harness (`flows eval`) that runs a corpus of
`(repository, task, verifier)` fixtures under a set of context configurations and
reports, per configuration, task success (verifier outcome) and efficiency
metrics (tokens, peak context size, cost, tool-rounds) drawn from the run
records, so each feature's usefulness can be compared to the baseline.

#### Scenario: Per-configuration report

- **WHEN** the harness runs the corpus under baseline and one or more feature configurations
- **THEN** it emits a per-configuration report of success rate and efficiency metrics suitable for comparing each configuration to the baseline.

#### Scenario: Repeated trials for a noisy metric

- **WHEN** a configuration is evaluated with more than one trial per fixture
- **THEN** the report aggregates trials (e.g. mean and spread) so model non-determinism is visible rather than hidden.
