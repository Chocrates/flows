//! Resource limits for sandboxed commands (memory + process count).
//!
//! Filesystem confinement is the security boundary and fails closed; resource
//! limits protect the host from runaway commands (a leaking test, a fork bomb)
//! and **degrade** instead: they never stop a command from running.
//!
//! * **Linux + systemd user session** ([`LimitMode::Cgroup`]): the whole command
//!   tree runs in a transient scope (`systemd-run --user --scope`) with
//!   `MemoryMax`, `MemorySwapMax=0`, and `TasksMax`. `systemd-run` sits *outside*
//!   bwrap; bwrap's `--clearenv` and the unix-socket seccomp filter keep the
//!   session bus out of the sandboxed command.
//! * **Linux without one** ([`LimitMode::PerProcess`]): `RLIMIT_DATA` per process.
//!   There is no usable per-command process cap (`RLIMIT_NPROC` is per user).
//! * **macOS** ([`LimitMode::None`]): nothing the kernel reliably enforces.

use std::process::Command;
use std::sync::OnceLock;

use flows_core::models::BashConfig;

/// Default memory cap per command tree (MB).
pub const DEFAULT_MEMORY_MB: u64 = 8192;
/// Default process/thread cap per command tree.
pub const DEFAULT_MAX_PROCS: u64 = 1024;

/// Limits for one command. `0` disables that dimension.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    pub memory_mb: u64,
    pub max_procs: u64,
}

impl Default for Limits {
    fn default() -> Self {
        Limits {
            memory_mb: DEFAULT_MEMORY_MB,
            max_procs: DEFAULT_MAX_PROCS,
        }
    }
}

impl Limits {
    /// A phase's bash limits: its overrides, else the defaults.
    pub fn from_bash(cfg: &BashConfig) -> Limits {
        Limits {
            memory_mb: cfg.memory_mb.unwrap_or(DEFAULT_MEMORY_MB),
            max_procs: cfg.max_procs.unwrap_or(DEFAULT_MAX_PROCS),
        }
    }

    /// No limit in either dimension.
    pub fn is_unlimited(&self) -> bool {
        self.memory_mb == 0 && self.max_procs == 0
    }
}

/// How limits are enforced on this host.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LimitMode {
    /// Per-command cgroup: memory + process caps for the whole tree.
    Cgroup,
    /// Per-process memory rlimit only.
    PerProcess,
    /// Not enforced.
    None,
}

impl LimitMode {
    /// One line for `flows doctor` / phase notes.
    pub fn describe(self) -> &'static str {
        match self {
            LimitMode::Cgroup => "cgroup (systemd user scope): memory + process caps per command",
            LimitMode::PerProcess => {
                "per-process memory rlimit only — no systemd user scope with memory/pids \
                 delegation, so process count is uncapped and memory is capped per process, \
                 not per command (enable a user session, e.g. `loginctl enable-linger`)"
            }
            LimitMode::None => "not enforced on this platform",
        }
    }
}

/// The active mode, probed once per process.
pub fn limits_mode() -> LimitMode {
    static MODE: OnceLock<LimitMode> = OnceLock::new();
    *MODE.get_or_init(detect)
}

#[cfg(target_os = "linux")]
fn detect() -> LimitMode {
    if cgroup_probe() {
        LimitMode::Cgroup
    } else {
        LimitMode::PerProcess
    }
}

#[cfg(not(target_os = "linux"))]
fn detect() -> LimitMode {
    LimitMode::None
}

/// `systemd-run` argv prefix that runs the rest of the argv in a limited scope.
pub fn cgroup_prefix(l: &Limits) -> Vec<String> {
    let mut v: Vec<String> = ["systemd-run", "--user", "--scope", "--quiet"]
        .iter()
        .map(|s| s.to_string())
        .collect();
    if l.memory_mb > 0 {
        v.push("-p".into());
        v.push(format!("MemoryMax={}M", l.memory_mb));
        v.push("-p".into());
        v.push("MemorySwapMax=0".into());
    }
    if l.max_procs > 0 {
        v.push("-p".into());
        v.push(format!("TasksMax={}", l.max_procs));
    }
    v.push("--".into());
    v
}

/// Environment `systemd-run` needs to reach the user manager. Only ever given to
/// the outer `systemd-run` process; bwrap clears it for the sandboxed command.
pub fn bus_env() -> Vec<(&'static str, String)> {
    let runtime = std::env::var("XDG_RUNTIME_DIR")
        .unwrap_or_else(|_| format!("/run/user/{}", unsafe { libc::getuid() }));
    let bus = std::env::var("DBUS_SESSION_BUS_ADDRESS")
        .unwrap_or_else(|_| format!("unix:path={runtime}/bus"));
    vec![
        ("XDG_RUNTIME_DIR", runtime),
        ("DBUS_SESSION_BUS_ADDRESS", bus),
    ]
}

