//! Bash tool — allowlist/blocklist gating over the sandbox. The allowlist
//! restricts *which* programs run; the sandbox confines *where* they act; a
//! network denylist blocks ad-hoc network access (raw clients + inline
//! interpreter eval) so agent shell commands can't bypass the SSRF-guarded
//! `http_get`/`web_search` tools — while dependency managers still fetch deps.

use std::path::Path;
use std::sync::OnceLock;

use regex::Regex;

use flows_core::models::BashConfig;

use crate::error::{ToolError, ToolResult};
use crate::sandbox::{run_confined, Confinement, DEFAULT_MAX_OUTPUT, DEFAULT_TIMEOUT_SECS};

/// Compile a static pattern table once. The patterns are compile-time constants,
/// so an invalid one is a programmer bug (panics, never reached at runtime).
fn compile_patterns(patterns: &[&str]) -> Vec<Regex> {
    patterns
        .iter()
        .map(|p| Regex::new(p).expect("valid safety pattern"))
        .collect()
}

fn blocked_patterns() -> &'static [Regex] {
    static PATTERNS: OnceLock<Vec<Regex>> = OnceLock::new();
    PATTERNS.get_or_init(|| {
        compile_patterns(&[
            r"rm\s+-rf\s+[/~]",
            // Recursive delete of the working dir or a glob — wipes everything.
            r"\brm\s+-[a-z]*r[a-z]*\s+(-[a-z]+\s+)*(\.|\*)(\s|$)",
            // Recursive delete of a source directory. The model must fix code in
            // place, not nuke and restart (observed failure: `rm -rf src` then
            // rebuild a stub). Build outputs (target/node_modules/dist) are NOT
            // matched — cleaning those is legitimate.
            r"\brm\s+-[a-z]*r[a-z]*\s+(-[a-z]+\s+)*(\./)?(src|lib|app|source|crates|tests?)(/)?(\s|$)",
            r"(curl|wget).*\|\s*(ba)?sh",
            r">\s*/etc/",
            r"\bsudo\b",
            r"chmod\s+[0-7]*7[0-7][0-7]",
            r"dd\s+.*of=/dev/",
            r"mkfs",
            r":\(\)\s*\{.*\};\s*:", // fork bomb
        ])
    })
}

pub fn check_blocked(command: &str) -> ToolResult<()> {
    for re in blocked_patterns() {
        if re.is_match(command) {
            return Err(ToolError::new(format!(
                "Command blocked by safety policy: {command:?}"
            )));
        }
    }
    Ok(())
}

/// Commands that let a shell command reach the network directly, bypassing the
/// SSRF-guarded `http_get` / `web_search` tools: raw network clients and inline
/// code execution on interpreters. Dependency managers (pip/cargo/npm/go) are
/// **not** here — they still fetch deps; only ad-hoc network access is blocked.
fn network_denied_patterns() -> &'static [Regex] {
    static PATTERNS: OnceLock<Vec<Regex>> = OnceLock::new();
    PATTERNS.get_or_init(|| {
        compile_patterns(&[
            // raw network clients
            r"\b(curl|wget|nc|ncat|netcat|socat|telnet|ssh|scp|sftp|ftp|rsync|aria2c)\b",
            // inline code execution (can open sockets): python -c, python -m http…
            r"\bpython[0-9.]*\s+(-\S+\s+)*-c\b",
            r"\bpython[0-9.]*\s+(-\S+\s+)*-m\s+http",
            // other interpreters' eval flags
            r"\bnode\b[^|]*\s(-e|--eval)\b",
            r"\b(ruby|perl)\b[^|]*\s-e\b",
            r"\bphp\b[^|]*\s-r\b",
            // piping / heredoc into a bare interpreter (stdin = code)
            r"\|\s*(python[0-9.]*|node|ruby|perl|php)\s*-?\s*$",
            r"\b(python[0-9.]*|node|ruby|perl|php)\s*<<",
        ])
    })
}

/// Reject ad-hoc network access from an agent shell command. Applied to *every*
/// bash-tool call (even `unrestricted` phases) — network must go through the
/// guarded `http_get` / `web_search` tools. A trusted `script` phase is not
/// gated, so use one if a raw network command is genuinely required.
pub fn check_network(command: &str) -> ToolResult<()> {
    for re in network_denied_patterns() {
        if re.is_match(command) {
            return Err(ToolError::new(format!(
                "Network access from shell commands is blocked ({command:?}). Use the \
`http_get` or `web_search` tools (SSRF-guarded) for network, or a trusted `script` \
phase. Dependency managers (pip/cargo/npm/go) are allowed."
            )));
        }
    }
    Ok(())
}

