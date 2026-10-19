//! Bounds what a shell command may delete. An agent needs to delete scratch
//! files and files its change made obsolete, but a confused model will also
//! `rm -rf` the repository and rebuild it from memory (observed in an e2e run:
//! 33 files, 2480 lines gone, then committed). Filtering command text cannot
//! catch every way to delete a file, so the guard compares the working tree
//! before and after each command instead, and puts back what a command over the
//! limit removed.
//!
//! Snapshots are git tree objects written through a temporary copy of the
//! index: untracked files are included, ignored ones (build output) are not,
//! and the real index and working tree are never touched.

use std::collections::HashSet;
use std::path::Path;
use std::process::Command;

/// Most files present at phase start that one command may delete.
pub const MAX_PER_COMMAND: usize = 3;
/// Most files present at phase start that one phase may delete.
pub const MAX_PER_PHASE: usize = 10;

fn git(repo_root: &Path, index: Option<&Path>, args: &[&str]) -> Option<String> {
    let mut cmd = Command::new("git");
    cmd.args(args).current_dir(repo_root);
    if let Some(index) = index {
        cmd.env("GIT_INDEX_FILE", index);
    }
    let out = cmd.output().ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).trim_end().to_string())
}

/// A tree object holding the working tree under `repo_root` as it is now, or
/// `None` outside a git repository.
pub fn snapshot(repo_root: &Path) -> Option<String> {
    let real = git(repo_root, None, &["rev-parse", "--git-path", "index"])?;
    let real = repo_root.join(real);
    let tmp = TempIndex::new();
    // Starting from the real index keeps git's stat cache, so only changed
    // files are hashed. A repo with no commits yet has no index; start empty.
    if real.exists() {
        std::fs::copy(&real, &tmp.0).ok()?;
    }
    git(repo_root, Some(&tmp.0), &["add", "-A", "--", "."])?;
    git(repo_root, Some(&tmp.0), &["write-tree"])
}

/// A uniquely named index file in the temp dir, removed on drop.
struct TempIndex(std::path::PathBuf);

impl TempIndex {
    fn new() -> Self {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        Self(std::env::temp_dir().join(format!("flows-guard-index-{}-{n}", std::process::id())))
    }
}

impl Drop for TempIndex {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
        let _ = std::fs::remove_file(self.0.with_extension("lock"));
    }
}

/// Paths under `repo_root` (relative to it) present in tree `a` but not `b`.
pub fn deleted(repo_root: &Path, a: &str, b: &str) -> Vec<String> {
    git(
        repo_root,
        None,
        &[
            "diff-tree",
            "-r",
            "--no-renames",
            "--relative",
            "--name-only",
            "--diff-filter=D",
            a,
            b,
        ],
    )
    .map(|s| s.lines().map(str::to_string).collect())
    .unwrap_or_default()
}

/// Every path (relative to `repo_root`) in tree `tree`.
fn paths_in(repo_root: &Path, tree: &str) -> HashSet<String> {
    git(
        repo_root,
        None,
        &["ls-tree", "-r", "--name-only", "--full-tree", tree],
    )
    .map(|s| s.lines().map(str::to_string).collect())
    .unwrap_or_default()
}

/// Write `paths` back into the working tree from `tree`, leaving the index alone.
pub fn restore(repo_root: &Path, tree: &str, paths: &[String]) -> bool {
    if paths.is_empty() {
        return true;
    }
    let source = format!("--source={tree}");
    let mut args = vec!["restore", source.as_str(), "--worktree", "--"];
    args.extend(paths.iter().map(String::as_str));
    git(repo_root, None, &args).is_some()
}

/// The phase's view of what it may delete: the files present when it started,
/// and how many of them it has deleted so far.
pub struct DeletionGuard {
    repo_root: std::path::PathBuf,
    at_start: HashSet<String>,
    deleted: HashSet<String>,
}

/// What a command did to files present at phase start.
#[derive(Debug, PartialEq)]
pub enum Verdict {
    /// Within the limits (possibly no deletions at all).
    Allowed,
    /// Over a limit; these paths were written back.
    Restored(Vec<String>),
}

impl DeletionGuard {
    /// `None` outside a git repository.
    pub fn start(repo_root: &Path) -> Option<Self> {
        let tree = snapshot(repo_root)?;
        // ls-tree --full-tree lists paths from the repository top; diff-tree
        // --relative lists them from `repo_root`. Strip the prefix to compare.
        let prefix = git(repo_root, None, &["rev-parse", "--show-prefix"]).unwrap_or_default();
        let at_start = paths_in(repo_root, &tree)
            .into_iter()
            .filter_map(|p| p.strip_prefix(&prefix).map(str::to_string))
            .collect();
        Some(Self {
            repo_root: repo_root.to_path_buf(),
            at_start,
            deleted: HashSet::new(),
        })
    }

    /// Snapshot to take right before a command.
    pub fn before(&self) -> Option<String> {
        snapshot(&self.repo_root)
    }

    /// Compare against the `before` snapshot after a command ran, restoring
    /// everything the command deleted if it went over a limit.
    pub fn after(&mut self, before: &str) -> Verdict {
        let Some(now) = snapshot(&self.repo_root) else {
            return Verdict::Allowed;
        };
        let gone = deleted(&self.repo_root, before, &now);
        let existing: Vec<&String> = gone.iter().filter(|p| self.at_start.contains(*p)).collect();
        let new_total = existing
            .iter()
            .filter(|p| !self.deleted.contains(**p))
            .count()
            + self.deleted.len();
        if existing.len() > MAX_PER_COMMAND || new_total > MAX_PER_PHASE {
            restore(&self.repo_root, before, &gone);
            return Verdict::Restored(gone);
        }
        self.deleted.extend(existing.into_iter().cloned());
        Verdict::Allowed
    }

