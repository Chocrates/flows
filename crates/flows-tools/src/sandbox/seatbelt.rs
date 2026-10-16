//! macOS sandbox backend: `sandbox-exec` (Seatbelt) with an inline SBPL profile.
//!
//! Seatbelt is the closest macOS analogue to bubblewrap. The profile denies by
//! default, allows reads anywhere except credential stores (so tools/dylibs
//! load), and restricts **writes** to the repo, temp dirs, and the toolchain
//! cache. The host environment is cleared at spawn time (Seatbelt profiles
//! don't scrub env).
//!
//! Host IPC is closed the way the Linux seccomp filter closes it: network is IP
//! only — unix-socket connects are denied except the DNS resolver and syslog
//! sockets — and mach-lookup is limited to [`MACH_SERVICES`]. Unrestricted
//! mach-lookup would let a command ask LaunchServices or AppleEvents to start
//! programs outside the sandbox (`open -a Terminal`, `osascript`).
//!
//! `sandbox-exec` is deprecated by Apple but still ships and works on current
//! macOS; if Apple removes it we'd move to a helper linking `sandbox_init`.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::OnceLock;

use super::{tool_cache_dir, Confinement, Sandbox};

/// Mach services a sandboxed command may look up: user/group lookup, prefs,
/// logging, TLS trust evaluation, DNS/network configuration, file events.
/// Everything else (LaunchServices, AppleEvents, pasteboard, window server, …)
/// is denied.
pub const MACH_SERVICES: &[&str] = &[
    "com.apple.system.opendirectoryd.libinfo",
    "com.apple.system.opendirectoryd.membership",
    "com.apple.system.notification_center",
    "com.apple.system.logger",
    "com.apple.logd",
    "com.apple.diagnosticd",
    "com.apple.bsd.dirhelper",
    "com.apple.cfprefsd.daemon",
    "com.apple.cfprefsd.agent",
    "com.apple.SecurityServer",
    "com.apple.trustd",
    "com.apple.trustd.agent",
    "com.apple.ocspd",
    "com.apple.SystemConfiguration.configd",
    "com.apple.SystemConfiguration.DNSConfiguration",
    "com.apple.networkd",
    "com.apple.dnssd.service",
    "com.apple.FSEvents",
];

/// Unix sockets a command may connect to (DNS resolution, syslog).
const UNIX_SOCKETS: &[&str] = &["/private/var/run/mDNSResponder", "/private/var/run/syslog"];

pub struct SeatbeltSandbox;

