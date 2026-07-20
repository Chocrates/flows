//! Filesystem-confined command execution, abstracted over a per-platform
//! backend so the tool layer is platform-agnostic.
//!
//! The contract every backend upholds: the command may read system files (so
//! tools run) but may only **write** inside the project directory and a private
//! `/tmp`; the host environment is cleared (no secrets leak in); execution
//! **fails closed** if no functional sandbox is present. An allowlist alone
//! cannot contain an interpreter — the sandbox is the real boundary.
//!
//! Backends:
//!   * [`bwrap`] — Linux, via bubblewrap + user namespaces.
//!   * [`seatbelt`] — macOS, via `sandbox-exec` (Seatbelt) profiles.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::time::Duration;

use wait_timeout::ChildExt;

use crate::error::{ToolError, ToolResult};

#[cfg(target_os = "linux")]
mod bwrap;
#[cfg(target_os = "macos")]
mod seatbelt;

// Cold compiles are the long pole: a first Bevy build pulls a large dependency
// tree and can run well past 120s. Default generously (15 min) so builds don't
// spuriously time out; a hung command still eventually aborts. Overridable per
// phase via `tools.bash.timeout_secs`.
pub const DEFAULT_TIMEOUT_SECS: u64 = 900;
pub const DEFAULT_MAX_OUTPUT: usize = 20_000;

/// A platform sandbox backend: builds the confined argv and reports whether it
/// can actually run here.
pub trait Sandbox: Send + Sync {
    /// Short backend name (for diagnostics).
    fn name(&self) -> &'static str;
    /// True if this backend can set up a working sandbox on this host.
    fn available(&self) -> bool;
    /// The argv that runs `command` confined to `repo_root`.
    fn wrap(&self, repo_root: &Path, command: &str) -> Vec<String>;
    /// Message shown when [`available`](Self::available) is false.
    fn unavailable_reason(&self) -> &'static str;
}

/// The backend for the current platform.
pub fn backend() -> &'static dyn Sandbox {
    #[cfg(target_os = "linux")]
    {
        static B: bwrap::BwrapSandbox = bwrap::BwrapSandbox;
        &B
    }
    #[cfg(target_os = "macos")]
    {
        static B: seatbelt::SeatbeltSandbox = seatbelt::SeatbeltSandbox;
        &B
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        static B: NoSandbox = NoSandbox;
        &B
    }
}

/// The minimal, secret-free environment injected into every sandboxed command.
/// Host env is cleared first (see [`spawn_with_timeout`]) so API keys / cloud
/// credentials never reach agent-run commands. `HOME=/tmp` keeps tool caches in
/// the private tmpfs.
pub fn minimal_env() -> Vec<(&'static str, String)> {
    let path = std::env::var("PATH").unwrap_or_else(|_| "/usr/local/bin:/usr/bin:/bin".to_string());
    let mut env = vec![
        ("PATH", path),
        ("HOME", "/tmp".into()),
        ("TMPDIR", "/tmp".into()),
        ("LANG", "C.UTF-8".into()),
        ("TERM", "dumb".into()),
    ];
    env.extend(toolchain_env());
    env
}

/// Base dir for persistent, sandbox-writable toolchain caches. `/tmp` is a fresh
/// tmpfs each sandbox call, so without a bound cache every build re-downloads its
/// dependencies. The bwrap backend binds this dir read-write into the sandbox.
pub fn tool_cache_dir() -> PathBuf {
    let base = std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".cache")))
        .unwrap_or_else(|| PathBuf::from("/tmp"));
    let dir = base.join("flows").join("toolcache");
    let _ = std::fs::create_dir_all(&dir);
    dir
}

