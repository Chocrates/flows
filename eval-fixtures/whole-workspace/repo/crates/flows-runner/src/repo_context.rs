//! Repo context for agent prompts. Port of `_build_tree` from
//! `flows/phases/agent.py` — a gitignore-respecting file tree the agent sees in
//! its system prompt (so it has repo context without a read_file round-trip).

use std::path::Path;

use ignore::WalkBuilder;

/// Directory names always skipped, even if not gitignored (matches the Python
/// default excludes).
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

/// Build an indented file tree for `repo_root`, respecting `.gitignore` and
/// skipping heavy/vendor directories, capped at `max_files` entries.
pub fn build_repo_tree(repo_root: &Path, max_files: usize) -> String {
    let mut lines = Vec::new();
    let mut count = 0;

    let walk = WalkBuilder::new(repo_root)
        .hidden(false) // show dotfiles; DENY + gitignore handle the noise
        .git_ignore(true)
        .git_global(false)
        .sort_by_file_path(|a, b| a.cmp(b))
        .build();

    for entry in walk.flatten() {
        if entry.depth() == 0 {
            continue; // the root itself
        }
        let rel = entry.path().strip_prefix(repo_root).unwrap_or(entry.path());
        if rel
            .components()
            .any(|c| DENY.contains(&c.as_os_str().to_string_lossy().as_ref()))
        {
            continue;
        }
        if count >= max_files {
            lines.push("... (truncated)".to_string());
            break;
        }
        let depth = rel.components().count().saturating_sub(1);
        let indent = "  ".repeat(depth);
        let is_dir = entry.file_type().map(|t| t.is_dir()).unwrap_or(false);
        let name = entry.file_name().to_string_lossy();
        lines.push(format!("{indent}{}{name}", if is_dir { "/" } else { "" }));
        count += 1;
    }

    lines.join("\n")
}
