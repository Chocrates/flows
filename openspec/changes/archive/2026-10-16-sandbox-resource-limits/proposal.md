## Why

Sandboxed commands are confined on the filesystem but not on resources. A
runaway build, a memory leak in a test, or a fork bomb from a confused (or
prompt-injected) agent can take the whole machine down. The pid namespace
isolates processes but does not limit them, and nothing caps memory.

## What Changes

- On Linux, every sandboxed command (bash + plugin tools) runs inside a
  transient systemd user scope (`systemd-run --user --scope`) with
  `MemoryMax`, `MemorySwapMax=0`, and `TasksMax`. That caps the whole command
  tree, wrapped around the existing bwrap sandbox.
- Defaults: **8 GB memory, 1024 tasks** per command. Per-phase overrides are
  `tools.bash.memory_mb` / `tools.bash.max_procs`; `0` means no limit.
- Fallback when no systemd user session can enforce limits (e.g. CI
  containers): a per-process `RLIMIT_DATA` memory cap on Linux. macOS has no
  enforceable equivalent, so limits are reported as unenforced there.
- Fallback is **run + warn**, never fail: filesystem confinement still fails
  closed, but missing resource limits only produce a phase note and a
  `flows doctor` line.

## Capabilities

### New Capabilities

_None._

### Modified Capabilities

- `security-model`: adds a requirement for resource-limited sandboxed commands.

## Impact

- **Code:** `crates/flows-tools/src/sandbox/` (limits probe, cgroup wrapper,
  rlimit fallback), `bash.rs` / `plugin.rs` (pass limits),
  `crates/flows-core` (`BashConfig.memory_mb` / `max_procs`),
  `crates/flows-runner/src/agent.rs` (phase note), `flows doctor`.
- **Security invariants:** strengthened, not relaxed. `systemd-run` runs
  *outside* bwrap: it gets `XDG_RUNTIME_DIR` / `DBUS_SESSION_BUS_ADDRESS` to
  reach the user manager, but bwrap's `--clearenv` and the unix-socket seccomp
  filter keep both the variables and the bus out of the sandboxed command.
- **On-disk formats:** none.
- **Dependencies:** none new (`systemd-run` is used when present; `libc` for rlimits).
