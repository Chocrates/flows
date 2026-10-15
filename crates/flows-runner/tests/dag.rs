//! DAG (depends_on) execution tests.

use std::collections::VecDeque;
use std::path::Path;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;

use flows_core::models::{
    GlobalConfig, LLMConfig, OnFailure, PhaseConfig, PhaseType, ProjectConfig, RunStatus,
};
use flows_core::state::Store;
use flows_llm::types::{Completion, CompletionRequest, LlmError};
use flows_llm::Provider;
use flows_runner::runner::{run_pipeline, RunError, RunnerCallbacks};
use tempfile::tempdir;

struct Mock(Mutex<VecDeque<Completion>>);
#[async_trait]
impl Provider for Mock {
    async fn complete(&self, _r: &CompletionRequest) -> Result<Completion, LlmError> {
        Ok(self
            .0
            .lock()
            .unwrap()
            .pop_front()
            .expect("out of responses"))
    }
}

#[derive(Default)]
struct Cb {
    started: Mutex<Vec<String>>,
    completed: Mutex<Vec<String>>,
}
#[async_trait]
impl RunnerCallbacks for Cb {
    async fn on_questions(&self, _p: &str, _q: &[String]) -> String {
        "a".into()
    }
    async fn on_human_gate(&self, _p: &str, _i: &str) -> String {
        "ok".into()
    }
    fn on_phase_start(&self, p: &str, _i: usize, _t: usize) {
        self.started.lock().unwrap().push(p.into());
    }
    fn on_run_complete(&self, _id: &str, phases: &[String]) {
        *self.completed.lock().unwrap() = phases.to_vec();
    }
}

fn script(name: &str, cmd: &str, deps: &[&str]) -> PhaseConfig {
    let mut p = PhaseConfig::new(name);
    p.phase_type = PhaseType::Script;
    p.command = Some(cmd.into());
    p.depends_on = deps.iter().map(|s| s.to_string()).collect();
    p
}

async fn run(
    phases: &[PhaseConfig],
    cb: &Cb,
    repo: &Path,
    store: &Store,
) -> Result<Vec<String>, RunError> {
    run_pipeline(
        "r1",
        "idea",
        repo,
        phases,
        "feature",
        &GlobalConfig::default(),
        &ProjectConfig {
            llm: Default::default(),
            models: Default::default(),
            phases: vec![],
            template: String::new(),
            context: Default::default(),
        },
        &|_: &LLMConfig| -> Arc<dyn Provider> { Arc::new(Mock(Mutex::new(VecDeque::new()))) },
        cb,
        store,
        None,
        None,
    )
    .await
}

#[tokio::test]
async fn diamond_respects_dependency_order() {
    let d = tempdir().unwrap();
    let store = Store::new(d.path().join(".flows"));
    // a → {b, c} → d
    let phases = vec![
        script("a", "echo a", &[]),
        script("b", "echo b", &["a"]),
        script("c", "echo c", &["a"]),
        script("d", "echo d", &["b", "c"]),
    ];
    let cb = Cb::default();
    let order = run(&phases, &cb, d.path(), &store).await.unwrap();
    assert_eq!(order.len(), 4);
    assert_eq!(order[0], "a"); // root first
    assert_eq!(order[3], "d"); // sink last
    assert_eq!(
        store.read_state("r1").unwrap().unwrap().status,
        RunStatus::Completed
    );
    // b and c only start after a
    let started = cb.started.lock().unwrap();
    let pa = started.iter().position(|x| x == "a").unwrap();
    let pb = started.iter().position(|x| x == "b").unwrap();
    let pc = started.iter().position(|x| x == "c").unwrap();
    assert!(pa < pb && pa < pc);
}

#[tokio::test]
async fn dag_on_failure_continue_lets_dependents_run() {
    let d = tempdir().unwrap();
    let store = Store::new(d.path().join(".flows"));
    let mut a = script("a", "exit 1", &[]);
    a.on_failure = OnFailure::Continue;
    let phases = vec![a, script("b", "echo b", &["a"])];
    let cb = Cb::default();
    let order = run(&phases, &cb, d.path(), &store).await.unwrap();
    assert!(order.contains(&"b".to_string()));
}

#[tokio::test]
async fn dag_on_failure_stop_fails_run() {
    let d = tempdir().unwrap();
    let store = Store::new(d.path().join(".flows"));
    let mut a = script("a", "exit 1", &[]);
    a.on_failure = OnFailure::Stop;
    let phases = vec![a, script("b", "echo b", &["a"])];
    let cb = Cb::default();
    let err = run(&phases, &cb, d.path(), &store).await.unwrap_err();
    assert!(matches!(err, RunError::Failed { .. }));
    // b must never have started (its dep failed / run stopped)
    assert!(!cb.started.lock().unwrap().contains(&"b".to_string()));
    assert_eq!(
        store.read_state("r1").unwrap().unwrap().status,
        RunStatus::Failed
    );
}

#[tokio::test]
async fn dag_independent_phases_both_run_before_join() {
    let d = tempdir().unwrap();
    let store = Store::new(d.path().join(".flows"));
    // a, b independent; c depends on both
    let phases = vec![
        script("a", "echo a", &[]),
        script("b", "echo b", &[]),
        script("c", "echo c", &["a", "b"]),
    ];
    let cb = Cb::default();
    let order = run(&phases, &cb, d.path(), &store).await.unwrap();
    assert_eq!(order.len(), 3);
    assert_eq!(order.last().unwrap(), "c"); // join runs last
}
