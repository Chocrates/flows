//! Ports of the behavior in `tests/test_detect.py` (+ generate_config round-trip).

use flows_core::config::parse_phases_from_yaml;
use flows_core::detect::{detect_stack, generate_config};
use flows_core::models::PhaseType;
use tempfile::tempdir;

fn touch(dir: &std::path::Path, name: &str, body: &str) {
    let p = dir.join(name);
    if let Some(parent) = p.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    std::fs::write(p, body).unwrap();
}

#[test]
fn detect_python_uv_pytest_ruff() {
    let d = tempdir().unwrap();
    touch(
        d.path(),
        "pyproject.toml",
        "[tool.ruff]\n[tool.pytest.ini_options]\n",
    );
    touch(d.path(), "uv.lock", "");
    std::fs::create_dir(d.path().join("tests")).unwrap();
    let info = detect_stack(d.path());
    assert_eq!(info.language, "python");
    assert_eq!(info.test_cmd, "pytest -x -q");
    assert!(info.lint_cmd.contains("ruff"));
    assert!(info.detected.contains(&"uv".to_string()));
    assert!(info.bash_allowed.contains(&"uv*".to_string()));
}

#[test]
fn detect_python_pip_no_pytest() {
    let d = tempdir().unwrap();
    touch(d.path(), "requirements.txt", "requests\n");
    let info = detect_stack(d.path());
    assert_eq!(info.language, "python");
    // pip is not a task runner — fall back to the stdlib runner, not `pip run test`.
    assert_eq!(info.test_cmd, "python -m unittest");
}

#[test]
fn detect_node_with_scripts() {
    let d = tempdir().unwrap();
    touch(
        d.path(),
        "package.json",
        r#"{"scripts":{"test":"jest","lint":"eslint ."}}"#,
    );
    touch(d.path(), "yarn.lock", "");
    let info = detect_stack(d.path());
    assert_eq!(info.language, "node");
    assert_eq!(info.test_cmd, "yarn test");
    assert_eq!(info.lint_cmd, "yarn run lint || true");
}

#[test]
fn detect_rust() {
    let d = tempdir().unwrap();
    touch(d.path(), "Cargo.toml", "[package]\n");
    let info = detect_stack(d.path());
    assert_eq!(info.language, "rust");
    assert_eq!(info.test_cmd, "cargo test");
    assert!(info.bash_allowed.contains(&"cargo*".to_string()));
}

#[test]
fn detect_go() {
    let d = tempdir().unwrap();
    touch(d.path(), "go.mod", "module x\n");
    let info = detect_stack(d.path());
    assert_eq!(info.language, "go");
    assert_eq!(info.test_cmd, "go test ./...");
}

#[test]
fn detect_unknown() {
    let d = tempdir().unwrap();
    let info = detect_stack(d.path());
    assert_eq!(info.language, "unknown");
    assert!(info.detected.contains(&"unknown stack".to_string()));
}

#[test]
fn generated_default_config_parses_back() {
    let d = tempdir().unwrap();
    touch(d.path(), "Cargo.toml", "[package]\n");
    let info = detect_stack(d.path());
    let yaml = generate_config(&info, "feature");

    let phases = parse_phases_from_yaml(&yaml).expect("generated yaml must parse");
    let names: Vec<_> = phases.iter().map(|p| p.name.as_str()).collect();
    assert!(names.contains(&"plan"));
    assert!(names.contains(&"implement"));
    assert!(names.contains(&"review"));
    assert!(names.contains(&"commit"));

    let implement = phases.iter().find(|p| p.name == "implement").unwrap();
    assert!(implement
        .tools
        .bash
        .as_ref()
        .unwrap()
        .allowed
        .contains(&"cargo*".to_string()));

    let review = phases.iter().find(|p| p.name == "review").unwrap();
    assert_eq!(review.phase_type, PhaseType::Human);
    assert_eq!(review.loop_back_to.as_deref(), Some("implement"));
}

#[test]
fn generated_research_config_parses_back() {
    let info = flows_core::detect::StackInfo::default();
    let yaml = generate_config(&info, "research");
    let phases = parse_phases_from_yaml(&yaml).expect("research yaml must parse");
    let names: Vec<_> = phases.iter().map(|p| p.name.as_str()).collect();
    assert_eq!(names, vec!["gather", "synthesize", "report", "approve"]);
    let gather = &phases[0];
    assert!(gather.tools.web_search); // research preset
}

#[test]
fn generated_config_omits_empty_lint_block() {
    // a stack with no lint command still produces valid yaml
    let d = tempdir().unwrap();
    touch(d.path(), "requirements.txt", "");
    let info = detect_stack(d.path());
    assert!(info.lint_cmd.is_empty());
    let yaml = generate_config(&info, "feature");
    assert!(!yaml.contains("name: lint"));
    // still parses
    assert!(parse_phases_from_yaml(&yaml).is_ok());
}
