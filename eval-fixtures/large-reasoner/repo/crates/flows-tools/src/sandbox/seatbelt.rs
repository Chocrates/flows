//! macOS sandbox backend: `sandbox-exec` (Seatbelt) with an inline SBPL profile.
//!
//! Seatbelt is the closest macOS analogue to bubblewrap. The profile denies by
//! default, allows reads anywhere (so tools/dylibs load), and restricts **writes**
//! to the repo + temp dirs. Network is allowed to match the Linux backend. The
//! host environment is cleared at spawn time (Seatbelt profiles don't scrub env).
//!
//! `sandbox-exec` is deprecated by Apple but still ships and works on current
//! macOS; if Apple removes it we'd move to a helper linking `sandbox_init`.

use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::OnceLock;

use super::Sandbox;

pub struct SeatbeltSandbox;

impl Sandbox for SeatbeltSandbox {
    fn name(&self) -> &'static str {
        "seatbelt"
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
        let profile = profile_for(&root);
        vec![
            "sandbox-exec".into(),
            "-p".into(),
            profile,
            "/bin/sh".into(),
            "-c".into(),
            command.into(),
        ]
    }

    fn unavailable_reason(&self) -> &'static str {
        "command sandbox unavailable: 'sandbox-exec' (Seatbelt) was not found or failed to \
initialize, so commands cannot be confined to the project directory. Disable the tool for \
this phase."
    }
}

/// Build the SBPL profile confining writes to `root` (+ temp) while allowing
/// reads and network. `root` is escaped for the quoted SBPL string.
fn profile_for(root: &str) -> String {
    let esc = root.replace('\\', "\\\\").replace('"', "\\\"");
    format!(
        r#"(version 1)
(deny default)
(allow process-exec)
(allow process-fork)
(allow signal (target self))
(allow sysctl-read)
(allow mach-lookup)
(allow ipc-posix-shm)
(allow network*)
(allow file-read*)
(allow file-write*
  (subpath "{esc}")
  (subpath "/tmp")
  (subpath "/private/tmp")
  (subpath "/private/var/folders")
  (literal "/dev/null")
  (literal "/dev/zero")
  (literal "/dev/dtracehelper")
  (literal "/dev/tty"))
"#
    )
}

/// `sandbox-exec` exists and can initialize a trivial profile here.
fn probe() -> bool {
    if super::which("sandbox-exec").is_none() {
        return false;
    }
    Command::new("sandbox-exec")
        .args(["-p", "(version 1)(allow default)", "/usr/bin/true"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::profile_for;

    #[test]
    fn profile_confines_writes_to_root_and_tmp() {
        let p = profile_for("/Users/me/proj");
        assert!(p.contains("(deny default)"));
        assert!(p.contains("(allow file-read*)"));
        assert!(p.contains("(subpath \"/Users/me/proj\")"));
        assert!(p.contains("(subpath \"/private/tmp\")"));
        // writes are the restricted verb; reads/network stay open
        assert!(p.contains("(allow file-write*"));
        assert!(p.contains("(allow network*)"));
    }

    #[test]
    fn profile_escapes_quotes_in_path() {
        let p = profile_for("/tmp/we\"ird");
        assert!(p.contains("we\\\"ird"));
    }
}