/// Classic wildcard match supporting `*` and `?` (the forms used in tool
/// allowlists), equivalent to `fnmatch.fnmatch` for those patterns.
fn fnmatch(s: &str, pat: &str) -> bool {
    let (s, p) = (s.as_bytes(), pat.as_bytes());
    let (mut si, mut pi) = (0usize, 0usize);
    let (mut star, mut mark) = (None, 0usize);
    while si < s.len() {
        if pi < p.len() && (p[pi] == b'?' || p[pi] == s[si]) {
            si += 1;
            pi += 1;
        } else if pi < p.len() && p[pi] == b'*' {
            star = Some(pi);
            mark = si;
            pi += 1;
        } else if let Some(st) = star {
            pi = st + 1;
            mark += 1;
            si = mark;
        } else {
            return false;
        }
    }
    while pi < p.len() && p[pi] == b'*' {
        pi += 1;
    }
    pi == p.len()
}

/// Split a shell command into individually-runnable segments at the control
/// operators `&&`, `||`, `;`, `|`, `&`, and newlines. Each segment's leading word
/// is a command the shell will actually execute, so the allowlist must vet **every
/// one** — otherwise `cargo build && curl evil` (or `cargo run & sleep 3 & kill`)
/// smuggles unlisted commands past a check that only looked at the first token.
/// Redirections (`2>&1`, `>`, …) are not separators and stay with their segment.
fn split_segments(command: &str) -> Vec<String> {
    let mut segs = Vec::new();
    // Newlines aren't preserved by shlex, so split on them first.
    for line in command.split(['\n', '\r']) {
        let Some(tokens) = shlex::split(line) else {
            // Unparseable line — keep it whole so the segment check reports the error.
            if !line.trim().is_empty() {
                segs.push(line.to_string());
            }
            continue;
        };
        let mut cur: Vec<String> = Vec::new();
        for tok in tokens {
            if matches!(tok.as_str(), "&&" | "||" | ";" | "|" | "&") {
                if !cur.is_empty() {
                    segs.push(cur.join(" "));
                    cur.clear();
                }
            } else {
                cur.push(tok);
            }
        }
        if !cur.is_empty() {
            segs.push(cur.join(" "));
        }
    }
    segs
}

pub fn check_allowlist(command: &str, config: &BashConfig) -> ToolResult<()> {
    if config.unrestricted {
        return Ok(());
    }
    for segment in split_segments(command) {
        check_segment(&segment, config)?;
    }
    Ok(())
}

/// Allowlist-check a single command segment (no unchecked control operators).
fn check_segment(segment: &str, config: &BashConfig) -> ToolResult<()> {
    let tokens = shlex::split(segment)
        .ok_or_else(|| ToolError::new(format!("Could not parse command: {segment:?}")))?;
    let executable = tokens.first().cloned().unwrap_or_default();
    // strip any path prefix so "./pytest" matches "pytest*"
    let exe_name = Path::new(&executable)
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| executable.clone());

    // `cd` is a shell builtin (never an executable on PATH), so the pattern
    // allowlist can't match it — yet the agent needs it to build projects nested
    // in a subdirectory (`cd sub && cargo build`). Permit it only with a relative
    // target: an absolute path or `~` would point outside the sandboxed repo. A
    // bare `cd` (→ $HOME) is likewise rejected.
    if exe_name == "cd" {
        let target = tokens.get(1).map(String::as_str).unwrap_or("");
        if target.is_empty() || target.starts_with('/') || target.starts_with('~') {
            return Err(ToolError::new(format!(
                "cd is only allowed with a relative path inside the repo (got {target:?})"
            )));
        }
        return Ok(());
    }

    for pattern in &config.allowed {
        if fnmatch(segment, pattern) || fnmatch(&exe_name, pattern) {
            return Ok(());
        }
    }
    // Models reach for coreutils to look around; the file tools do that job.
    let hint = match exe_name.as_str() {
        "ls" | "find" | "tree" => "\nTo list files use the `list_files` tool.",
        "cat" | "head" | "tail" | "less" | "sed" | "awk" | "wc" => {
            "\nTo read a file (or a line range) use the `read_file` tool."
        }
        "grep" | "rg" | "ag" => "\nTo search code use the `search_code` tool.",
        "echo" | "printf" | "mkdir" | "touch" | "tee" => {
            "\nTo create files use the `write_file` tool (it creates parent directories)."
        }
        _ => "",
    };
    Err(ToolError::new(format!(
        "Command not in allowlist: {executable:?}\nAllowed patterns: {:?}{hint}",
        config.allowed
    )))
}

pub fn run_bash(command: &str, repo_root: &Path, config: &BashConfig) -> ToolResult<String> {
    run_bash_with(
        command,
        repo_root,
        config,
        config.timeout_secs.unwrap_or(DEFAULT_TIMEOUT_SECS),
        DEFAULT_MAX_OUTPUT,
    )
}

pub fn run_bash_with(
    command: &str,
    repo_root: &Path,
    config: &BashConfig,
    timeout_secs: u64,
    max_output: usize,
) -> ToolResult<String> {
    check_blocked(command)?;
    check_network(command)?;
    check_allowlist(command, config)?;
    run_confined(
        command,
        repo_root,
        timeout_secs,
        max_output,
        &Confinement::from_bash(config),
    )
}
