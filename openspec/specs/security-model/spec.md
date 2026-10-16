# Security Model Specification

## Purpose

`flows` runs LLM-driven agents that execute shell commands and touch files in a
user's repository. These requirements capture the sandboxing and input-guard
invariants that keep an agent (or a prompt-injected one) confined. They are the
baseline every future change must preserve; a change that relaxes any of them
must say so explicitly and justify it.
## Requirements
### Requirement: Sandboxed shell execution

The system SHALL execute every agent-invoked shell command (the `bash` and
`plugin` tools) inside a filesystem sandbox — bubblewrap on Linux, Seatbelt
(`sandbox-exec`) on macOS — that permits reads broadly but restricts writes to
the run's repository root, and SHALL run with the host environment cleared so
process secrets are not exposed to the command.

#### Scenario: No sandbox backend available

- **WHEN** no working sandbox backend is present (e.g. bubblewrap missing or
  unprivileged user namespaces are blocked)
- **THEN** the system fails closed — the `bash`/`plugin` tools return an error
  rather than running the command unconfined.

#### Scenario: Write outside the repository

- **WHEN** a sandboxed command attempts to write outside the repository root
- **THEN** the write is denied by the sandbox.

### Requirement: No host IPC from the sandbox

On Linux the system SHALL deny creation of unix (`AF_UNIX`) sockets and
io_uring instances inside the sandbox with a seccomp filter, and deny syscalls
made through a foreign ABI, while still allowing `socketpair()` and inet sockets
so builds, package fetches, and in-process IPC keep working. A read-only
filesystem does not stop a process from connecting to a unix socket, and host
services reachable that way (the systemd/D-Bus session bus, container engines,
the display server, abstract-namespace sockets) can run code outside the
sandbox.

#### Scenario: Session-bus escape attempt

- **WHEN** a sandboxed command tries to reach the user's session bus (e.g. `systemd-run --user …`) or any other unix socket on the host
- **THEN** socket creation fails with a permission error and nothing runs outside the sandbox.

#### Scenario: Builds and IPC still work

- **WHEN** a sandboxed command fetches dependencies over the network or uses `socketpair()`-based IPC
- **THEN** it succeeds as before.

#### Scenario: macOS parity (known gap)

- **WHEN** commands run under the macOS Seatbelt backend
- **THEN** unix sockets and mach-lookup are currently still allowed by the profile; closing this requires a profile change verified on macOS and is tracked on the roadmap.

### Requirement: Repository-scoped file tools

The system SHALL confine the `read_file`, `write_file`, `list_files`, and
`make_dir` tools to paths inside the repository root, rejecting path traversal
(`..`) that escapes the root and refusing to follow symlinks out of it.

#### Scenario: Path traversal escape

- **WHEN** a file tool is called with a path that resolves outside the repo root
  (via `..` or an absolute path)
- **THEN** the tool returns an error and performs no file operation.

#### Scenario: Symlink escape

- **WHEN** a file tool target is a symlink pointing outside the repo root
- **THEN** the tool refuses to follow it (opened with `O_NOFOLLOW`).

### Requirement: Shell network denylist

The system SHALL reject ad-hoc network access from agent shell commands — raw
network clients (`curl`, `wget`, `nc`, `ssh`, `scp`, `rsync`, …) and inline
interpreter evaluation (`python -c`, `node -e`, piping/heredoc into an
interpreter) — on every `bash`-tool call, including `unrestricted` phases, while
still permitting dependency managers (`pip`, `cargo`, `npm`, `go`) to fetch
dependencies.

#### Scenario: Inline network request

- **WHEN** an agent runs `python3 -c "urllib.request.urlopen(...)"` or `curl http://host`
- **THEN** the command is rejected with guidance to use the `http_get` /
  `web_search` tools or a trusted `script` phase.

#### Scenario: Dependency fetch

- **WHEN** an agent runs `pip install <pkg>` or `cargo fetch`
- **THEN** the command is allowed (dependency managers are not on the denylist).

### Requirement: SSRF-guarded HTTP

The system SHALL block requests from the `http_get` and `web_search` tools to
private, loopback, link-local, CGNAT, broadcast, and IPv4-mapped-IPv6 addresses
(including cloud metadata endpoints), and SHALL re-validate the destination on
every redirect hop.

#### Scenario: Request to a blocked address

- **WHEN** `http_get` is asked to fetch a URL that resolves to a private,
  loopback, link-local, CGNAT, or IPv4-mapped-IPv6 address
- **THEN** the request is refused before any connection is made.

#### Scenario: Redirect to a blocked address

- **WHEN** an allowed URL responds with a redirect to a blocked address
- **THEN** the redirect is refused (the guard re-runs on each hop).

### Requirement: Read-only planning phases

The system SHALL withhold the write tools (`write_file`, `make_dir`) from phases
configured read-only (e.g. the `research_ro` preset used by plan / spec / design
phases), and SHALL reject a write tool call in such a phase even if issued.

#### Scenario: Write attempt in a read-only phase

- **WHEN** an agent in a read-only phase calls `write_file` or `make_dir`
- **THEN** the tool is not offered, and any such call is rejected with an
  explanation that the phase is read-only.

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

