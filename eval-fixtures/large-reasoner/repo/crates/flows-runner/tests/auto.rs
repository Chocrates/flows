//! @auto:test / @auto:lint resolution.
use flows_runner::script::{auto_kind, resolve_auto, AutoKind};
use tempfile::tempdir;

fn repo() -> tempfile::TempDir {
    tempdir().unwrap()
}

#[test]
fn recognizes_sentinels() {
    assert!(matches!(auto_kind("@auto:test"), Some(AutoKind::Test)));
    assert!(matches!(auto_kind("@auto:lint"), Some(AutoKind::Lint)));
    assert!(auto_kind("cargo test").is_none());
}

#[test]
fn prefers_makefile_target() {
    let d = repo();
    std::fs::write(d.path().join("Cargo.toml"), "[package]\nname='x'\n").unwrap();
    std::fs::write(
        d.path().join("Makefile"),
        "test:\n\tcargo test\nlint:\n\tcargo clippy\n",
    )
    .unwrap();
    assert_eq!(resolve_auto(AutoKind::Test, d.path()).unwrap(), "make test");
    assert_eq!(resolve_auto(AutoKind::Lint, d.path()).unwrap(), "make lint");
}

#[test]
fn detects_languages() {
    let cases = [
        (
            "Cargo.toml",
            "[package]",
            "cargo test",
            "cargo clippy -- -D warnings",
        ),
        ("go.mod", "module x", "go test ./...", "go vet ./..."),
    ];
    for (file, body, test, lint) in cases {
        let d = repo();
        std::fs::write(d.path().join(file), body).unwrap();
        assert_eq!(resolve_auto(AutoKind::Test, d.path()).unwrap(), test);
        assert_eq!(resolve_auto(AutoKind::Lint, d.path()).unwrap(), lint);
    }
}

#[test]
fn python_test_but_lint_needs_config() {
    let d = repo();
    std::fs::write(d.path().join("pyproject.toml"), "[tool.pytest]\n").unwrap();
    assert_eq!(resolve_auto(AutoKind::Test, d.path()).unwrap(), "pytest");
    // no ruff/flake8 configured → lint unresolvable → hard stop
    assert!(resolve_auto(AutoKind::Lint, d.path()).is_err());
    // add ruff config → resolves
    std::fs::write(d.path().join("pyproject.toml"), "[tool.ruff]\n").unwrap();
    assert_eq!(
        resolve_auto(AutoKind::Lint, d.path()).unwrap(),
        "ruff check ."
    );
}

#[test]
fn npm_only_when_script_present() {
    let d = repo();
    std::fs::write(
        d.path().join("package.json"),
        r#"{"scripts":{"test":"jest"}}"#,
    )
    .unwrap();
    assert_eq!(
        resolve_auto(AutoKind::Test, d.path()).unwrap(),
        "npm run test"
    );
    assert!(resolve_auto(AutoKind::Lint, d.path()).is_err()); // no lint script
}

#[test]
fn unknown_stack_errors() {
    let d = repo();
    std::fs::write(d.path().join("README.md"), "hi").unwrap();
    assert!(resolve_auto(AutoKind::Test, d.path()).is_err());
    assert!(resolve_auto(AutoKind::Lint, d.path()).is_err());
}
