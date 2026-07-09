//! Linux sandbox backend: [bubblewrap](https://github.com/containers/bubblewrap).
//!
//! The whole filesystem is bound read-only, the repo re-bound read-write and
//! made the working directory, `/tmp` a private tmpfs, and user/ipc/pid/uts/
//! cgroup namespaces unshared. Network is left shared so package/build commands
//! can fetch dependencies. Requires unprivileged user namespaces.

use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::OnceLock;

use super::{minimal_env, which, Sandbox};

pub struct BwrapSandbox;

impl Sandbox for BwrapSandbox {
    fn name(&self) -> &'static str {
        "bwrap"
    }

    fn available(&self) -> bool {
        static PROBE: OnceLock<bool> = OnceLock::new();
        *PROBE.get_or_init(probe)
    }

    fn wrap(&self, repo_root: &Path, command: &str) -> Vec<String> {
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
            "--bind",
            &root,
            &root,
            "--chdir",
            &root,
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();

        for (k, v) in minimal_env() {
            argv.push("--setenv".into());
            argv.push(k.into());
            argv.push(v);
        }

        argv.extend(
            ["--", "/bin/sh", "-c", command]
                .iter()
                .map(|s| s.to_string()),
        );
        argv
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
