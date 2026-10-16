## ADDED Requirements

### Requirement: Credential stores hidden from the sandbox

The system SHALL hide a fixed set of credential locations under the invoking user's home directory from every sandboxed command, so the command can neither read nor list them, even by absolute path. The set SHALL cover at least SSH keys, cloud CLI credentials (AWS, Azure, Google Cloud, Kubernetes, Docker), forge CLI tokens (`gh`, `hub`), package-registry credential files (`.netrc`, `.git-credentials`, `.npmrc`, `.pypirc`, cargo credentials), GPG and password stores, desktop keyrings, browser profiles, AI-assistant credential stores, and flows' own `~/.flows/config.yml` and `~/.flows/projects/`. Only locations that exist SHALL be masked, and symlinked locations SHALL be masked at their resolved target. The repository and the toolchain cache SHALL stay accessible even when they lie inside a masked location.

A phase SHALL be able to opt out with `tools.bash.expose_credentials: true` (and a plugin tool with `expose_credentials: true` in its YAML); this SHALL re-expose the credential set but SHALL NOT expose flows' own configuration files.

#### Scenario: SSH key unreadable by default

- **WHEN** a sandboxed command runs `cat ~/.ssh/id_ed25519` (or the same file by absolute path) in a phase without `expose_credentials`
- **THEN** the file is not found, and the directory lists as empty.

#### Scenario: Flows API keys always hidden

- **WHEN** a sandboxed command reads `~/.flows/config.yml`, with or without `expose_credentials: true`
- **THEN** it gets no content.

#### Scenario: Opt-out restores credentials

- **WHEN** a phase sets `tools.bash.expose_credentials: true` and a command reads `~/.ssh/known_hosts`
- **THEN** the real file contents are returned.

#### Scenario: Repository inside a masked location

- **WHEN** the run's repository lies under a masked directory
- **THEN** the repository is still readable and writable from the sandbox.
