## Context

`run_confined` builds a bwrap argv and spawns it with a cleared environment and
a timeout. Verified on the dev host: a `systemd-run --user --scope` cgroup
enforces `MemoryMax` (OOM kill, exit 137) and `TasksMax`, composes with bwrap,
and costs about 15 ms per command.

## Goals / Non-Goals

**Goals:** cap memory and process count for the whole command tree on Linux;
per-phase overrides; honest reporting when limits are weaker or absent.

**Non-Goals:** CPU quotas (timeouts already bound runtime), disk quotas,
cgroup v1 support, macOS enforcement.

## Decisions

### D1. systemd transient scope, probed once

Wrap as `systemd-run --user --scope -q -p MemoryMax=<n>M -p MemorySwapMax=0
-p TasksMax=<n> -- bwrap …`. The probe runs a scope with small limits and reads
back `memory.max` / `pids.max` from the scope's own cgroup. That catches hosts
where `systemd-run` succeeds but the memory/pids controllers aren't delegated
(limits would silently be ignored). The result is cached per process.

*Alternative:* write cgroups directly under the user's delegated subtree.
Rejected: it needs us to find and own a writable cgroup, which systemd already does.

### D2. Environment for systemd-run only

The spawn still clears the host environment; when the scope wrapper is used it
additionally passes `XDG_RUNTIME_DIR` and `DBUS_SESSION_BUS_ADDRESS` (taken from
the host) so `systemd-run` can reach the user manager. bwrap's `--clearenv`
removes them before the command starts, and the seccomp filter blocks the bus
socket regardless.

### D3. Fallback: per-process RLIMIT_DATA (Linux), nothing (macOS)

Without a scope, `pre_exec` sets `RLIMIT_DATA` to the memory limit. It's
per-process, not per-tree, and there's no usable per-command process cap
(`RLIMIT_NPROC` counts all of the user's processes). macOS doesn't enforce
`RLIMIT_DATA`/`RLIMIT_AS` meaningfully, so it reports `none`.
`sandbox::limits_mode()` → `Cgroup | PerProcess | None`.

### D4. Run + warn

Missing or weaker limits never block a command. The agent adds one note per
phase that enables bash (`⚠ resource limits: …`), and `flows doctor` prints the
mode with the fix (enable a systemd user session / lingering).

### D5. Config

`tools.bash.memory_mb` and `tools.bash.max_procs` (`Option<u64>`; unset →
8192 / 1024; `0` → unlimited for that dimension). Plugin tools use the
defaults.

## Risks / Trade-offs

- **Big builds over 8 GB get OOM-killed** → the exit is reported as "killed by
  signal" plus a hint naming `tools.bash.memory_mb`; raise it per phase.
- **`MemorySwapMax=0` on swapless hosts** is a no-op; harmless.
- **Probe false negative** (e.g. no session bus in a headless SSH login without
  lingering) → falls back to RLIMIT_DATA with a warning; documented fix is
  `loginctl enable-linger`.
