//! Tests for `search_code` — match, no-match, scope escape, and cap.

use flows_tools::code_search::search_code;
use flows_tools::files::write_file;
use tempfile::tempdir;

#[test]
fn finds_matches_with_path_and_line() {
    let d = tempdir().unwrap();
    write_file(d.path(), "src/a.rs", "fn alpha() {}\nlet x = 1;\n").unwrap();
    write_file(d.path(), "src/b.rs", "fn beta() {}\n").unwrap();
    let out = search_code(d.path(), r"fn \w+", "", 100).unwrap();
    assert!(out.contains("src/a.rs:1: fn alpha() {}"), "got: {out}");
    assert!(out.contains("src/b.rs:1: fn beta() {}"), "got: {out}");
    assert!(!out.contains("let x")); // non-matching line excluded
}

#[test]
fn no_matches_is_a_message_not_an_error() {
    let d = tempdir().unwrap();
    write_file(d.path(), "a.txt", "hello world\n").unwrap();
    let out = search_code(d.path(), "zzz_nope", "", 100).unwrap();
    assert!(out.starts_with("No matches"), "got: {out}");
}

#[test]
fn scope_limits_the_search() {
    let d = tempdir().unwrap();
    write_file(d.path(), "src/a.rs", "needle\n").unwrap();
    write_file(d.path(), "other/b.rs", "needle\n").unwrap();
    let out = search_code(d.path(), "needle", "src", 100).unwrap();
    assert!(out.contains("src/a.rs"));
    assert!(!out.contains("other/b.rs"));
}

#[test]
fn scope_escape_is_refused() {
    let d = tempdir().unwrap();
    let err = search_code(d.path(), "x", "../..", 100).unwrap_err();
    assert!(err.0.contains("escapes repo root"), "got: {}", err.0);
}

#[test]
fn cap_truncates_output() {
    let d = tempdir().unwrap();
    let body: String = (0..50).map(|_| "match\n").collect();
    write_file(d.path(), "big.txt", &body).unwrap();
    let out = search_code(d.path(), "match", "", 5).unwrap();
    assert_eq!(out.lines().filter(|l| l.contains("big.txt")).count(), 5);
    assert!(out.contains("truncated at 5 matches"), "got: {out}");
}

#[test]
fn invalid_regex_errors() {
    let d = tempdir().unwrap();
    write_file(d.path(), "a.txt", "x\n").unwrap();
    assert!(search_code(d.path(), "(", "", 100).is_err());
}

#[test]
fn respects_gitignore() {
    let d = tempdir().unwrap();
    write_file(d.path(), ".gitignore", "ignored/\n").unwrap();
    write_file(d.path(), "kept.txt", "needle\n").unwrap();
    write_file(d.path(), "ignored/hidden.txt", "needle\n").unwrap();
    let out = search_code(d.path(), "needle", "", 100).unwrap();
    assert!(out.contains("kept.txt"));
    assert!(!out.contains("ignored/hidden.txt"), "got: {out}");
}