impl Sandbox for SeatbeltSandbox {
    fn name(&self) -> &'static str {
        "seatbelt"
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
        let profile = profile_for(&root, c.offline, hidden);
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

fn sbpl_escape(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

/// Build the SBPL profile confining writes to `root` (+ temp + toolchain
/// cache), allowing reads except `hidden`, IP networking, and the
/// [`MACH_SERVICES`] allowlist. Later SBPL rules take precedence, so the read
/// denies follow the broad `(allow file-read*)`.
/// `offline` keeps loopback only and drops the DNS resolver socket.
fn profile_for(root: &str, offline: bool, hidden: &[PathBuf]) -> String {
    profile_with_cache(root, &tool_cache_dir().to_string_lossy(), offline, hidden)
}

fn profile_with_cache(root: &str, cache: &str, offline: bool, hidden: &[PathBuf]) -> String {
    let esc = sbpl_escape(root);
    let cache = sbpl_escape(cache);
    let mach: String = MACH_SERVICES
        .iter()
        .map(|s| format!("\n  (global-name \"{s}\")"))
        .collect();
    let sockets: String = UNIX_SOCKETS
        .iter()
        .filter(|s| !(offline && s.contains("mDNSResponder")))
        .map(|s| format!("\n  (literal \"{s}\")"))
        .collect();
    let host = if offline { "localhost" } else { "*" };
    let mut denies = String::new();
    for p in hidden {
        let kind = if p.is_dir() { "subpath" } else { "literal" };
        denies.push_str(&format!(
            "(deny file-read* ({kind} \"{}\"))\n",
            sbpl_escape(&p.to_string_lossy())
        ));
    }
    let profile = format!(
        r#"(version 1)
(deny default)
(allow process-exec)
(allow process-fork)
(allow signal (target same-sandbox))
(allow sysctl-read)
(allow ipc-posix-shm)
(allow system-socket)
(allow network-outbound (remote ip "{host}:*"))
(allow network-inbound (local ip "{host}:*"))
(allow network-bind (local ip "{host}:*"))
(allow network-outbound{sockets})
(allow mach-lookup{mach})
(allow file-read*)
(allow file-write*
  (subpath "{esc}")
  (subpath "{cache}")
  (subpath "/tmp")
  (subpath "/private/tmp")
  (subpath "/private/var/folders")
  (literal "/dev/null")
  (literal "/dev/zero")
  (literal "/dev/dtracehelper")
  (literal "/dev/tty"))
"#
    );
    // A repo under a hidden dir must stay readable: re-allow it last.
    let reallow = if denies.is_empty() {
        String::new()
    } else {
        format!("(allow file-read* (subpath \"{esc}\"))\n")
    };
    profile + &denies + &reallow
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
    use super::{profile_with_cache, MACH_SERVICES};
    use std::path::PathBuf;

    fn profile(root: &str, hidden: &[PathBuf]) -> String {
        profile_with_cache(root, "/Users/me/.cache/flows/toolcache", false, hidden)
    }

    #[test]
    fn offline_profile_is_loopback_only_without_dns() {
        let p = profile_with_cache("/Users/me/proj", "/c", true, &[]);
        assert!(p.contains("(allow network-outbound (remote ip \"localhost:*\"))"));
        assert!(!p.contains("\"*:*\""));
        assert!(!p.contains("mDNSResponder"));
        assert!(p.contains("/private/var/run/syslog"));
    }

    #[test]
    fn profile_confines_writes_to_root_tmp_and_cache() {
        let p = profile("/Users/me/proj", &[]);
        assert!(p.contains("(deny default)"));
        assert!(p.contains("(allow file-read*)"));
        assert!(p.contains("(subpath \"/Users/me/proj\")"));
        assert!(p.contains("(subpath \"/Users/me/.cache/flows/toolcache\")"));
        assert!(p.contains("(subpath \"/private/tmp\")"));
        assert!(p.contains("(allow file-write*"));
    }

    #[test]
    fn profile_has_no_blanket_ipc() {
        let p = profile("/Users/me/proj", &[]);
        // network is IP-only; unix sockets only for the listed paths
        assert!(!p.contains("(allow network*)"));
        assert!(p.contains("(allow network-outbound (remote ip \"*:*\"))"));
        assert!(p.contains("(literal \"/private/var/run/mDNSResponder\")"));
        // mach-lookup is an allowlist, never bare
        assert!(!p.contains("(allow mach-lookup)"));
        for s in MACH_SERVICES {
            assert!(p.contains(&format!("(global-name \"{s}\")")), "{s}");
        }
        assert!(!p.contains("launchservicesd") && !p.contains("appleevents"));
    }

    #[test]
    fn profile_escapes_quotes_in_path() {
        let p = profile("/tmp/we\"ird", &[]);
        assert!(p.contains("we\\\"ird"));
    }

    #[test]
    fn profile_denies_hidden_paths_after_read_allow() {
        let d = tempfile::tempdir().unwrap();
        let dir = d.path().join(".ssh");
        std::fs::create_dir(&dir).unwrap();
        let file = d.path().join(".netrc");
        std::fs::write(&file, "x").unwrap();
        let p = profile("/Users/me/proj", &[dir.clone(), file.clone()]);
        let allow = p.find("(allow file-read*)").unwrap();
        let deny_dir = p
            .find(&format!(
                "(deny file-read* (subpath \"{}\"))",
                dir.display()
            ))
            .unwrap();
        let deny_file = p
            .find(&format!(
                "(deny file-read* (literal \"{}\"))",
                file.display()
            ))
            .unwrap();
        assert!(allow < deny_dir && allow < deny_file);
        // the repo is re-allowed after the denies
        let reallow = p
            .rfind("(allow file-read* (subpath \"/Users/me/proj\"))")
            .unwrap();
        assert!(reallow > deny_dir && reallow > deny_file);
    }
}
