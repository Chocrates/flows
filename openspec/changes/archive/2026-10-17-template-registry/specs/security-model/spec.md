## ADDED Requirements

### Requirement: Untrusted installed templates are confined

Phases defined by a template installed through the registry and not marked trusted SHALL be confined: their script phases SHALL run inside the shell sandbox (writes limited to the repository, credential stores hidden, host environment cleared) and fail closed when no sandbox is available, and their agent phases SHALL NOT be able to expose credential stores to shell commands. This SHALL hold wherever those phases appear, including through `include:`. Trusting the template SHALL lift both restrictions. Templates from the project, the user's own global templates, and builtins SHALL be unaffected. A project template with the same name SHALL take precedence over an installed one and is not confined.

#### Scenario: Installed script tries to write outside the repo

- **WHEN** an untrusted installed template's script phase runs `touch ~/pwned`
- **THEN** the write fails and the file does not exist on the host.

#### Scenario: Installed template asks for credentials

- **WHEN** an untrusted installed template's agent phase sets `tools.bash.expose_credentials: true`
- **THEN** its shell commands still cannot read the credential stores.

#### Scenario: Trusted template

- **WHEN** the user has trusted an installed template
- **THEN** its script phases run on the host like local ones.
