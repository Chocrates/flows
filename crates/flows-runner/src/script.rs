//! Script phase — runs a user-authored shell command (trusted, not sandboxed),
//! captures output. Port of `flows/phases/script.py`.
//!
//! Also resolves the `@auto:test` / `@auto:lint` sentinels: pick the project's
//! test/lint command in pure Rust (no LLM, no tokens) — preferring a `make`
//! target, else a hardcoded command for the common languages. Resolution
//! happens at execute time, when the repo is actually populated.

use std::path::Path;
use std::process::Command;

use minijinja::{context, Environment};

use flows_core::models::{PhaseConfig, ScriptResult};

/// Which `@auto:*` command to resolve.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum AutoKind {
    Test,
    Lint,
}

impl AutoKind {
    fn target(self) -> &'static str {
        match self {
            AutoKind::Test => "test",
            AutoKind::Lint => "lint",
        }
    }
}

/// Recognize a script command as an `@auto:test` / `@auto:lint` sentinel.
pub fn auto_kind(command: &str) -> Option<AutoKind> {
    match command.trim() {
        "@auto:test" => Some(AutoKind::Test),
        "@auto:lint" => Some(AutoKind::Lint),
        _ => None,
    }
}

fn exists(root: &Path, name: &str) -> bool {
    root.join(name).exists()
}

/// True if a `Makefile` (any case) declares a `<target>:` rule.
fn makefile_has_target(root: &Path, target: &str) -> bool {
    for mk in ["Makefile", "makefile", "GNUmakefile"] {
        if let Ok(text) = std::fs::read_to_string(root.join(mk)) {
            let needle = format!("{target}:");
            if text.lines().any(|l| l.trim_start().starts_with(&needle)) {
                return true;
            }
        }
    }
    false
}

/// True if `package.json` defines a `scripts.<name>` entry.
fn npm_script(root: &Path, name: &str) -> bool {
    std::fs::read_to_string(root.join("package.json"))
        .ok()
        .and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok())
        .and_then(|v| v.get("scripts").and_then(|s| s.get(name)).cloned())
        .is_some()
}

fn python_project(root: &Path) -> bool {
    exists(root, "pyproject.toml")
        || exists(root, "setup.py")
        || exists(root, "setup.cfg")
        || exists(root, "requirements.txt")
        || exists(root, "pytest.ini")
        || root.join("tests").is_dir()
}

/// Hardcoded test/lint commands for the common languages (no config needed).
fn detect_command(kind: AutoKind, root: &Path) -> Option<String> {
    let test = kind == AutoKind::Test;

    // Rust
    if exists(root, "Cargo.toml") {
        return Some(
            if test {
                "cargo test"
            } else {
                "cargo clippy -- -D warnings"
            }
            .into(),
        );
    }
    // Go
    if exists(root, "go.mod") {
        return Some(
            if test {
                "go test ./..."
            } else {
                "go vet ./..."
            }
            .into(),
        );
    }
    // Node / JS / TS
    if exists(root, "package.json") {
        let name = if test { "test" } else { "lint" };
        return npm_script(root, name).then(|| format!("npm run {name}"));
    }
    // Python
    if python_project(root) {
        if test {
            return Some("pytest".into());
        }
        // lint: only if a linter is actually configured, else fall through to None
        let cfg = std::fs::read_to_string(root.join("pyproject.toml")).unwrap_or_default();
        if cfg.contains("ruff") {
            return Some("ruff check .".into());
        }
        if cfg.contains("flake8") || exists(root, ".flake8") {
            return Some("flake8 .".into());
        }
        return None;
    }
    // Ruby
    if exists(root, "Gemfile") {
        return Some(
            if test {
                "bundle exec rake test"
            } else {
                "bundle exec rubocop"
            }
            .into(),
        );
    }
    // Java (Maven / Gradle) — test only; no standard lint
    if exists(root, "pom.xml") {
        return test.then(|| "mvn -q test".into());
    }
    if exists(root, "build.gradle") || exists(root, "build.gradle.kts") {
        return test.then(|| "./gradlew test".into());
    }
    None
}

/// Resolve `@auto:test` / `@auto:lint` to a concrete command, or `Err` (which the
/// runner treats as a hard stop — an undetectable tool is an environment problem,
/// not a retryable test failure). Prefers a `make` target.
pub fn resolve_auto(kind: AutoKind, repo_root: &Path) -> Result<String, String> {
    let target = kind.target();
    if makefile_has_target(repo_root, target) {
        return Ok(format!("make {target}"));
    }
    detect_command(kind, repo_root).ok_or_else(|| {
        format!(
            "@auto:{target}: could not detect a {target} tool for this project. \
Add a `{target}:` target to a Makefile, or set an explicit `{target}` script \
command in .flows.yml."
        )
    })
}

pub fn run_script(
    phase: &PhaseConfig,
    repo_root: &Path,
    idea: &str,
) -> Result<ScriptResult, String> {
    let command = phase
        .command
        .as_deref()
        .filter(|c| !c.is_empty())
        .ok_or_else(|| format!("Script phase {:?} has no command", phase.name))?;

    let env = Environment::new();
    let rendered = env
        .render_str(command, context! { idea => idea })
        .map_err(|e| format!("command render failed: {e}"))?;

    let output = Command::new("/bin/sh")
        .arg("-c")
        .arg(&rendered)
        .current_dir(repo_root)
        .output()
        .map_err(|e| e.to_string())?;

    Ok(ScriptResult {
        command: rendered,
        exit_code: output.status.code().unwrap_or(-1),
        stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
    })
}
