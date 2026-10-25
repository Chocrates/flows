//! Template registry: install templates from git, pinned to a commit, after a
//! review; manage them via a lockfile; and mark phases of untrusted installed
//! templates so the runner confines them.
//!
//! Source syntax: `<git-url>[//<path>][@<ref>]`. `<path>` names a `.yml`
//! template or a directory holding exactly one; prompts come from
//! `prompts/<stem>/*.md` beside it. Installed to `<flows>/templates/<name>.yml`
//! and `<flows>/prompts/<name>/`, recorded in `<flows>/templates/registry.lock`.
//!
//! A template is code: script phases run shell commands and agent phases pick
//! tool permissions. Untrusted installed templates therefore get their script
//! phases sandboxed and cannot expose credentials (see [`mark_untrusted`]).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde::{Deserialize, Serialize};
use serde_yaml_ng::Value;

use crate::models::PhaseConfig;

/// Max size of one template or prompt file.
const MAX_FILE_BYTES: u64 = 256 * 1024;

// ---------------------------------------------------------------------------
// Sources
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Source {
    pub url: String,
    /// Path inside the repo ("" = repo root).
    pub path: String,
    /// Branch, tag or commit; `None` = the remote's default branch.
    pub git_ref: Option<String>,
}

impl Source {
    /// Parse `<url>[//<path>][@<ref>]`. `@` only starts a ref after the last
    /// `/`, so `git@host:org/repo` stays a URL.
    pub fn parse(s: &str) -> Result<Source, String> {
        let s = s.trim();
        if s.is_empty() {
            return Err("empty template source".into());
        }
        let last_slash = s.rfind('/').unwrap_or(0);
        let (rest, git_ref) = match s.rfind('@') {
            Some(at) if at > last_slash => (&s[..at], Some(s[at + 1..].to_string())),
            _ => (s, None),
        };
        if git_ref.as_deref() == Some("") {
            return Err(format!("empty ref in {s:?}"));
        }
        // `//` after the scheme separates the repo from the path inside it.
        let search_from = rest.find("://").map(|i| i + 3).unwrap_or(0);
        let (url, path) = match rest[search_from..].find("//") {
            Some(i) => {
                let i = search_from + i;
                (&rest[..i], rest[i + 2..].trim_matches('/'))
            }
            None => (rest, ""),
        };
        if url.is_empty() {
            return Err(format!("no repository in {s:?}"));
        }
        if path.split('/').any(|seg| seg == "..") {
            return Err(format!("path {path:?} may not contain `..`"));
        }
        Ok(Source {
            url: url.to_string(),
            path: path.to_string(),
            git_ref,
        })
    }

    pub fn display(&self) -> String {
        let mut s = self.url.clone();
        if !self.path.is_empty() {
            s.push_str("//");
            s.push_str(&self.path);
        }
        if let Some(r) = &self.git_ref {
            s.push('@');
            s.push_str(r);
        }
        s
    }
}

// ---------------------------------------------------------------------------
// Fetching
// ---------------------------------------------------------------------------

/// A fetched template: its text, prompts and the commit it came from.
#[derive(Debug, Clone)]
pub struct Package {
    pub commit: String,
    /// File stem of the template (the default install name).
    pub stem: String,
    pub template: String,
    /// `(file name, text)` of each prompt, sorted.
    pub prompts: Vec<(String, String)>,
}

impl Package {
    /// Files as installed, relative to the flows dir, with their contents.
    fn files(&self, name: &str) -> Vec<(String, &str)> {
        let mut out = vec![(format!("templates/{name}.yml"), self.template.as_str())];
        for (f, text) in &self.prompts {
            out.push((format!("prompts/{name}/{f}"), text.as_str()));
        }
        out
    }
}

