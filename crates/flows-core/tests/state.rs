//! Ports of the behavior in `tests/test_state.py`.

use flows_core::models::{RunMeta, RunState, RunStatus};
use flows_core::state::Store;
use tempfile::tempdir;

fn store() -> (tempfile::TempDir, Store) {
    let dir = tempdir().unwrap();
    let store = Store::new(dir.path().join(".flows"));
    (dir, store)
}

fn meta(run_id: &str) -> RunMeta {
    RunMeta {
        run_id: run_id.into(),
        idea: "do a thing".into(),
        slug: "do-a-thing".into(),
        repo_path: "/repo".into(),
        template: "feature".into(),
        phases: vec![serde_json::json!({"name": "plan"})],
        created_at: "2026-07-04T00:00:00Z".into(),
        pid: Some(1234),
    }
}

fn state(run_id: &str) -> RunState {
    RunState {
        run_id: run_id.into(),
        status: RunStatus::Running,
        current_phase: "plan".into(),
        current_phase_index: 0,
        round: 0,
        questions: vec![],
        error: None,
        completed_phases: vec![],
        skipped_phases: vec![],
        fan_outs: Default::default(),
    }
}

#[test]
fn meta_round_trips() {
    let (_d, s) = store();
    s.ensure_run_dir("r1").unwrap();
    let m = meta("r1");
    s.write_meta(&m).unwrap();
    assert_eq!(s.read_meta("r1").unwrap(), m);
}

#[test]
fn state_round_trips_and_missing_is_none() {
    let (_d, s) = store();
    s.ensure_run_dir("r1").unwrap();
    assert!(s.read_state("r1").unwrap().is_none());
    let st = state("r1");
    s.write_state(&st).unwrap();
    assert_eq!(s.read_state("r1").unwrap().unwrap(), st);
}

#[test]
fn write_is_atomic_no_tmp_left() {
    let (_d, s) = store();
    s.ensure_run_dir("r1").unwrap();
    s.write_state(&state("r1")).unwrap();
    assert!(s.run_dir("r1").join("state.json").exists());
    assert!(!s.run_dir("r1").join("state.tmp").exists());
}

#[test]
fn ensure_run_dir_creates_subdirs() {
    let (_d, s) = store();
    let d = s.ensure_run_dir("r1").unwrap();
    assert!(d.join("artifacts").is_dir());
    assert!(d.join("signals").is_dir());
}

#[test]
fn cancel_signal_roundtrip() {
    let (_d, s) = store();
    s.ensure_run_dir("r1").unwrap();
    assert!(!s.poll_cancel("r1"));
    s.send_cancel("r1").unwrap();
    assert!(s.poll_cancel("r1")); // consumed
    assert!(!s.poll_cancel("r1")); // gone
}

#[test]
fn advance_signal_roundtrip() {
    let (_d, s) = store();
    s.ensure_run_dir("r1").unwrap();
    s.send_advance("r1").unwrap();
    assert!(s.poll_advance("r1"));
    assert!(!s.poll_advance("r1"));
}

#[test]
fn answer_signal_sets_answer_and_advance() {
    let (_d, s) = store();
    s.ensure_run_dir("r1").unwrap();
    s.send_answer("r1", "  use option A  ").unwrap();
    // answer is trimmed on read, and advance is also raised
    assert_eq!(s.read_answer("r1").as_deref(), Some("use option A"));
    assert!(s.read_answer("r1").is_none()); // consumed
    assert!(s.poll_advance("r1"));
}

#[test]
fn artifact_path_shape() {
    let (_d, s) = store();
    let p = s.artifact_path("r1", "implement");
    assert!(p.ends_with("runs/r1/artifacts/implement.yml"));
}

#[test]
fn list_runs_newest_first() {
    let (_d, s) = store();
    for id in ["a", "b", "c"] {
        s.ensure_run_dir(id).unwrap();
        s.write_meta(&meta(id)).unwrap();
        // stagger mtimes
        std::thread::sleep(std::time::Duration::from_millis(10));
        // bump c/b mtime by rewriting later
    }
    let runs = s.list_runs();
    assert_eq!(runs.len(), 3);
    // newest (c, written last) first
    assert_eq!(runs[0].run_id, "c");
}

#[test]
fn list_runs_skips_dirs_without_meta() {
    let (_d, s) = store();
    s.ensure_run_dir("has_meta").unwrap();
    s.write_meta(&meta("has_meta")).unwrap();
    s.ensure_run_dir("no_meta").unwrap(); // dir exists, no meta.json
    let runs = s.list_runs();
    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0].run_id, "has_meta");
}

#[test]
fn delete_run_removes_dir_idempotently() {
    let dir = tempfile::tempdir().unwrap();
    let store = flows_core::state::Store::new(dir.path());
    store.ensure_run_dir("r1").unwrap();
    assert!(store.run_dir("r1").exists());
    store.delete_run("r1").unwrap();
    assert!(!store.run_dir("r1").exists());
    store.delete_run("r1").unwrap(); // idempotent
}

#[test]
fn state_without_fan_outs_loads_and_round_trips() {
    // A state.json written before fan-out groups existed.
    let old = r#"{"run_id":"r1","status":"running","current_phase":"plan",
                  "current_phase_index":0,"completed_phases":[]}"#;
    let st: RunState = serde_json::from_str(old).unwrap();
    assert!(st.fan_outs.is_empty());
    // Empty map stays off disk; a filled one round-trips.
    assert!(!serde_json::to_string(&st).unwrap().contains("fan_outs"));
    let mut st = st;
    st.fan_outs
        .insert("implement".into(), vec!["1".into(), "2".into()]);
    let back: RunState = serde_json::from_str(&serde_json::to_string(&st).unwrap()).unwrap();
    assert_eq!(back.fan_outs["implement"], ["1", "2"]);
}
