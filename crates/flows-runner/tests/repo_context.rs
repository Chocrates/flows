use flows_runner::repo_context::{build_repo_tree, build_slim_tree, working_changes};
use tempfile::tempdir;

#[test]
fn slim_tree_bounds_depth_with_neutral_note() {
    let d = tempdir().unwrap();
    let r = d.path();
    std::fs::write(r.join("top.rs"), "").unwrap();
    std::fs::create_dir_all(r.join("a/b/c")).unwrap();
    std::fs::write(r.join("a/shallow.rs"), "").unwrap(); // depth 1
    std::fs::write(r.join("a/b/c/deep.rs"), "").unwrap(); // depth 3, beyond max
    std::fs::create_dir_all(r.join("node_modules")).unwrap();
    std::fs::write(r.join("node_modules/x.js"), "").unwrap();

    let tree = build_slim_tree(r, 200, 2);
    assert!(tree.contains("top.rs"));
    assert!(tree.contains("shallow.rs"));
    assert!(!tree.contains("deep.rs"), "depth-4 entry leaked past bound");
    assert!(!tree.contains("node_modules"), "vendor dir leaked");
    assert!(tree.contains("depth ≤2"), "no depth-clip note");
    // Tool names live in the prompt's tools section, never in the tree — so a
    // no-tool model's tree carries no tool references.
    assert!(!tree.contains("search_code") && !tree.contains("list_files"));
}

#[test]
fn slim_tree_caps_with_neutral_note() {
    let d = tempdir().unwrap();
    for i in 0..20 {
        std::fs::write(d.path().join(format!("f{i}.txt")), "").unwrap();
    }
    let tree = build_slim_tree(d.path(), 5, 2);
    assert!(tree.contains("capped"));
    assert!(!tree.contains("list_files") && !tree.contains("search_code"));
}

#[test]
fn tree_lists_files_respects_gitignore_and_deny() {
    let d = tempdir().unwrap();
    let r = d.path();
    std::fs::write(r.join("main.rs"), "").unwrap();
    std::fs::write(r.join(".gitignore"), "secret.txt\n").unwrap();
    std::fs::write(r.join("secret.txt"), "").unwrap();
    std::fs::create_dir_all(r.join("src")).unwrap();
    std::fs::write(r.join("src/lib.rs"), "").unwrap();
    std::fs::create_dir_all(r.join("node_modules/pkg")).unwrap();
    std::fs::write(r.join("node_modules/pkg/index.js"), "").unwrap();
    std::fs::create_dir_all(r.join(".git")).unwrap();
    std::fs::write(r.join(".git/config"), "").unwrap();

    let tree = build_repo_tree(r, 500);
    assert!(tree.contains("main.rs"));
    assert!(tree.contains("src/"));
    assert!(
        !tree.contains("/src"),
        "directories end with a slash, never start with one"
    );
    assert!(tree.contains("lib.rs"));
    assert!(!tree.contains("secret.txt"), "gitignored file leaked");
    assert!(!tree.contains("node_modules"), "vendor dir leaked");
    assert!(!tree.contains(".git/"), ".git leaked");
}

#[test]
fn tree_truncates() {
    let d = tempdir().unwrap();
    for i in 0..20 {
        std::fs::write(d.path().join(format!("f{i}.txt")), "").unwrap();
    }
    let tree = build_repo_tree(d.path(), 5);
    assert!(tree.contains("truncated"));
    assert!(tree.lines().count() <= 6);
}

fn git(repo: &std::path::Path, args: &[&str]) {
    let ok = std::process::Command::new("git")
        .args([
            "-c",
            "user.email=t@t",
            "-c",
            "user.name=t",
            "-c",
            "commit.gpgsign=false",
        ])
        .args(args)
        .current_dir(repo)
        .output()
        .unwrap()
        .status
        .success();
    assert!(ok, "git {args:?}");
}

#[test]
fn working_changes_lists_new_files_then_the_diff() {
    let d = tempdir().unwrap();
    let r = d.path();
    assert_eq!(working_changes(r, 10_000), "", "not a repo yet");
    git(r, &["init", "-q"]);
    std::fs::write(r.join("a.go"), "package a\n").unwrap();
    git(r, &["add", "."]);
    git(r, &["commit", "-qm", "init"]);
    assert_eq!(working_changes(r, 10_000), "", "clean tree");

    std::fs::write(r.join("a.go"), "package a\n\nfunc Last() {}\n").unwrap();
    std::fs::write(r.join("a_test.go"), "package a\n").unwrap();
    let changes = working_changes(r, 10_000);
    let new_at = changes.find("- a_test.go").expect("new file listed");
    let diff_at = changes.find("+func Last() {}").expect("diff shown");
    assert!(new_at < diff_at, "new files come first:\n{changes}");

    let cut = working_changes(r, 40);
    assert!(cut.contains("- a_test.go") && cut.contains("diff truncated"));
}
