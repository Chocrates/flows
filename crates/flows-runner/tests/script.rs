//! Script-phase tests. Port of `tests/test_phases_script.py`.

use flows_core::models::PhaseConfig;
use flows_runner::script::{run_script, run_script_cancellable, SCRIPT_CANCELLED};
use tempfile::tempdir;

fn script_phase(name: &str, command: &str) -> PhaseConfig {
    let mut p = PhaseConfig::new(name);
    p.phase_type = flows_core::models::PhaseType::Script;
    p.command = Some(command.into());
    p
}

#[test]
fn runs_command_and_captures_stdout() {
    let d = tempdir().unwrap();
    let r = run_script(&script_phase("echo", "echo hello"), d.path(), "").unwrap();
    assert_eq!(r.exit_code, 0);
    assert!(r.stdout.contains("hello"));
    assert!(r.success());
}

#[test]
fn cancel_kills_a_long_running_script() {
    let d = tempdir().unwrap();
    // A script that would run for 30s must be killed promptly when cancel fires.
    let start = std::time::Instant::now();
    let err = run_script_cancellable(&script_phase("slow", "sleep 30"), d.path(), "", &|| true)
        .unwrap_err();
    assert_eq!(err, SCRIPT_CANCELLED);
    assert!(
        start.elapsed().as_secs() < 5,
        "should have been killed fast"
    );
}

#[test]
fn no_cancel_runs_to_completion() {
    let d = tempdir().unwrap();
    let r =
        run_script_cancellable(&script_phase("echo", "echo hi"), d.path(), "", &|| false).unwrap();
    assert_eq!(r.exit_code, 0);
    assert!(r.stdout.contains("hi"));
}

#[test]
fn nonzero_exit_captured() {
    let d = tempdir().unwrap();
    let r = run_script(&script_phase("fail", "exit 3"), d.path(), "").unwrap();
    assert_eq!(r.exit_code, 3);
    assert!(!r.success());
}

#[test]
fn stderr_captured() {
    let d = tempdir().unwrap();
    let r = run_script(&script_phase("err", "echo oops >&2"), d.path(), "").unwrap();
    assert!(r.stderr.contains("oops"));
}

#[test]
fn runs_in_repo_root() {
    let d = tempdir().unwrap();
    std::fs::write(d.path().join("marker.txt"), "found").unwrap();
    let r = run_script(&script_phase("cat", "cat marker.txt"), d.path(), "").unwrap();
    assert!(r.stdout.contains("found"));
}

#[test]
fn renders_idea_into_command() {
    let d = tempdir().unwrap();
    let r = run_script(
        &script_phase("echo", "echo {{ idea }}"),
        d.path(),
        "my great idea",
    )
    .unwrap();
    assert!(r.stdout.contains("my great idea"));
}

#[test]
fn missing_command_errors() {
    let d = tempdir().unwrap();
    let mut p = PhaseConfig::new("empty");
    p.phase_type = flows_core::models::PhaseType::Script;
    assert!(run_script(&p, d.path(), "").is_err());
}
