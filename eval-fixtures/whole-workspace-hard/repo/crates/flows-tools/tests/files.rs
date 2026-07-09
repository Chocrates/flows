//! Ports of the behavior in `tests/test_tools_files.py`.

use flows_tools::files::{list_files, read_file, read_file_limited, safe_path, write_file};
use tempfile::tempdir;

#[test]
fn write_then_read() {
    let d = tempdir().unwrap();
    let msg = write_file(d.path(), "out.txt", "hello").unwrap();
    assert!(msg.contains("Written"));
    assert_eq!(read_file(d.path(), "out.txt").unwrap(), "hello");
}

#[test]
fn write_creates_parent_dirs() {
    let d = tempdir().unwrap();
    write_file(d.path(), "a/b/c.txt", "x").unwrap();
    assert!(d.path().join("a/b/c.txt").exists());
}

#[test]
fn read_missing_file_errors() {
    let d = tempdir().unwrap();
    assert!(read_file(d.path(), "nope.txt").is_err());
}

#[test]
fn empty_path_errors() {
    let d = tempdir().unwrap();
    assert!(safe_path(d.path(), "").is_err());
}

#[test]
fn parent_traversal_blocked() {
    let d = tempdir().unwrap();
    let err = read_file(d.path(), "../escape").unwrap_err();
    assert!(err.0.contains("escapes repo root"));
}

#[test]
fn absolute_path_blocked() {
    let d = tempdir().unwrap();
    let err = read_file(d.path(), "/etc/passwd").unwrap_err();
    assert!(err.0.contains("escapes repo root"));
}

#[test]
fn nested_traversal_blocked() {
    let d = tempdir().unwrap();
    write_file(d.path(), "sub/f.txt", "x").unwrap();
    let err = write_file(d.path(), "sub/../../evil.txt", "x").unwrap_err();
    assert!(err.0.contains("escapes repo root"));
}

#[test]
fn symlink_escape_blocked() {
    let d = tempdir().unwrap();
    let outside = tempdir().unwrap();
    std::fs::write(outside.path().join("secret.txt"), "top secret").unwrap();
    // symlink inside repo → outside dir
    std::os::unix::fs::symlink(outside.path(), d.path().join("link")).unwrap();
    let err = read_file(d.path(), "link/secret.txt").unwrap_err();
    assert!(err.0.contains("escapes repo root"), "got: {}", err.0);
}

#[test]
fn read_truncates_large_file() {
    let d = tempdir().unwrap();
    write_file(d.path(), "big.txt", &"x".repeat(1000)).unwrap();
    let out = read_file_limited(d.path(), "big.txt", 100).unwrap();
    assert!(out.contains("truncated at 100 bytes"));
    assert!(out.len() < 200);
}

#[test]
fn list_files_dirs_first_then_files() {
    let d = tempdir().unwrap();
    write_file(d.path(), "zeta.txt", "").unwrap();
    write_file(d.path(), "alpha.txt", "").unwrap();
    std::fs::create_dir(d.path().join("subdir")).unwrap();
    let out = list_files(d.path(), ".").unwrap();
    let lines: Vec<&str> = out.lines().collect();
    // dir first (with leading /), then files alphabetical
    assert_eq!(lines[0], "/subdir");
    assert_eq!(lines[1], "alpha.txt");
    assert_eq!(lines[2], "zeta.txt");
}

#[test]
fn list_files_on_a_file_returns_its_rel_path() {
    let d = tempdir().unwrap();
    write_file(d.path(), "x.txt", "").unwrap();
    assert_eq!(list_files(d.path(), "x.txt").unwrap(), "x.txt");
}

#[test]
fn list_empty_dir() {
    let d = tempdir().unwrap();
    std::fs::create_dir(d.path().join("empty")).unwrap();
    assert_eq!(list_files(d.path(), "empty").unwrap(), "(empty)");
}

#[test]
fn make_dir_creates_nested_within_root() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let out = flows_tools::files::make_dir(root, "a/b/c").unwrap();
    assert!(out.contains("a/b/c"));
    assert!(root.join("a/b/c").is_dir());
}

#[test]
fn make_dir_blocks_escape() {
    let dir = tempfile::tempdir().unwrap();
    assert!(flows_tools::files::make_dir(dir.path(), "../evil").is_err());
    assert!(flows_tools::files::make_dir(dir.path(), "/etc/evil").is_err());
}

#[test]
fn write_refuses_symlink_at_target() {
    use std::os::unix::fs::symlink;
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    // an in-repo symlink; O_NOFOLLOW must refuse to write through it
    std::fs::write(root.join("real.txt"), "orig").unwrap();
    symlink("real.txt", root.join("link.txt")).unwrap();
    let err = flows_tools::files::write_file(root, "link.txt", "pwned").unwrap_err();
    assert!(err.0.contains("symlink"), "got: {}", err.0);
    // the real file is untouched
    assert_eq!(
        std::fs::read_to_string(root.join("real.txt")).unwrap(),
        "orig"
    );
}

#[test]
fn read_refuses_symlink_at_target() {
    use std::os::unix::fs::symlink;
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    std::fs::write(root.join("real.txt"), "secret").unwrap();
    symlink("real.txt", root.join("link.txt")).unwrap();
    let err = flows_tools::files::read_file(root, "link.txt").unwrap_err();
    assert!(err.0.contains("symlink"), "got: {}", err.0);
}

#[test]
fn write_and_read_regular_file_still_works() {
    let dir = tempfile::tempdir().unwrap();
    flows_tools::files::write_file(dir.path(), "a/b.txt", "hi").unwrap();
    assert_eq!(
        flows_tools::files::read_file(dir.path(), "a/b.txt").unwrap(),
        "hi"
    );
}