/// Cross-language toolchain env for sandboxed builds. `HOME=/tmp` hides
/// credential-bearing dotfiles (`~/.npmrc`, `~/.netrc`, cloud creds), so rather
/// than exposing the real home we:
///   1. pass through well-known *locator* vars — where a toolchain / version
///      manager is installed (no secrets) — so compilers + shims still resolve
///      (this is what fixes `cargo`'s "no default toolchain", `nvm`/`pyenv`/`asdf`
///      shims, `JAVA_HOME`, etc.); and
///   2. point each ecosystem's cache at a persistent writable dir (bound into the
///      sandbox), so dependencies aren't re-fetched on every call.
fn toolchain_env() -> Vec<(&'static str, String)> {
    let cache = tool_cache_dir();
    let sub = |s: &str| cache.join(s).to_string_lossy().into_owned();
    let mut env: Vec<(&'static str, String)> = Vec::new();

    // 1. Locator vars: pass through from the host if set (read-only installs).
    const LOCATORS: &[&str] = &[
        "RUSTUP_HOME", // rust toolchains
        "GOROOT",      // go install
        "GOPATH",      // go workspace (module cache is redirected below)
        "JAVA_HOME",   // jdk
        "NVM_DIR",     // node (nvm)
        "VOLTA_HOME",  // node (volta)
        "PNPM_HOME",   // pnpm global
        "PYENV_ROOT",  // python (pyenv)
        "ASDF_DIR",    // asdf core
        "ASDF_DATA_DIR",
        "SDKMAN_DIR", // java/kotlin/etc (sdkman)
    ];
    for &k in LOCATORS {
        if let Ok(v) = std::env::var(k) {
            env.push((k, v));
        }
    }
    // rustup's default location if RUSTUP_HOME isn't explicitly set.
    if !env.iter().any(|(k, _)| *k == "RUSTUP_HOME") {
        if let Some(home) = std::env::var_os("HOME") {
            let p = PathBuf::from(home).join(".rustup");
            if p.is_dir() {
                env.push(("RUSTUP_HOME", p.to_string_lossy().into_owned()));
            }
        }
    }

    // 2. Cache redirects → the persistent writable dir (one bind covers all).
    env.push(("CARGO_HOME", sub("cargo"))); // rust: registry + downloaded crates
    env.push(("GOMODCACHE", sub("go-mod"))); // go: module cache
    env.push(("GOCACHE", sub("go-build"))); // go: build cache
    env.push(("npm_config_cache", sub("npm"))); // npm
    env.push(("YARN_CACHE_FOLDER", sub("yarn"))); // yarn
    env.push(("PIP_CACHE_DIR", sub("pip"))); // pip
    env.push(("UV_CACHE_DIR", sub("uv"))); // uv
    env.push(("GRADLE_USER_HOME", sub("gradle"))); // gradle: caches + dep cache
    env.push(("MAVEN_ARGS", format!("-Dmaven.repo.local={}", sub("m2")))); // maven 3.9+
    env
}

fn which(bin: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|d| d.join(bin))
        .find(|p| p.is_file())
}

/// True if the current platform's sandbox can run here (probed + cached by the
/// backend).
pub fn sandbox_available() -> bool {
    backend().available()
}

/// The confined argv for `command` under `repo_root` (delegates to the backend).
pub fn wrap_command(repo_root: &Path, command: &str) -> Vec<String> {
    backend().wrap(repo_root, command)
}

/// Name of the active backend (`bwrap`, `seatbelt`, or `none`).
pub fn backend_name() -> &'static str {
    backend().name()
}

/// Combine stdout/stderr/exit-code into the model-facing string (pure).
pub fn format_output(output: &Output, max_output: usize) -> String {
    let trunc = |b: &[u8]| String::from_utf8_lossy(&b[..b.len().min(max_output)]).into_owned();
    let stdout = trunc(&output.stdout);
    let stderr = trunc(&output.stderr);
    let mut combined = stdout;
    if !stderr.is_empty() {
        combined.push_str(&format!("\nSTDERR:\n{stderr}"));
    }
    match output.status.code() {
        Some(0) => {}
        Some(code) => combined.push_str(&format!("\n[exit code: {code}]")),
        None => combined.push_str("\n[exit code: killed by signal]"),
    }
    if combined.is_empty() {
        "(no output)".to_string()
    } else {
        combined
    }
}

