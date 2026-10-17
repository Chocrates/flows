## ADDED Requirements

### Requirement: Doctor probes local endpoints

The environment check SHALL, for each distinct custom OpenAI-compatible endpoint used by the default model or a profile, request the endpoint's model list with a timeout of at most 5 seconds and report whether the endpoint answered and whether it lists the configured model. It SHALL NOT probe cloud endpoints, and a failed probe SHALL be reported as a warning without failing the check.

#### Scenario: Server serves the model

- **WHEN** a profile's local endpoint answers and lists its model
- **THEN** doctor reports the endpoint reachable for that profile.

#### Scenario: Wrong model name

- **WHEN** the endpoint answers but does not list the configured model
- **THEN** doctor warns and shows models the endpoint does list.

#### Scenario: Server down

- **WHEN** the endpoint refuses the connection or does not answer in time
- **THEN** doctor warns that it is unreachable and still completes.

#### Scenario: Cloud model

- **WHEN** a profile uses a cloud endpoint
- **THEN** no request is made for it.