fn git(dir: Option<&Path>, args: &[&str]) -> Result<String, String> {
    let mut cmd = Command::new("git");
    // Nothing from the clone may execute: no hooks, no ext:: transport, no
    // credential prompts on the terminal.
    cmd.args([
        "-c",
        "core.hooksPath=/dev/null",
        "-c",
        "protocol.ext.allow=never",
    ])
    .env("GIT_TERMINAL_PROMPT", "0");
    if let Some(d) = dir {
        cmd.arg("-C").arg(d);
    }
    let out = cmd.args(args).output().map_err(|e| format!("git: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "git {}: {}",
            args.first().unwrap_or(&""),
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// Clone `src`, pin its ref to a commit, and read the template package.
pub fn fetch(src: &Source) -> Result<Package, String> {
    let tmp = tempfile::Builder::new()
        .prefix("flows-template-")
        .tempdir()
        .map_err(|e| e.to_string())?;
    let dir = tmp.path().join("repo");
    let dir_s = dir.to_string_lossy().into_owned();
    git(
        None,
        &["clone", "--quiet", "--no-checkout", &src.url, &dir_s],
    )?;
    let want = src.git_ref.as_deref().unwrap_or("HEAD");
    let commit = git(
        Some(&dir),
        &["rev-parse", "--verify", &format!("{want}^{{commit}}")],
    )
    .or_else(|_| {
        git(
            Some(&dir),
            &[
                "rev-parse",
                "--verify",
                &format!("origin/{want}^{{commit}}"),
            ],
        )
    })
    .map_err(|_| format!("ref {want:?} not found in {}", src.url))?;
    git(Some(&dir), &["checkout", "--quiet", "--detach", &commit])?;
    read_package(&dir, &src.path, commit)
}

fn read_small(path: &Path) -> Result<String, String> {
    let meta = std::fs::symlink_metadata(path).map_err(|e| format!("{}: {e}", path.display()))?;
    if !meta.is_file() {
        return Err(format!("{} is not a regular file", path.display()));
    }
    if meta.len() > MAX_FILE_BYTES {
        return Err(format!(
            "{} is larger than {MAX_FILE_BYTES} bytes",
            path.display()
        ));
    }
    std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))
}

fn valid_name(n: &str) -> bool {
    !n.is_empty()
        && n.chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// Locate and read the template under `root/path` (no symlinks, size-capped).
pub fn read_package(root: &Path, path: &str, commit: String) -> Result<Package, String> {
    let target = root.join(path);
    let meta = std::fs::symlink_metadata(&target)
        .map_err(|_| format!("path {path:?} not found in the repository"))?;
    let file = if meta.is_dir() {
        let mut ymls: Vec<PathBuf> = std::fs::read_dir(&target)
            .map_err(|e| e.to_string())?
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|e| e == "yml" || e == "yaml"))
            .collect();
        ymls.sort();
        match ymls.len() {
            1 => ymls.remove(0),
            0 => return Err(format!("no .yml template in {:?}", display_rel(path))),
            _ => {
                let names: Vec<String> = ymls
                    .iter()
                    .filter_map(|p| p.file_name().map(|f| f.to_string_lossy().into_owned()))
                    .collect();
                return Err(format!(
                    "{:?} holds several templates ({}); add `//<path>/<file>.yml` to pick one",
                    display_rel(path),
                    names.join(", ")
                ));
            }
        }
    } else {
        target
    };
    let stem = file
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    if !valid_name(&stem) {
        return Err(format!(
            "template name {stem:?} must be letters, digits, - or _"
        ));
    }
    let template = read_small(&file)?;
    let parsed: Value =
        serde_yaml_ng::from_str(&template).map_err(|e| format!("{stem}.yml: {e}"))?;
    if !matches!(parsed.get("phases"), Some(Value::Sequence(_))) {
        return Err(format!("{stem}.yml has no `phases:` list"));
    }

    let mut prompts = Vec::new();
    let pdir = file.parent().unwrap_or(root).join("prompts").join(&stem);
    if std::fs::symlink_metadata(&pdir).is_ok_and(|m| m.is_dir()) {
        for e in std::fs::read_dir(&pdir)
            .map_err(|e| e.to_string())?
            .flatten()
        {
            let fname = e.file_name().to_string_lossy().into_owned();
            let Some(base) = fname.strip_suffix(".md") else {
                continue;
            };
            if !valid_name(base) {
                return Err(format!("prompt file name {fname:?} is not allowed"));
            }
            prompts.push((fname, read_small(&e.path())?));
        }
    }
    prompts.sort();
    Ok(Package {
        commit,
        stem,
        template,
        prompts,
    })
}

fn display_rel(path: &str) -> &str {
    if path.is_empty() {
        "(repository root)"
    } else {
        path
    }
}

// ---------------------------------------------------------------------------
// Review
// ---------------------------------------------------------------------------