/// Testable core: `available` and `exec` are injected so output-handling and
/// fail-closed behavior can be tested without a functional sandbox.
fn run_confined_inner<F>(
    command: &str,
    repo_root: &Path,
    max_output: usize,
    available: bool,
    exec: F,
) -> ToolResult<String>
where
    F: FnOnce(&[String]) -> std::io::Result<Output>,
{
    if !available {
        return Err(ToolError::new(backend().unavailable_reason()));
    }
    let argv = wrap_command(repo_root, command);
    let output = exec(&argv).map_err(|e| ToolError::new(format!("sandbox exec failed: {e}")))?;
    Ok(format_output(&output, max_output))
}

/// Run `command` in the platform sandbox. Fails closed if unavailable.
pub fn run_confined(
    command: &str,
    repo_root: &Path,
    timeout_secs: u64,
    max_output: usize,
) -> ToolResult<String> {
    run_confined_inner(
        command,
        repo_root,
        max_output,
        sandbox_available(),
        |argv| spawn_with_timeout(argv, repo_root, timeout_secs),
    )
}

fn spawn_with_timeout(argv: &[String], cwd: &Path, timeout_secs: u64) -> std::io::Result<Output> {
    let mut child = Command::new(&argv[0])
        .args(&argv[1..])
        .current_dir(cwd)
        // Clear the host env and inject only the minimal set. On Linux bwrap
        // also --clearenv's (defense in depth); on macOS this is the sole env
        // control, since Seatbelt profiles don't scrub the environment.
        .env_clear()
        .envs(minimal_env())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;

    // Drain both pipes on threads so a chatty child can't deadlock on a full
    // pipe while we wait.
    let mut out_pipe = child.stdout.take().unwrap();
    let mut err_pipe = child.stderr.take().unwrap();
    let out_h = std::thread::spawn(move || {
        let mut b = Vec::new();
        let _ = out_pipe.read_to_end(&mut b);
        b
    });
    let err_h = std::thread::spawn(move || {
        let mut b = Vec::new();
        let _ = err_pipe.read_to_end(&mut b);
        b
    });

    let status = match child.wait_timeout(Duration::from_secs(timeout_secs))? {
        Some(s) => s,
        None => {
            let _ = child.kill();
            let _ = child.wait();
            return Err(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                format!("command timed out after {timeout_secs}s"),
            ));
        }
    };
    let stdout = out_h.join().unwrap_or_default();
    let stderr = err_h.join().unwrap_or_default();
    Ok(Output {
        status,
        stdout,
        stderr,
    })
}

/// Fallback backend for unsupported platforms: always unavailable (fail-closed).
#[cfg(not(any(target_os = "linux", target_os = "macos")))]
struct NoSandbox;
#[cfg(not(any(target_os = "linux", target_os = "macos")))]
impl Sandbox for NoSandbox {
    fn name(&self) -> &'static str {
        "none"
    }
    fn available(&self) -> bool {
        false
    }
    fn wrap(&self, _repo_root: &Path, command: &str) -> Vec<String> {
        vec!["/bin/sh".into(), "-c".into(), command.into()]
    }
    fn unavailable_reason(&self) -> &'static str {
        "no command sandbox is available on this platform; disable the bash/plugin tools"
    }
}

// ---------------------------------------------------------------------------
// Test seam
// ---------------------------------------------------------------------------

#[doc(hidden)]
pub mod testing {
    use super::*;

    /// Exposes `run_confined_inner` for tests (inject availability + a fake exec).
    pub fn run_confined_inner_for_test<F>(
        command: &str,
        repo_root: &Path,
        max_output: usize,
        available: bool,
        exec: F,
    ) -> ToolResult<String>
    where
        F: FnOnce(&[String]) -> std::io::Result<Output>,
    {
        super::run_confined_inner(command, repo_root, max_output, available, exec)
    }
}
