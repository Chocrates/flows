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
