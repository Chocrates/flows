//! Repo context for agent prompts. Port of `_build_tree` from
//! `flows/phases/agent.py` — a gitignore-respecting file tree the agent sees in
//! its system prompt (so it has repo context without a read_file round-trip).

use std::path::Path;

use ignore::WalkBuilder;

/// Directory names always skipped, even if not gitignored (matches the Python
/// default excludes).
pub(crate) const DENY: &[&str] = &[
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
        lines.push(format!("{indent}{name}{}", if is_dir { "/" } else { "" }));
        count += 1;
    }

    lines.join("\n")
}

/// A slimmer tree for small-context models: gitignore-aware like
/// [`build_repo_tree`] but depth-bounded (entries deeper than `max_depth` are
/// omitted) and tightly capped. Past the cap it stops with a neutral "not shown"
/// note rather than a long truncated flat list, so the model spends tokens on
/// layout. Navigation tools (`list_files`, `search_code`) are documented in the
/// prompt's tools section — deliberately not named here, so a no-tool model's
/// tree carries no tool references.
pub fn build_slim_tree(repo_root: &Path, max_files: usize, max_depth: usize) -> String {
    let mut lines = Vec::new();
    let mut count = 0;
    let mut clipped_depth = false;

    let walk = WalkBuilder::new(repo_root)
        .hidden(false)
        .git_ignore(true)
        .git_global(false)
        .require_git(false)
        .sort_by_file_path(|a, b| a.cmp(b))
        .build();

    for entry in walk.flatten() {
        if entry.depth() == 0 {
            continue;
        }
        let rel = entry.path().strip_prefix(repo_root).unwrap_or(entry.path());
        if rel
            .components()
            .any(|c| DENY.contains(&c.as_os_str().to_string_lossy().as_ref()))
        {
            continue;
        }
        let depth = rel.components().count().saturating_sub(1);
        if depth > max_depth {
            clipped_depth = true;
            continue;
        }
        if count >= max_files {
            lines.push(format!(
                "... (tree capped at {max_files} entries — some directories not shown)"
            ));
            break;
        }
        let indent = "  ".repeat(depth);
        let is_dir = entry.file_type().map(|t| t.is_dir()).unwrap_or(false);
        let name = entry.file_name().to_string_lossy();
        lines.push(format!("{indent}{name}{}", if is_dir { "/" } else { "" }));
        count += 1;
    }

    if clipped_depth {
        lines.push(format!(
            "... (showing depth ≤{max_depth} — deeper entries not shown)"
        ));
    }
    lines.join("\n")
}
