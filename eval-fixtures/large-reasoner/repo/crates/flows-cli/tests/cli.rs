//! End-to-end CLI tests: run the built `flows` binary in a sandboxed HOME/cwd.

use std::path::Path;
use std::process::Command;

const BIN: &str = env!("CARGO_BIN_EXE_flows");

fn run(args: &[&str], cwd: &Path, home: &Path) -> std::process::Output {
    Command::new(BIN)
        .args(args)
        .current_dir(cwd)
        .env("HOME", home)
        .env_remove("ANTHROPIC_API_KEY")
        .output()
        .expect("failed to run flows binary")
}

#[test]
fn doctor_runs() {
    let dir = tempfile::tempdir().unwrap();
    let out = run(&["doctor"], dir.path(), dir.path());
    assert!(out.status.success());
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(text.contains("flows doctor"));
    assert!(text.contains("templates:"));
    assert!(text.contains("feature"));
}

#[test]
fn templates_sync_writes_builtins() {
    let dir = tempfile::tempdir().unwrap();
    let out = run(&["templates", "sync"], dir.path(), dir.path());
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let tdir = dir.path().join(".flows").join("templates");
    for name in [
        "feature.yml",
        "software.yml",
        "research.yml",
        "review.yml",
        "blank.yml",
    ] {
        assert!(tdir.join(name).exists(), "missing {name}");
    }
    // second sync without --force skips
    let out2 = run(&["templates", "sync"], dir.path(), dir.path());
    assert!(String::from_utf8_lossy(&out2.stdout).contains("skipped"));
}

#[test]
fn init_writes_flows_yml_from_detected_stack() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("Cargo.toml"), "[package]\n").unwrap();
    let out = run(&["init"], dir.path(), dir.path());
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let yml = std::fs::read_to_string(dir.path().join(".flows.yml")).unwrap();
    assert!(yml.contains("Rust"));
    assert!(yml.contains("cargo*"));

    // second init without --force refuses
    let out2 = run(&["init"], dir.path(), dir.path());
    assert!(!out2.status.success());
    assert!(String::from_utf8_lossy(&out2.stderr).contains("already exists"));
}

#[test]
fn plan_shows_execution_plan_and_estimate() {
    let dir = tempfile::tempdir().unwrap();
    let out = run(
        &["plan", "add a feature", "--template", "feature"],
        dir.path(),
        dir.path(),
    );
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(text.contains("Execution plan:"));
    assert!(text.contains("plan"));
    assert!(text.contains("implement"));
    assert!(text.contains("Cost estimate:"));
    assert!(text.contains("Total:"));
}

#[test]
fn plan_requires_idea() {
    let dir = tempfile::tempdir().unwrap();
    let out = run(&["plan"], dir.path(), dir.path());
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("provide an idea"));
}

#[test]
fn runs_empty_when_no_runs() {
    let dir = tempfile::tempdir().unwrap();
    let out = run(&["runs"], dir.path(), dir.path());
    assert!(out.status.success());
    assert!(String::from_utf8_lossy(&out.stdout).contains("no runs"));
}

#[test]
fn run_without_api_key_errors() {
    let dir = tempfile::tempdir().unwrap();
    // background and foreground both require a key up front
    for args in [
        vec!["run", "do a thing"],
        vec!["run", "do a thing", "--background"],
    ] {
        let out = run(&args, dir.path(), dir.path());
        assert!(!out.status.success());
        assert!(String::from_utf8_lossy(&out.stderr).contains("no model endpoint"));
    }
}

#[test]
fn usage_reports_nothing_for_unknown_run() {
    let dir = tempfile::tempdir().unwrap();
    let out = run(&["usage", "nope"], dir.path(), dir.path());
    assert!(out.status.success());
    assert!(String::from_utf8_lossy(&out.stdout).contains("no usage recorded"));
}

#[test]
fn config_set_writes_and_show_reflects_it() {
    let dir = tempfile::tempdir().unwrap();
    let out = run(
        &["config", "set", "model", "gpt-4o"],
        dir.path(),
        dir.path(),
    );
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let yml = std::fs::read_to_string(dir.path().join(".flows/config.yml")).unwrap();
    assert!(yml.contains("model: gpt-4o"));

    let show = run(&["config", "show"], dir.path(), dir.path());
    assert!(String::from_utf8_lossy(&show.stdout).contains("gpt-4o"));
}

