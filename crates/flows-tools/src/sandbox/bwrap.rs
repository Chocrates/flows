//! Linux sandbox backend: [bubblewrap](https://github.com/containers/bubblewrap).
//!
//! The whole filesystem is bound read-only, the repo re-bound read-write and
//! made the working directory, `/tmp` a private tmpfs, and user/ipc/pid/uts/
//! cgroup namespaces unshared, credential stores masked (empty tmpfs over a
//! directory, `/dev/null` over a file), and a seccomp filter (see `seccomp`) denies unix
//! sockets so the command can't reach host services (session bus, container
//! engines, display server) that would run code outside the sandbox. Network is
//! left shared so package/build commands can fetch dependencies, unless the
//! phase is `offline` (`--unshare-net`: loopback only). Requires
//! unprivileged user namespaces.

use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::OnceLock;

use super::{minimal_env, seccomp, tool_cache_dir, which, Confinement, Sandbox};

pub struct BwrapSandbox;

/// Child-side fd number the seccomp program is handed to bwrap on.
const SECCOMP_FD: i32 = 999;

fn seccomp_program() -> Option<&'static [u8]> {
    static PROG: OnceLock<Option<Vec<u8>>> = OnceLock::new();
    PROG.get_or_init(seccomp::program).as_deref()
}

impl Sandbox for BwrapSandbox {
    fn name(&self) -> &'static str {
        "bwrap"
    }

    fn available(&self) -> bool {
        static PROBE: OnceLock<bool> = OnceLock::new();
        *PROBE.get_or_init(probe)
    }

    fn wrap(
        &self,
        repo_root: &Path,
        command: &str,
        c: &Confinement,
        hidden: &[PathBuf],
    ) -> Vec<String> {
        let root = repo_root
            .canonicalize()
            .unwrap_or_else(|_| repo_root.to_path_buf())
            .to_string_lossy()
            .into_owned();

        let mut argv: Vec<String> = [
            "bwrap",
            "--die-with-parent",
            "--unshare-user",
            "--unshare-ipc",
            "--unshare-pid",
            "--unshare-uts",
            "--unshare-cgroup",
            "--clearenv", // defense in depth; spawn also clears the env
            "--ro-bind",
            "/",
            "/",
            "--dev",
            "/dev",
            "--proc",
            "/proc",
            "--tmpfs",
            "/tmp",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();

        if c.offline {
            argv.push("--unshare-net".into());
        }

        // Credential masks go before the repo/cache binds: later mounts win, and
        // bwrap creates their mount points inside a mask's tmpfs, so a repo that
        // lives under a masked dir stays reachable.
        for p in hidden {
            let p = p.to_string_lossy().into_owned();
            if Path::new(&p).is_dir() {
                argv.extend(["--tmpfs".into(), p]);
            } else {
                argv.extend(["--ro-bind".into(), "/dev/null".into(), p]);
            }
        }

        argv.extend(["--bind".into(), root.clone(), root.clone()]);
        argv.extend(["--chdir".into(), root]);

        // Persistent, writable toolchain cache (cargo registry, go/npm/pip/gradle/…
        // caches) so sandboxed builds survive the fresh-`/tmp`-per-call reset. Bound
        // read-write over the read-only root bind (later binds win).
        let cache = tool_cache_dir().to_string_lossy().into_owned();
        argv.push("--bind".into());
        argv.push(cache.clone());
        argv.push(cache);

        for (k, v) in minimal_env() {
            argv.push("--setenv".into());
            argv.push(k.into());
            argv.push(v);
        }

        if seccomp_program().is_some() {
            argv.push("--seccomp".into());
            argv.push(SECCOMP_FD.to_string());
        }

        argv.extend(
            ["--", "/bin/sh", "-c", command]
                .iter()
                .map(|s| s.to_string()),
        );
        argv
    }

    fn prepare(&self, cmd: &mut Command) {
        let Some(prog) = seccomp_program() else {
            return;
        };
        // SAFETY: runs in the forked child before exec; only async-signal-safe
        // syscalls (memfd_create/write/lseek/dup2/close) on a pre-built buffer.
        unsafe {
            cmd.pre_exec(move || {
                let fd = libc::memfd_create(c"flows-seccomp".as_ptr(), 0);
                if fd < 0 {
                    return Err(std::io::Error::last_os_error());
                }
                let mut off = 0;
                while off < prog.len() {
                    let n = libc::write(fd, prog[off..].as_ptr().cast(), prog.len() - off);
                    if n <= 0 {
                        return Err(std::io::Error::last_os_error());
                    }
                    off += n as usize;
                }
                if libc::lseek(fd, 0, libc::SEEK_SET) < 0
                    || (fd != SECCOMP_FD && libc::dup2(fd, SECCOMP_FD) < 0)
                {
                    return Err(std::io::Error::last_os_error());
                }
                if fd != SECCOMP_FD {
                    libc::close(fd);
                }
                Ok(())
            });
        }
    }

    fn unavailable_reason(&self) -> &'static str {
        "command sandbox unavailable: running shell commands requires bubblewrap ('bwrap') with \
unprivileged user namespaces enabled, so commands can be confined to the project directory. \
Install bwrap (e.g. 'apt install bubblewrap') and enable unprivileged user namespaces, or \
disable the tool for this phase."
    }
}

/// bwrap is present AND can actually set up a sandbox (userns enabled).
fn probe() -> bool {
    if which("bwrap").is_none() {
        return false;
    }
    Command::new("bwrap")
        .args(["--ro-bind", "/", "/", "--unshare-user", "--", "true"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}