fn yaml_inline(v: &Value) -> String {
    serde_json::to_string(v).unwrap_or_default()
}

/// Human review of what a package will be able to do, shown before install.
pub fn review(src: &Source, pkg: &Package, name: &str, trusted: bool) -> String {
    let mut out = format!(
        "Template {name}\n  source: {}\n  commit: {}\n\nPhases:\n",
        src.display(),
        pkg.commit
    );
    let root: Value = serde_yaml_ng::from_str(&pkg.template).unwrap_or(Value::Null);
    let phases = root
        .get("phases")
        .and_then(Value::as_sequence)
        .cloned()
        .unwrap_or_default();
    let mut shell = Vec::new();
    for (i, p) in phases.iter().enumerate() {
        let n = i + 1;
        if let Some(inc) = p.get("include").and_then(Value::as_str) {
            let alias = p
                .get("as")
                .and_then(Value::as_str)
                .map(|a| format!(" as {a}"))
                .unwrap_or_default();
            out.push_str(&format!("  {n}. include: {inc}{alias}\n"));
            continue;
        }
        let pname = p.get("name").and_then(Value::as_str).unwrap_or("?");
        let ptype = p.get("type").and_then(Value::as_str).unwrap_or("agent");
        out.push_str(&format!("  {n}. {pname} [{ptype}]"));
        if ptype == "script" {
            let cmd = p.get("command").and_then(Value::as_str).unwrap_or("");
            out.push_str(&format!("\n       $ {cmd}"));
        }
        match p.get("tools") {
            Some(Value::String(preset)) => out.push_str(&format!("  tools: {preset}")),
            Some(t @ Value::Mapping(_)) => {
                if let Some(b) = t.get("bash").filter(|b| !b.is_null()) {
                    shell.push(format!("  {pname}: bash {}", yaml_inline(b)));
                    for flag in ["unrestricted", "expose_credentials"] {
                        if b.get(flag).and_then(Value::as_bool) == Some(true) {
                            shell.push(format!("    ! {flag}: true"));
                        }
                    }
                }
                let on: Vec<&str> = t
                    .as_mapping()
                    .into_iter()
                    .flatten()
                    .filter(|(_, v)| !matches!(v, Value::Bool(false) | Value::Null))
                    .filter_map(|(k, _)| k.as_str())
                    .collect();
                out.push_str(&format!("  tools: {}", on.join(", ")));
            }
            _ => {}
        }
        if let Some(w) = p.get("when").and_then(Value::as_str) {
            out.push_str(&format!("  when: {w}"));
        }
        out.push('\n');
    }
    if !shell.is_empty() {
        out.push_str("\nShell access:\n");
        out.push_str(&shell.join("\n"));
        out.push('\n');
    }
    out.push_str("\nPrompts: ");
    if pkg.prompts.is_empty() {
        out.push_str("(none — builtin/generic prompts will be used)\n");
    } else {
        let names: Vec<&str> = pkg.prompts.iter().map(|(f, _)| f.as_str()).collect();
        out.push_str(&names.join(", "));
        out.push('\n');
    }
    out.push('\n');
    if trusted {
        out.push_str(
            "TRUSTED: script phases will run directly on this machine with your\n\
             permissions, and agent phases may expose credential stores.\n",
        );
    } else {
        out.push_str(
            "Untrusted: script phases will run in the shell sandbox (writes limited to\n\
             the project, credentials hidden), and `expose_credentials` is ignored.\n\
             Trust it later with `flows templates trust <name>` after reviewing it.\n",
        );
    }
    out
}

// ---------------------------------------------------------------------------
// Lockfile
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LockEntry {
    pub source: String,
    #[serde(default)]
    pub path: String,
    #[serde(rename = "ref", default, skip_serializing_if = "Option::is_none")]
    pub git_ref: Option<String>,
    pub commit: String,
    /// Installed file (relative to the flows dir) → blake3 of its contents.
    #[serde(default)]
    pub files: BTreeMap<String, String>,
    #[serde(default)]
    pub trusted: bool,
    #[serde(default)]
    pub installed_at: String,
}

