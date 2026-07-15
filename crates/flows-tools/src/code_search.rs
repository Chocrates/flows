//! Repo-scoped code search — a gitignore-aware regex grep with bounded output.
//!
//! Gives a model a way to *find* code by pattern (`search_code`) without pulling
//! whole files into context. Traversal respects `.gitignore` and skips heavy
//! vendor dirs (same deny-list as `repo_context`), the scope is guarded by the
//! same repo-root check as the file tools, and the result is capped so a broad
//! pattern can't flood the context window.

use std::path::Path;

use ignore::WalkBuilder;
use regex::Regex;

use crate::error::{ToolError, ToolResult};
use crate::files::safe_path;

/// Directory names always skipped, even if not gitignored (mirrors
/// `repo_context::DENY`).
const DENY: &[&str] = &[
    ".git",
    ".venv",
    "node_modules",
    "__pycache__",
    "target",
    "dist",
    "build",
    ".mypy_cache",
    ".pytest_cache",
];

/// Default match cap when the caller passes `0`.
const DEFAULT_CAP: usize = 100;
/// Longest match line rendered before it is clipped (chars, not bytes).
const MAX_LINE_CHARS: usize = 300;
/// Files larger than this are skipped (bytes) — a match in a multi-MB generated
/// file is rarely what the model wants and reading it is slow.
const MAX_FILE_BYTES: u64 = 2_000_000;

/// Search `pattern` (a regex) across repo files under `scope` — a repo-relative
/// subdirectory, or the whole repo when empty/`.`/`/`. Returns up to `cap`
/// matches as `path:line: text`, with a truncation indicator when the cap is
/// hit. An empty result is the explicit string `No matches ...` (not an error).
/// Refuses a `scope` that resolves outside the repo root.
pub fn search_code(repo_root: &Path, pattern: &str, scope: &str, cap: usize) -> ToolResult<String> {
    if pattern.is_empty() {
        return Err(ToolError::new("search_code requires a non-empty pattern"));
    }
    let re = Regex::new(pattern)
        .map_err(|e| ToolError::new(format!("Invalid regex {pattern:?}: {e}")))?;

    // Normalize the scope the same way `list_files` does, then run it through the
    // repo-root guard so `../` / absolute paths are refused.
    let trimmed = scope.trim();
    let rel = if trimmed.is_empty() || trimmed == "." || trimmed == "/" {
        "."
    } else {
        trimmed.trim_start_matches('/')
    };
    let root = safe_path(repo_root, rel)?;
    if !root.exists() {
        return Err(ToolError::new(format!("Scope not found: {rel}")));
    }

    let cap = if cap == 0 { DEFAULT_CAP } else { cap };
    let mut out: Vec<String> = Vec::new();
    let mut truncated = false;

    let walk = WalkBuilder::new(&root)
        .hidden(false) // show dotfiles; DENY + gitignore handle the noise
        .git_ignore(true)
        .git_global(false)
        .require_git(false) // honor .gitignore even outside an initialized repo
        .sort_by_file_path(|a, b| a.cmp(b))
        .build();

    'files: for entry in walk.flatten() {
        let path = entry.path();
        let relp = path.strip_prefix(repo_root).unwrap_or(path);
        if relp
            .components()
            .any(|c| DENY.contains(&c.as_os_str().to_string_lossy().as_ref()))
        {
            continue;
        }
        if !entry.file_type().map(|t| t.is_file()).unwrap_or(false) {
            continue;
        }
        if entry
            .metadata()
            .map(|m| m.len() > MAX_FILE_BYTES)
            .unwrap_or(false)
        {
            continue;
        }
        // Skip binary / non-UTF-8 files silently rather than erroring the search.
        let Ok(content) = std::fs::read_to_string(path) else {
            continue;
        };
        for (i, line) in content.lines().enumerate() {
            if re.is_match(line) {
                if out.len() >= cap {
                    truncated = true;
                    break 'files;
                }
                out.push(format!(
                    "{}:{}: {}",
                    rel_display(repo_root, path),
                    i + 1,
                    clip(line.trim_end())
                ));
            }
        }
    }

    if out.is_empty() {
        return Ok(format!("No matches for {pattern:?} under {rel:?}"));
    }
    let mut result = out.join("\n");
    if truncated {
        result.push_str(&format!(
            "\n... [truncated at {cap} matches — narrow the pattern or scope]"
        ));
    }
    Ok(result)
}

/// Clip a match line to `MAX_LINE_CHARS` on a char boundary.
fn clip(line: &str) -> String {
    if line.chars().count() > MAX_LINE_CHARS {
        let s: String = line.chars().take(MAX_LINE_CHARS).collect();
        format!("{s}…")
    } else {
        line.to_string()
    }
}

fn rel_display(repo_root: &Path, p: &Path) -> String {
    p.strip_prefix(repo_root)
        .unwrap_or(p)
        .to_string_lossy()
        .into_owned()
}
