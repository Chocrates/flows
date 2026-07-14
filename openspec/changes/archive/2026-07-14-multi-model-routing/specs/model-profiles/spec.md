## ADDED Requirements

### Requirement: Named model profiles

The system SHALL support a `models` map in configuration where each entry is a
named profile carrying its own `model` and optional `api_base`, `api_key`, and
`thinking`. Profiles defined in project config SHALL override same-named profiles
in global config.

#### Scenario: Profile resolves to its provider settings

- **WHEN** a profile `reasoner` is defined with a model and endpoint and is referenced by a run or phase
- **THEN** requests for that profile use the profile's model, endpoint, key, and thinking settings.

#### Scenario: Project overrides global profile

- **WHEN** global and project config both define a profile of the same name
- **THEN** the project definition is used.

### Requirement: Per-phase model selection

The system SHALL allow the run default (`llm.model`) and any phase (`phase.model`)
to select a model, and SHALL route each agent phase's LLM calls to the provider
resolved for that selection. Phases that do not specify a model SHALL use the run
default.

#### Scenario: Phase uses its own model

- **WHEN** the `design` phase selects the `reasoner` profile and `implement` selects `coder`
- **THEN** `design`'s calls go to the reasoner's provider and `implement`'s to the coder's.

#### Scenario: Phase without a model uses the default

- **WHEN** a phase specifies no model
- **THEN** it uses the run's default model/profile.

### Requirement: Literal model backward compatibility

The system SHALL treat a model selection that does not match any profile name as a
literal model string on the default endpoint, so existing configs and templates
that set a bare model name continue to work unchanged.

#### Scenario: Bare model name still works

- **WHEN** `phase.model` is a literal model name that is not a defined profile
- **THEN** that model is used on the default endpoint/key (the prior behavior), with no error.

### Requirement: Per-phase provider isolation

The system SHALL build a distinct provider for each distinct resolved profile
(model + endpoint + key + dialect) used by a run, reusing one provider across
phases that resolve to the same configuration, so phases on different endpoints or
API dialects each reach the correct server with the correct key.

#### Scenario: Two endpoints in one run

- **WHEN** a run uses a cloud reasoner for one phase and a local endpoint for another
- **THEN** each phase's request reaches its own endpoint with its own key and correct API dialect.

#### Scenario: Shared provider reused

- **WHEN** multiple phases resolve to the same model + endpoint
- **THEN** a single provider is reused for them rather than rebuilt per phase.

### Requirement: Routing visibility and validation

The system SHALL show, in the execution plan and environment check, the model each
agent phase will use, and SHALL validate every profile a run will use before the
run starts (each has a model and a reachable/authorized endpoint), reporting a
clear error for an unresolved profile or a missing key/endpoint.

#### Scenario: Plan shows per-phase models

- **WHEN** a user previews the plan for a template with per-phase models
- **THEN** each agent phase lists the model it will use.

#### Scenario: Pre-flight catches an unusable profile

- **WHEN** a run will use a profile that has no model or no reachable endpoint/key
- **THEN** the run is refused before starting with an error naming the offending profile.
