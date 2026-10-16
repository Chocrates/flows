## ADDED Requirements

### Requirement: Resource-limited shell execution

The system SHALL bound the memory and process count of every sandboxed shell
command (the `bash` and `plugin` tools). On Linux with a systemd user session
that delegates the memory and pids controllers, the command tree SHALL run in a
transient cgroup scope capped at the configured memory (default 8192 MB, swap
disallowed) and task count (default 1024), overridable per phase via
`tools.bash.memory_mb` and `tools.bash.max_procs` (`0` = unlimited). When no
such scope is available, the system SHALL apply a per-process memory rlimit
where the platform enforces one, SHALL still run the command (filesystem
confinement remains fail-closed), and SHALL tell the user that limits are
weaker or absent.

#### Scenario: Memory cap enforced

- **WHEN** a sandboxed command on a cgroup-capable Linux host allocates more than the phase's memory limit
- **THEN** the command is killed and the tool result reports that it was killed.

#### Scenario: Fork bomb contained

- **WHEN** a sandboxed command spawns processes beyond the task limit
- **THEN** further process creation fails inside the command and the host is unaffected.

#### Scenario: Per-phase override

- **WHEN** a phase sets `tools.bash.memory_mb` or `tools.bash.max_procs`
- **THEN** that phase's commands use those limits, and `0` disables that limit.

#### Scenario: No cgroup scope available

- **WHEN** the host has no usable systemd user scope
- **THEN** commands still run, with a per-process memory rlimit where supported, and the phase notes and `flows doctor` report the weaker limit mode.