/// A scope can be created *and* its memory/pids limits actually land (a host
/// can run scopes without delegating those controllers, which would silently
/// ignore the limits). Reads the values back from the scope's own cgroup.
#[cfg(target_os = "linux")]
fn cgroup_probe() -> bool {
    if super::which("systemd-run").is_none() {
        return false;
    }
    let probe = Limits {
        memory_mb: 64,
        max_procs: 16,
    };
    let mut argv = cgroup_prefix(&probe);
    argv.extend(
        [
            "/bin/sh",
            "-c",
            "p=$(grep '^0::' /proc/self/cgroup | cut -d: -f3); \
             cat /sys/fs/cgroup$p/memory.max /sys/fs/cgroup$p/pids.max",
        ]
        .iter()
        .map(|s| s.to_string()),
    );
    let out = Command::new(&argv[0])
        .args(&argv[1..])
        .env_clear()
        .envs(bus_env())
        .env("PATH", "/usr/local/bin:/usr/bin:/bin")
        .output();
    match out {
        Ok(o) if o.status.success() => {
            let s = String::from_utf8_lossy(&o.stdout);
            let vals: Vec<&str> = s.split_whitespace().collect();
            vals == ["67108864", "16"]
        }
        _ => false,
    }
}

/// Apply the per-process fallback (`RLIMIT_DATA`) to `cmd` on Linux.
#[cfg(target_os = "linux")]
pub fn apply_rlimits(cmd: &mut Command, l: &Limits) {
    use std::os::unix::process::CommandExt;
    if l.memory_mb == 0 {
        return;
    }
    let bytes = l.memory_mb.saturating_mul(1024 * 1024) as libc::rlim_t;
    // SAFETY: setrlimit is async-signal-safe; runs in the child before exec.
    unsafe {
        cmd.pre_exec(move || {
            let lim = libc::rlimit {
                rlim_cur: bytes,
                rlim_max: bytes,
            };
            if libc::setrlimit(libc::RLIMIT_DATA, &lim) != 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
}

#[cfg(not(target_os = "linux"))]
pub fn apply_rlimits(_cmd: &mut Command, _l: &Limits) {}

/// Hint appended when a command was killed the way the memory cap kills it.
pub fn kill_hint(l: &Limits) -> Option<String> {
    (l.memory_mb > 0).then(|| {
        format!(
            "[killed — if it ran out of memory, the limit is {} MB; raise \
             `tools.bash.memory_mb` for this phase]",
            l.memory_mb
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_and_overrides() {
        assert_eq!(Limits::from_bash(&BashConfig::default()), Limits::default());
        let cfg = BashConfig {
            memory_mb: Some(0),
            max_procs: Some(64),
            ..BashConfig::default()
        };
        let l = Limits::from_bash(&cfg);
        assert_eq!((l.memory_mb, l.max_procs), (0, 64));
        assert!(!l.is_unlimited());
        assert!(Limits {
            memory_mb: 0,
            max_procs: 0
        }
        .is_unlimited());
    }

    #[test]
    fn prefix_has_requested_properties_only() {
        let p = cgroup_prefix(&Limits::default());
        assert_eq!(&p[..4], ["systemd-run", "--user", "--scope", "--quiet"]);
        assert!(p.contains(&"MemoryMax=8192M".to_string()));
        assert!(p.contains(&"MemorySwapMax=0".to_string()));
        assert!(p.contains(&"TasksMax=1024".to_string()));
        assert_eq!(p.last().unwrap(), "--");

        let mem_only = cgroup_prefix(&Limits {
            memory_mb: 100,
            max_procs: 0,
        });
        assert!(!mem_only.iter().any(|a| a.starts_with("TasksMax")));
        assert!(mem_only.contains(&"MemoryMax=100M".to_string()));
    }

    #[test]
    fn bus_env_names_both_vars() {
        let keys: Vec<&str> = bus_env().iter().map(|(k, _)| *k).collect();
        assert_eq!(keys, ["XDG_RUNTIME_DIR", "DBUS_SESSION_BUS_ADDRESS"]);
    }

    #[test]
    fn kill_hint_only_with_memory_limit() {
        assert!(kill_hint(&Limits::default()).unwrap().contains("8192 MB"));
        assert!(kill_hint(&Limits {
            memory_mb: 0,
            max_procs: 5
        })
        .is_none());
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn rlimit_fallback_caps_process_memory() {
        let mut cmd = Command::new("python3");
        cmd.args(["-c", "b = bytearray(512 * 1024 * 1024); print('ALLOCATED')"]);
        apply_rlimits(
            &mut cmd,
            &Limits {
                memory_mb: 64,
                max_procs: 0,
            },
        );
        let Ok(out) = cmd.output() else {
            eprintln!("SKIP rlimit_fallback_caps_process_memory: python3 missing");
            return;
        };
        let stdout = String::from_utf8_lossy(&out.stdout);
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(!stdout.contains("ALLOCATED"), "allocation succeeded");
        assert!(stderr.contains("MemoryError"), "unexpected: {stderr}");
    }

    #[test]
    fn mode_descriptions_are_distinct() {
        let d =
            [LimitMode::Cgroup, LimitMode::PerProcess, LimitMode::None].map(LimitMode::describe);
        assert!(d[0] != d[1] && d[1] != d[2]);
    }
}
