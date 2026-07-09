use flows_runner::repo_context::build_repo_tree;
use tempfile::tempdir;

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
    assert!(tree.contains("/src"));
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
