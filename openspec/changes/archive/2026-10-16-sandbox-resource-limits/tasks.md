## 1. Config

- [x] 1.1 Add `memory_mb` / `max_procs` (`Option<u64>`) to `BashConfig` with parsing from `tools.bash`; defaults 8192 / 1024; tests.

## 2. Sandbox limits

- [x] 2.1 `sandbox::Limits` + `limits_mode()` (`Cgroup | PerProcess | None`) with a cached cgroup probe that reads back `memory.max` / `pids.max`.
- [x] 2.2 Cgroup wrapper: prefix `systemd-run --user --scope …` to the backend argv and pass `XDG_RUNTIME_DIR` / `DBUS_SESSION_BUS_ADDRESS` to the spawn only in that mode; pure argv builder unit-tested.
- [x] 2.3 Fallback: `RLIMIT_DATA` in `pre_exec` on Linux when not in cgroup mode; none on macOS.
- [x] 2.4 Thread `Limits` through `run_confined` → bash (from `BashConfig`) and plugin (defaults); OOM/kill output names `tools.bash.memory_mb`.
- [x] 2.5 E2E tests, gated on cgroup mode: memory hog killed; fork bomb capped; normal command unaffected.

## 3. Reporting

- [x] 3.1 Agent: one `on_note` per phase with bash enabled when mode ≠ `Cgroup`.
- [x] 3.2 `flows doctor`: print limit mode and the fix.

## 4. Docs & wrap-up

- [x] 4.1 `docs/configuration.md` (bash limits), `DESIGN.md` sandbox section, `ROADMAP.md`.
- [x] 4.2 `make check` green; `openspec validate sandbox-resource-limits`.
