//! Port of `flows/tools/files.py` — file tools scoped to the repo root.
//!
//! `safe_path` resolves `.`/`..` and follows symlinks on the existing portion
//! of the path, then requires the result to stay under the (canonicalized) repo
//! root — blocking `../` traversal, absolute-path escapes, and symlink escapes.

use std::ffi::OsString;
use std::path::{Component, Path, PathBuf};

use crate::error::{ToolError, ToolResult};

const DEFAULT_MAX_BYTES: usize = 200_000;

/// Lexically resolve `.` and `..` without touching the filesystem.
fn lexical_normalize(p: &Path) -> PathBuf {
    let mut out: Vec<Component> = Vec::new();
    for comp in p.components() {
        match comp {
            Component::CurDir => {}
            Component::ParentDir => {
                // don't pop the root/prefix
                if matches!(out.last(), Some(Component::Normal(_))) {
                    out.pop();
                } else {
                    out.push(comp);
                }
            }
            c => out.push(c),
        }
    }
    out.iter().map(|c| c.as_os_str()).collect()
}

/// Canonicalize the longest existing ancestor (following symlinks), then append
/// the non-existent remainder — a non-strict analogue of Python's `Path.resolve`.
fn real_path(target: &Path) -> PathBuf {
    let mut existing = target.to_path_buf();
    let mut tail: Vec<OsString> = Vec::new();
    loop {
        if existing.exists() {
            let mut result = existing.canonicalize().unwrap_or(existing.clone());
            for part in tail.iter().rev() {
                result.push(part);
            }
            return lexical_normalize(&result);
        }
        match existing.file_name() {
            Some(name) => {
                tail.push(name.to_os_string());
                if !existing.pop() {
                    break;
                }
            }
            None => break,
        }
    }
    lexical_normalize(target)
}

/// Resolve `user_path` under `repo_root`, or error if it escapes.
pub fn safe_path(repo_root: &Path, user_path: &str) -> ToolResult<PathBuf> {
    if user_path.is_empty() {
        return Err(ToolError::new("Path must not be empty"));
    }
    // Path::join replaces the base when user_path is absolute — matching Python's
    // `repo_root / user_path` (so "/etc/passwd" becomes an escape, caught below).
    let target = repo_root.join(user_path);
    let root_real = real_path(repo_root);
    let target_real = real_path(&target);
    if !target_real.starts_with(&root_real) {
        return Err(ToolError::new(format!(
            "Path escapes repo root: {user_path:?}"
        )));
    }
    Ok(lexical_normalize(&target))
}

/// `O_NOFOLLOW` on the final component: `safe_path` catches symlink escapes when
/// it resolves, but a symlink swapped in between check and open (TOCTOU) would
/// otherwise be followed. Refusing to open a symlinked target closes that race.
fn no_follow_error(path: &str) -> ToolError {
    ToolError::new(format!(
        "Refusing to follow a symlink at {path:?} (open the real file instead)"
    ))
}

pub fn read_file(repo_root: &Path, path: &str) -> ToolResult<String> {
    read_file_limited(repo_root, path, DEFAULT_MAX_BYTES)
}

pub fn read_file_limited(repo_root: &Path, path: &str, max_bytes: usize) -> ToolResult<String> {
    use std::io::Read;
    use std::os::unix::fs::OpenOptionsExt;

    let target = safe_path(repo_root, path)?;
    let mut file = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(&target)
        .map_err(|e| match e.raw_os_error() {
            Some(libc::ELOOP) => no_follow_error(path),
            _ if e.kind() == std::io::ErrorKind::NotFound => {
                ToolError::new(format!("File not found: {path}"))
            }
            _ => ToolError::new(format!("{e}")),
        })?;
    if file.metadata().map(|m| m.is_dir()).unwrap_or(false) {
        return Err(ToolError::new(format!("Not a file: {path}")));
    }
    // Read at most max_bytes + 1 so we can flag truncation.
    let mut raw = Vec::new();
    file.by_ref()
        .take(max_bytes as u64 + 1)
        .read_to_end(&mut raw)
        .map_err(|e| ToolError::new(format!("{e}")))?;
    let truncated = raw.len() > max_bytes;
    let slice = &raw[..raw.len().min(max_bytes)];
    let mut text = String::from_utf8_lossy(slice).into_owned();
    if truncated {
        text.push_str(&format!("\n... [truncated at {max_bytes} bytes]"));
    }
    Ok(text)
}