impl LockEntry {
    pub fn source(&self) -> Source {
        Source {
            url: self.source.clone(),
            path: self.path.clone(),
            git_ref: self.git_ref.clone(),
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Lock {
    #[serde(default)]
    pub templates: BTreeMap<String, LockEntry>,
}

fn lock_path(flows_dir: &Path) -> PathBuf {
    flows_dir.join("templates").join("registry.lock")
}

/// The lockfile; missing or unreadable → empty (nothing installed).
pub fn read_lock(flows_dir: &Path) -> Lock {
    std::fs::read(lock_path(flows_dir))
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default()
}

fn write_lock(flows_dir: &Path, lock: &Lock) -> Result<(), String> {
    let path = lock_path(flows_dir);
    std::fs::create_dir_all(path.parent().unwrap()).map_err(|e| e.to_string())?;
    let tmp = path.with_extension("lock.tmp");
    let bytes = serde_json::to_vec_pretty(lock).map_err(|e| e.to_string())?;
    std::fs::write(&tmp, bytes).map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, &path).map_err(|e| e.to_string())
}

// ---------------------------------------------------------------------------
// Install / update / remove / trust
// ---------------------------------------------------------------------------

/// Whether `name` may be installed fresh: a valid name, not a builtin, not an
/// existing global template the registry doesn't own, not already installed.
pub fn check_new_name(flows_dir: &Path, name: &str) -> Result<(), String> {
    if !valid_name(name) {
        return Err(format!(
            "template name {name:?} must be letters, digits, - or _"
        ));
    }
    if read_lock(flows_dir).templates.contains_key(name) {
        return Err(format!(
            "{name:?} is already installed; use `flows templates update {name}`"
        ));
    }
    if crate::templates::builtin_files()
        .iter()
        .any(|(f, _)| f == &format!("{name}.yml"))
    {
        return Err(format!(
            "{name:?} is a builtin template; install under another name with --as <name>"
        ));
    }
    if flows_dir
        .join("templates")
        .join(format!("{name}.yml"))
        .exists()
    {
        return Err(format!(
            "~/.flows/templates/{name}.yml already exists; install under another name with --as <name>"
        ));
    }
    Ok(())
}

fn hash(text: &str) -> String {
    blake3::hash(text.as_bytes()).to_hex().to_string()
}

/// Write the package as `name` and record it. Replaces a previous install of
/// the same name (its prompt dir is cleared first).
pub fn install(
    flows_dir: &Path,
    src: &Source,
    pkg: &Package,
    name: &str,
    trusted: bool,
) -> Result<LockEntry, String> {
    let pdir = flows_dir.join("prompts").join(name);
    if pdir.exists() {
        std::fs::remove_dir_all(&pdir).map_err(|e| e.to_string())?;
    }
    let mut files = BTreeMap::new();
    for (rel, text) in pkg.files(name) {
        let path = flows_dir.join(&rel);
        std::fs::create_dir_all(path.parent().unwrap()).map_err(|e| e.to_string())?;
        std::fs::write(&path, text).map_err(|e| format!("{}: {e}", path.display()))?;
        files.insert(rel, hash(text));
    }
    let entry = LockEntry {
        source: src.url.clone(),
        path: src.path.clone(),
        git_ref: src.git_ref.clone(),
        commit: pkg.commit.clone(),
        files,
        trusted,
        installed_at: chrono::Utc::now().to_rfc3339(),
    };
    let mut lock = read_lock(flows_dir);
    lock.templates.insert(name.to_string(), entry.clone());
    write_lock(flows_dir, &lock)?;
    Ok(entry)
}

/// Delete an installed template's files and lock entry.
pub fn remove(flows_dir: &Path, name: &str) -> Result<(), String> {
    let mut lock = read_lock(flows_dir);
    let Some(entry) = lock.templates.remove(name) else {
        return Err(format!("{name:?} is not an installed template"));
    };
    for rel in entry.files.keys() {
        let _ = std::fs::remove_file(flows_dir.join(rel));
    }
    let _ = std::fs::remove_dir_all(flows_dir.join("prompts").join(name));
    write_lock(flows_dir, &lock)
}

pub fn set_trust(flows_dir: &Path, name: &str, trusted: bool) -> Result<(), String> {
    let mut lock = read_lock(flows_dir);
    let Some(entry) = lock.templates.get_mut(name) else {
        return Err(format!("{name:?} is not an installed template"));
    };
    entry.trusted = trusted;
    write_lock(flows_dir, &lock)
}

/// Line diff of what `update` would change: per changed file a `-`/`+` diff,
/// plus added and removed files. Empty when nothing changes.
pub fn update_diff(flows_dir: &Path, name: &str, pkg: &Package) -> String {
    let lock = read_lock(flows_dir);
    let old_files: Vec<String> = lock
        .templates
        .get(name)
        .map(|e| e.files.keys().cloned().collect())
        .unwrap_or_default();
    let new_files = pkg.files(name);
    let mut out = String::new();
    for (rel, text) in &new_files {
        let old = std::fs::read_to_string(flows_dir.join(rel)).ok();
        match old {
            None => out.push_str(&format!("+++ new file {rel}\n")),
            Some(o) if o == *text => {}
            Some(o) => {
                out.push_str(&format!("--- {rel}\n+++ {rel}\n"));
                out.push_str(&line_diff(&o, text));
            }
        }
    }
    for rel in old_files {
        if !new_files.iter().any(|(r, _)| *r == rel) {
            out.push_str(&format!("--- removed {rel}\n"));
        }
    }
    out
}

/// Minimal LCS line diff (`-` old, `+` new, changed lines only).
fn line_diff(old: &str, new: &str) -> String {
    let a: Vec<&str> = old.lines().collect();
    let b: Vec<&str> = new.lines().collect();
    let (n, m) = (a.len(), b.len());
    let mut lcs = vec![vec![0u32; m + 1]; n + 1];
    for i in (0..n).rev() {
        for j in (0..m).rev() {
            lcs[i][j] = if a[i] == b[j] {
                lcs[i + 1][j + 1] + 1
            } else {
                lcs[i + 1][j].max(lcs[i][j + 1])
            };
        }
    }
    let (mut i, mut j, mut out) = (0, 0, String::new());
    while i < n || j < m {
        if i < n && j < m && a[i] == b[j] {
            i += 1;
            j += 1;
        } else if j < m && (i == n || lcs[i][j + 1] >= lcs[i + 1][j]) {
            out.push_str(&format!("+{}\n", b[j]));
            j += 1;
        } else {
            out.push_str(&format!("-{}\n", a[i]));
            i += 1;
        }
    }
    out
}

// ---------------------------------------------------------------------------
// Loading
// ---------------------------------------------------------------------------

/// Mark phases defined by untrusted installed templates: `untrusted = true`
/// (script phases run sandboxed) and `expose_credentials` cleared. A phase's
/// defining template is its include origin, else `root` (the loaded template).
/// `installed_untrusted(name)` says whether that template resolved to an
/// untrusted registry install.
pub fn mark_untrusted(
    phases: &mut [PhaseConfig],
    root: Option<&str>,
    installed_untrusted: &dyn Fn(&str) -> bool,
) {
    for p in phases {
        let defining = p.origin.as_ref().map(|o| o.template.as_str()).or(root);
        if defining.is_some_and(installed_untrusted) {
            p.untrusted = true;
            if let Some(b) = &mut p.tools.bash {
                b.expose_credentials = false;
            }
        }
        // A fan-out group's nested phases are defined by the same template.
        mark_untrusted(&mut p.phases, defining, installed_untrusted);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_sources() {
        let s = Source::parse("https://github.com/acme/t//ship@v1.2").unwrap();
        assert_eq!(
            (s.url.as_str(), s.path.as_str(), s.git_ref.as_deref()),
            ("https://github.com/acme/t", "ship", Some("v1.2"))
        );
        let s = Source::parse("git@github.com:acme/t.git").unwrap();
        assert_eq!(s.url, "git@github.com:acme/t.git");
        assert_eq!((s.path.as_str(), s.git_ref), ("", None));
        let s = Source::parse("/tmp/repo//a/b.yml").unwrap();
        assert_eq!((s.url.as_str(), s.path.as_str()), ("/tmp/repo", "a/b.yml"));
        assert!(Source::parse("https://x/y//../etc").is_err());
        assert!(Source::parse("https://x/y@").is_err());
        assert_eq!(
            Source::parse("https://x/y//p@r").unwrap().display(),
            "https://x/y//p@r"
        );
    }

    #[test]
    fn line_diff_marks_changes() {
        assert_eq!(line_diff("a\nb\nc\n", "a\nB\nc\nd\n"), "+B\n-b\n+d\n");
        assert_eq!(line_diff("x\n", "x\n"), "");
    }
}