    /// The error a command over the limit gets in place of its output.
    pub fn restored_message(&self, paths: &[String]) -> String {
        let shown: Vec<&str> = paths.iter().take(10).map(String::as_str).collect();
        let more = paths.len().saturating_sub(shown.len());
        let more = if more > 0 {
            format!(" and {more} more")
        } else {
            String::new()
        };
        format!(
            "Error: Deletion limit exceeded — this command deleted {} file(s) that existed when \
             the phase started (limit {MAX_PER_COMMAND} per command, {MAX_PER_PHASE} per phase), \
             so all {} file(s) it deleted were restored: {}{more}. Fix code in place; never delete \
             files to start over. Delete a file only when it is scratch you made or your change \
             made it obsolete, one file at a time, and list it with a reason under files_deleted \
             in your artifact.",
            paths.iter().filter(|p| self.at_start.contains(*p)).count(),
            paths.len(),
            shown.join(", "),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sh(dir: &Path, cmd: &str) {
        let ok = Command::new("sh")
            .arg("-c")
            .arg(cmd)
            .current_dir(dir)
            .status()
            .unwrap()
            .success();
        assert!(ok, "{cmd}");
    }

    /// A repo with 6 committed files under src/ plus an uncommitted edit.
    fn repo() -> tempfile::TempDir {
        let d = tempfile::tempdir().unwrap();
        sh(
            d.path(),
            "git init -q && git config user.email t@t && git config user.name t \
             && mkdir src && for i in 1 2 3 4 5 6; do echo v$i > src/f$i.go; done \
             && echo target > .gitignore && git add -A && git commit -qm init \
             && echo edited > src/f1.go",
        );
        d
    }

    fn run(g: &mut DeletionGuard, dir: &Path, cmd: &str) -> Verdict {
        let before = g.before().unwrap();
        sh(dir, cmd);
        g.after(&before)
    }

    #[test]
    fn mass_delete_is_restored_with_contents() {
        let d = repo();
        let dir = d.path();
        let mut g = DeletionGuard::start(dir).unwrap();
        sh(dir, "echo new > src/new.go");
        let v = run(&mut g, dir, "rm -rf src");
        let Verdict::Restored(paths) = v else {
            panic!("{v:?}")
        };
        assert_eq!(paths.len(), 7);
        // Uncommitted edit and the file made this phase both come back.
        assert_eq!(
            std::fs::read_to_string(dir.join("src/f1.go")).unwrap(),
            "edited\n"
        );
        assert_eq!(
            std::fs::read_to_string(dir.join("src/new.go")).unwrap(),
            "new\n"
        );
        assert!(dir.join("src/f6.go").exists());
        // The real index is untouched: nothing staged.
        let staged = git(dir, None, &["diff", "--cached", "--name-only"]).unwrap();
        assert_eq!(staged, "");
        assert!(g
            .restored_message(&paths)
            .contains("6 file(s) that existed"));
    }

    #[test]
    fn scratch_files_are_free_and_one_obsolete_file_is_allowed() {
        let d = repo();
        let dir = d.path();
        let mut g = DeletionGuard::start(dir).unwrap();
        sh(dir, "for i in 1 2 3 4 5; do echo x > tmp$i.go; done");
        assert_eq!(
            run(&mut g, dir, "rm tmp1.go tmp2.go tmp3.go tmp4.go tmp5.go"),
            Verdict::Allowed
        );
        assert_eq!(run(&mut g, dir, "rm src/f2.go"), Verdict::Allowed);
        assert!(!dir.join("src/f2.go").exists());
        // Ignored build output never counts.
        sh(
            dir,
            "mkdir target && for i in 1 2 3 4 5; do echo o > target/o$i; done",
        );
        assert_eq!(run(&mut g, dir, "rm -rf target"), Verdict::Allowed);
    }

    #[test]
    fn phase_total_is_capped() {
        let d = tempfile::tempdir().unwrap();
        let dir = d.path();
        sh(
            dir,
            "git init -q && git config user.email t@t && git config user.name t \
             && for i in $(seq 1 12); do echo $i > f$i; done && git add -A && git commit -qm init",
        );
        let mut g = DeletionGuard::start(dir).unwrap();
        for batch in ["f1 f2 f3", "f4 f5 f6", "f7 f8 f9", "f10"] {
            assert_eq!(run(&mut g, dir, &format!("rm {batch}")), Verdict::Allowed);
        }
        assert_eq!(
            run(&mut g, dir, "rm f11"),
            Verdict::Restored(vec!["f11".to_string()])
        );
        assert!(dir.join("f11").exists());
    }

    #[test]
    fn works_from_a_subdirectory_and_not_outside_git() {
        let d = repo();
        let dir = d.path().join("src");
        let mut g = DeletionGuard::start(&dir).unwrap();
        assert!(matches!(
            run(&mut g, &dir, "rm f3.go f4.go f5.go f6.go"),
            Verdict::Restored(_)
        ));
        assert!(dir.join("f6.go").exists());
        assert_eq!(run(&mut g, &dir, "rm f3.go"), Verdict::Allowed);

        let plain = tempfile::tempdir().unwrap();
        assert!(DeletionGuard::start(plain.path()).is_none());
    }
}