/// Read only lines `start_line..=end_line` (both 1-indexed, inclusive) of a
/// file. Labels the returned range and how many lines exist, and flags when
/// there are more lines after the window — so a model reading a slice knows
/// where it sits without pulling the whole file. Errors on empty/inverted
/// ranges or a start past end-of-file. Content is still byte-capped via
/// [`read_file_limited`].
pub fn read_file_range(
    repo_root: &Path,
    path: &str,
    start_line: usize,
    end_line: usize,
) -> ToolResult<String> {
    if start_line == 0 {
        return Err(ToolError::new(
            "start_line must be >= 1 (lines are 1-indexed)",
        ));
    }
    if end_line < start_line {
        return Err(ToolError::new(format!(
            "end_line ({end_line}) must be >= start_line ({start_line})"
        )));
    }
    let full = read_file_limited(repo_root, path, DEFAULT_MAX_BYTES)?;
    let lines: Vec<&str> = full.lines().collect();
    let total = lines.len();
    if start_line > total {
        return Err(ToolError::new(format!(
            "start_line {start_line} is past end of file ({total} lines)"
        )));
    }
    let end = end_line.min(total);
    let selected = lines[start_line - 1..end].join("\n");
    let mut text = format!("[lines {start_line}-{end} of {total}]\n{selected}");
    if end < total {
        text.push_str(&format!("\n... [{} more lines below]", total - end));
    }
    Ok(text)
}

pub fn write_file(repo_root: &Path, path: &str, content: &str) -> ToolResult<String> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;

    let target = safe_path(repo_root, path)?;
    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent).map_err(|e| ToolError::new(format!("{e}")))?;
    }
    // O_NOFOLLOW: never write *through* a symlink at the target path.
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(&target)
        .map_err(|e| match e.raw_os_error() {
            Some(libc::ELOOP) => no_follow_error(path),
            _ => ToolError::new(format!("{e}")),
        })?;
    file.write_all(content.as_bytes())
        .map_err(|e| ToolError::new(format!("{e}")))?;
    Ok(format!(
        "Written: {path} ({} chars)",
        content.chars().count()
    ))
}

pub fn make_dir(repo_root: &Path, path: &str) -> ToolResult<String> {
    let target = safe_path(repo_root, path)?;
    std::fs::create_dir_all(&target).map_err(|e| ToolError::new(format!("{e}")))?;
    Ok(format!("Created directory: {path}"))
}

pub fn list_files(repo_root: &Path, path: &str) -> ToolResult<String> {
    // The repo root, however the model spells it: models commonly send `""`,
    // `"."`, or `"/"` (absolute-looking) for "the root". Normalize all of these
    // to the root instead of erroring (`"/"` would otherwise escape the repo).
    let trimmed = path.trim();
    let path = if trimmed.is_empty() || trimmed == "/" || trimmed == "." {
        "."
    } else {
        // Strip a leading `/` so `"/src"` reads as the repo-relative `"src"`
        // rather than an absolute path that escapes the repo root.
        trimmed.trim_start_matches('/')
    };
    let target = safe_path(repo_root, path)?;
    if !target.exists() {
        return Err(ToolError::new(format!("Path not found: {path}")));
    }
    if target.is_file() {
        return Ok(rel(repo_root, &target));
    }
    let mut entries: Vec<PathBuf> = std::fs::read_dir(&target)
        .map_err(|e| ToolError::new(format!("{e}")))?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .collect();
    // sort by (is_file, name): dirs first, then files, each alphabetical
    entries.sort_by(|a, b| {
        (a.is_file(), a.file_name().map(|n| n.to_os_string()))
            .cmp(&(b.is_file(), b.file_name().map(|n| n.to_os_string())))
    });
    if entries.is_empty() {
        return Ok("(empty)".to_string());
    }
    let lines: Vec<String> = entries
        .iter()
        .map(|e| {
            let r = rel(repo_root, e);
            // Directories get a trailing `/`. (A leading one read as an absolute
            // path: models passed `/.flows` on to read_file, which rejects it.)
            if e.is_dir() {
                format!("{r}/")
            } else {
                r
            }
        })
        .collect();
    Ok(lines.join("\n"))
}

fn rel(repo_root: &Path, p: &Path) -> String {
    p.strip_prefix(repo_root)
        .unwrap_or(p)
        .to_string_lossy()
        .into_owned()
}