#[test]
fn config_set_preserves_other_keys() {
    let dir = tempfile::tempdir().unwrap();
    run(&["config", "set", "model", "qwen"], dir.path(), dir.path());
    run(
        &["config", "set", "api-base", "http://localhost:8080/v1"],
        dir.path(),
        dir.path(),
    );
    let yml = std::fs::read_to_string(dir.path().join(".flows/config.yml")).unwrap();
    assert!(yml.contains("model: qwen"));
    assert!(yml.contains("api_base: http://localhost:8080/v1"));
}

#[test]
fn config_set_unknown_key_errors() {
    let dir = tempfile::tempdir().unwrap();
    let out = run(&["config", "set", "bogus", "x"], dir.path(), dir.path());
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("unknown config key"));
}

#[test]
fn completions_generates_script() {
    let dir = tempfile::tempdir().unwrap();
    let out = run(&["completions", "bash"], dir.path(), dir.path());
    assert!(out.status.success());
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(text.contains("_flows") && text.contains("complete"));
}

#[test]
fn templates_sync_also_writes_prompts() {
    let dir = tempfile::tempdir().unwrap();
    let out = run(&["templates", "sync"], dir.path(), dir.path());
    assert!(out.status.success());
    assert!(dir.path().join(".flows/prompts/feature/plan.md").exists());
    assert!(String::from_utf8_lossy(&out.stdout).contains("Synced prompts"));
}

fn seed_run(home: &std::path::Path, id: &str, status: &str) {
    let rd = home.join(".flows/runs").join(id);
    std::fs::create_dir_all(rd.join("signals")).unwrap();
    std::fs::write(
        rd.join("state.json"),
        format!(r#"{{"run_id":"{id}","status":"{status}","current_phase":"plan","current_phase_index":0,"round":0,"questions":[],"error":null,"completed_phases":[]}}"#),
    ).unwrap();
    std::fs::write(
        rd.join("meta.json"),
        format!(r#"{{"run_id":"{id}","idea":"x","slug":"","repo_path":"/","template":"feature","phases":[],"created_at":"","pid":null}}"#),
    ).unwrap();
}

#[test]
fn delete_missing_run_errors() {
    let dir = tempfile::tempdir().unwrap();
    let out = run(&["delete", "nope"], dir.path(), dir.path());
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("no such run"));
}

#[test]
fn delete_finished_run_removes_it() {
    let dir = tempfile::tempdir().unwrap();
    seed_run(dir.path(), "r1", "completed");
    let out = run(&["delete", "r1"], dir.path(), dir.path());
    assert!(out.status.success());
    assert!(!dir.path().join(".flows/runs/r1").exists());
}

#[test]
fn delete_active_run_needs_force() {
    let dir = tempfile::tempdir().unwrap();
    seed_run(dir.path(), "r1", "running");
    // without --force: refused, dir intact
    let out = run(&["delete", "r1"], dir.path(), dir.path());
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("--force"));
    assert!(dir.path().join(".flows/runs/r1").exists());
    // with --force: deleted
    let out = run(&["delete", "r1", "--force"], dir.path(), dir.path());
    assert!(out.status.success());
    assert!(!dir.path().join(".flows/runs/r1").exists());
}

#[test]
fn delete_accepts_unique_prefix() {
    let dir = tempfile::tempdir().unwrap();
    seed_run(dir.path(), "abcd1234ef01", "completed");
    let out = run(&["delete", "abcd"], dir.path(), dir.path());
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(!dir.path().join(".flows/runs/abcd1234ef01").exists());
}

#[test]
fn delete_ambiguous_prefix_errors() {
    let dir = tempfile::tempdir().unwrap();
    seed_run(dir.path(), "abcd1111", "completed");
    seed_run(dir.path(), "abcd2222", "completed");
    let out = run(&["delete", "abcd"], dir.path(), dir.path());
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("ambiguous"));
    // both still present
    assert!(dir.path().join(".flows/runs/abcd1111").exists());
}

#[test]
fn init_global_writes_config_and_templates() {
    let dir = tempfile::tempdir().unwrap();
    let out = run(&["init", "--global"], dir.path(), dir.path());
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(dir.path().join(".flows/config.yml").exists());
    assert!(dir.path().join(".flows/templates/feature.yml").exists());
    assert!(dir.path().join(".flows/prompts/feature/plan.md").exists());
    // a second run must not clobber the config
    std::fs::write(
        dir.path().join(".flows/config.yml"),
        "llm:\n  model: mine\n",
    )
    .unwrap();
    let _ = run(&["init", "--global"], dir.path(), dir.path());
    let cfg = std::fs::read_to_string(dir.path().join(".flows/config.yml")).unwrap();
    assert!(cfg.contains("model: mine"), "config was clobbered");
}
