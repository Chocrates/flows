## ADDED Requirements

### Requirement: Opt-in network isolation

A phase SHALL be able to run its sandboxed shell commands without network access by setting `tools.bash.offline: true` (a plugin tool by `offline: true` in its YAML). Offline commands SHALL be able to use loopback networking among their own processes, but SHALL NOT reach any host address, including services listening on the host's loopback interface, and SHALL NOT be able to resolve names over DNS on Linux. When offline, the `bash` tool's description SHALL tell the model that commands have no network access. Network access SHALL remain enabled by default.

#### Scenario: Host unreachable when offline

- **WHEN** an offline phase's command connects to a service the host is listening on at 127.0.0.1
- **THEN** the connection fails.

#### Scenario: Loopback inside the sandbox works

- **WHEN** an offline command starts a server on 127.0.0.1 and connects to it
- **THEN** the connection succeeds.

#### Scenario: Default keeps network

- **WHEN** a phase does not set `offline`
- **THEN** commands can reach the network as before.
