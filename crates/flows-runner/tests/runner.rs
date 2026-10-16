//! Linear runner tests with a scripted mock Provider.

use std::collections::{HashMap, VecDeque};
use std::path::Path;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;

use flows_core::models::{
    GlobalConfig, LLMConfig, OnFailure, PhaseConfig, PhaseType, ProjectConfig, RunStatus,
};
use flows_core::state::Store;
use flows_llm::types::{Completion, CompletionRequest, LlmError, Usage};
use flows_llm::Provider;
use flows_runner::runner::{eval_loop_condition, run_pipeline, RunError, RunnerCallbacks};
use serde_json::json;
use tempfile::tempdir;

// -- mocks -------------------------------------------------------------------

struct Mock {
    responses: Mutex<VecDeque<Completion>>,
}
impl Mock {
    fn new(r: Vec<Completion>) -> Self {
        Self {
            responses: Mutex::new(r.into()),
        }
    }
}
#[async_trait]
impl Provider for Mock {
    async fn complete(&self, _req: &CompletionRequest) -> Result<Completion, LlmError> {
        Ok(self
            .responses
            .lock()
            .unwrap()
            .pop_front()
            .expect("mock out of responses"))
    }
}

fn artifact(body: &str) -> Completion {
    artifact_usage(body, Usage::default())
}

fn artifact_usage(body: &str, usage: Usage) -> Completion {
    Completion {
        content: format!("```json\n{{\"type\": \"artifact\", \"content\": {body}}}\n```"),
        reasoning: None,
        tool_calls: vec![],
        usage,
        stop_reason: None,
    }
}

#[derive(Default)]
struct Cb {
    started: Mutex<Vec<String>>,
    completed: Mutex<Vec<String>>,
    loops: Mutex<Vec<(String, String)>>,
    gates: Mutex<Vec<String>>,
}
#[async_trait]
impl RunnerCallbacks for Cb {
    async fn on_questions(&self, _p: &str, _q: &[String]) -> String {
        "answer".into()
    }
    async fn on_human_gate(&self, p: &str, _i: &str) -> String {
        self.gates.lock().unwrap().push(p.into());
        "approved".into()
    }
    fn on_phase_start(&self, phase: &str, _i: usize, _t: usize) {
        self.started.lock().unwrap().push(phase.into());
    }
    fn on_run_complete(&self, _id: &str, phases: &[String]) {
        *self.completed.lock().unwrap() = phases.to_vec();
    }
    fn on_loop_back(&self, from: &str, to: &str, _it: u32, _max: u32) {
        self.loops.lock().unwrap().push((from.into(), to.into()));
    }
}

fn agent(name: &str) -> PhaseConfig {
    let mut p = PhaseConfig::new(name);
    p.tools = flows_core::models::ToolsConfig {
        files: false,
        ..Default::default()
    };
    p
}
fn script(name: &str, cmd: &str) -> PhaseConfig {
    let mut p = PhaseConfig::new(name);
    p.phase_type = PhaseType::Script;
    p.command = Some(cmd.into());
    p
}

async fn run(
    phases: &[PhaseConfig],
    provider: Mock,
    cb: &Cb,
    repo: &Path,
    store: &Store,
) -> Result<Vec<String>, RunError> {
    let provider: Arc<dyn Provider> = Arc::new(provider);
    let factory = move |_: &LLMConfig| provider.clone();
    run_pipeline(
        "r1",
        "build a thing",
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
            context_keys: Vec::new(),
        },
        &factory,
        cb,
        store,
        None,
        None,
    )
    .await
}

// -- tests -------------------------------------------------------------------

/// A provider that always returns the same canned artifact (serves any number of
/// phases), used to test per-profile routing + the provider cache.
struct Canned;
#[async_trait]
impl Provider for Canned {
    async fn complete(&self, _req: &CompletionRequest) -> Result<Completion, LlmError> {
        Ok(artifact("{}"))
    }
}

#[tokio::test]
async fn per_phase_profiles_route_to_distinct_cached_providers() {
    let d = tempdir().unwrap();
    let store = Store::new(d.path().join(".flows"));

    // Two profiles: a local coder (different endpoint) + a cloud reasoner.
    let mut models = HashMap::new();
    models.insert(
        "coder".to_string(),
        LLMConfig {
            model: "qwen2.5-coder".into(),
            api_base: "http://localhost:8080/v1".into(),
            ..LLMConfig::default()
        },
    );
    let project = ProjectConfig {
        llm: Default::default(),
        models,
        phases: vec![],
        template: String::new(),
        context: Default::default(),
        context_keys: Vec::new(),
    };

    // Phases: a → coder, b → default, c → coder (reuses a's provider).
    let mk = |name: &str, model: Option<&str>| {
        let mut p = agent(name);
        p.model = model.map(str::to_string);
        p
    };
    let phases = vec![
        mk("a", Some("coder")),
        mk("b", None),
        mk("c", Some("coder")),
    ];

    // Record the (model, endpoint) the factory is asked to build.
    let built: Arc<Mutex<Vec<(String, String)>>> = Arc::new(Mutex::new(Vec::new()));
    let built2 = built.clone();
    let factory = move |llm: &LLMConfig| -> Arc<dyn Provider> {
        built2
            .lock()
            .unwrap()
            .push((llm.model.clone(), llm.api_base.clone()));
        Arc::new(Canned)
    };

    let cb = Cb::default();
    let completed = run_pipeline(
        "r1",
        "idea",
        d.path(),
        &phases,
        "feature",
        &GlobalConfig::default(),
        &project,
        &factory,
        &cb,
        &store,
        None,
        None,
    )
    .await
    .unwrap();
    assert_eq!(completed, vec!["a", "b", "c"]);

    // Built exactly twice (coder + default); phase c reused coder's provider.
    let built = built.lock().unwrap();
    assert_eq!(built.len(), 2, "provider built once per distinct config");
    assert!(built.contains(&("qwen2.5-coder".into(), "http://localhost:8080/v1".into())));
    assert!(built.contains(&("claude-sonnet-4-6".into(), String::new())));
}

#[tokio::test]
async fn linear_agent_script_human_completes() {
    let d = tempdir().unwrap();
    let store = Store::new(d.path().join(".flows"));
    let phases = vec![agent("plan"), script("lint", "echo ok"), {
        let mut h = PhaseConfig::new("approve");
        h.phase_type = PhaseType::Human;
        h
    }];
    let provider = Mock::new(vec![artifact(r#"{"summary": "planned"}"#)]);
    let cb = Cb::default();
    let completed = run(&phases, provider, &cb, d.path(), &store).await.unwrap();
    assert_eq!(completed, vec!["plan", "lint", "approve"]);
    // state + artifacts persisted
    assert_eq!(
        store.read_state("r1").unwrap().unwrap().status,
        RunStatus::Completed
    );
    assert!(store.artifact_path("r1", "plan").exists());
    assert!(store.artifact_path("r1", "lint").exists());
}

#[tokio::test]
async fn script_failure_stops_run() {
    let d = tempdir().unwrap();
    let store = Store::new(d.path().join(".flows"));
    let mut lint = script("lint", "exit 1");
    lint.on_failure = OnFailure::Stop;
    let phases = vec![agent("plan"), lint, agent("after")];
    let provider = Mock::new(vec![artifact("{}")]); // only plan runs
    let cb = Cb::default();
    let err = run(&phases, provider, &cb, d.path(), &store)
        .await
        .unwrap_err();
    assert!(matches!(err, RunError::Failed { .. }));
    assert!(!cb.started.lock().unwrap().contains(&"after".to_string()));
    assert_eq!(
        store.read_state("r1").unwrap().unwrap().status,
        RunStatus::Failed
    );
}

#[tokio::test]
async fn script_failure_continue_proceeds() {
    let d = tempdir().unwrap();
    let store = Store::new(d.path().join(".flows"));
    let mut lint = script("lint", "exit 1");
    lint.on_failure = OnFailure::Continue;
    let phases = vec![lint, script("done", "echo done")];
    let cb = Cb::default();
    let completed = run(&phases, Mock::new(vec![]), &cb, d.path(), &store)
        .await
        .unwrap();
    assert!(completed.contains(&"done".to_string()));
}

#[tokio::test]
async fn agent_error_artifact_stops() {
    let d = tempdir().unwrap();
    let store = Store::new(d.path().join(".flows"));
    let mut plan = agent("plan");
    plan.on_failure = OnFailure::Stop;
    let provider = Mock::new(vec![artifact(r#"{"error": "boom"}"#)]);
    // an artifact whose content is {"error": ...} → treated as failure
    let cb = Cb::default();
    let err = run(&[plan, agent("next")], provider, &cb, d.path(), &store)
        .await
        .unwrap_err();
    assert!(matches!(err, RunError::Failed { .. }));
    assert!(!cb.started.lock().unwrap().contains(&"next".to_string()));
}

#[tokio::test]
async fn retry_on_failure_jumps_back() {
    let d = tempdir().unwrap();
    let store = Store::new(d.path().join(".flows"));
    // `test` FAILS the first run (creates the marker, exit 1) then PASSES (marker
    // exists, exit 0). So its `on_failure: retry:implement` fires exactly once and
    // jumps back to implement. If the jump-back were broken, implement runs once
    // and the run fails at `test`.
    let phases = vec![agent("implement"), {
        let mut s = script(
            "test",
            "if [ -f marker ]; then exit 0; else touch marker; exit 1; fi",
        );
        s.on_failure = OnFailure::Retry("implement".into());
        s
    }];
    // implement runs twice: the initial pass and again after the retry jump-back.
    let provider = Mock::new(vec![artifact("{}"), artifact("{}")]);
    let cb = Cb::default();
    let completed = run(&phases, provider, &cb, d.path(), &store).await.unwrap();
    assert_eq!(
        cb.started
            .lock()
            .unwrap()
            .iter()
            .filter(|p| *p == "implement")
            .count(),
        2,
        "retry:implement must re-run implement after the test failure"
    );
    assert_eq!(
        store.read_state("r1").unwrap().unwrap().status,
        RunStatus::Completed
    );
    assert_eq!(completed, vec!["implement", "test"]);
}

#[tokio::test]
async fn loop_back_on_condition_bounded() {
    let d = tempdir().unwrap();
    let store = Store::new(d.path().join(".flows"));
    let mut review = agent("review");
    review.loop_back_to = Some("implement".into());
    review.loop_condition = "content.get('verdict') != 'approved'".into();
    review.max_loop_iterations = 2;
    let phases = vec![agent("implement"), review];

    // implement, review(needs_fixes)→loop, implement, review(approved)→stop
    let provider = Mock::new(vec![
        artifact("{}"),
        artifact(r#"{"verdict": "needs_fixes"}"#),
        artifact("{}"),
        artifact(r#"{"verdict": "approved"}"#),
    ]);
    let cb = Cb::default();
    let completed = run(&phases, provider, &cb, d.path(), &store).await.unwrap();
    assert_eq!(
        store.read_state("r1").unwrap().unwrap().status,
        RunStatus::Completed
    );
    // implement appears started twice (looped once)
    let started = cb.started.lock().unwrap();
    assert_eq!(started.iter().filter(|p| *p == "implement").count(), 2);
    assert_eq!(cb.loops.lock().unwrap().len(), 1);
    let _ = completed;
}

// -- eval_loop_condition -----------------------------------------------------

#[test]
fn loop_condition_get_access() {
    let c = json!({"verdict": "needs_fixes"});
    assert!(eval_loop_condition(
        "content.get('verdict') != 'approved'",
        &c
    ));
    let c2 = json!({"verdict": "approved"});
    assert!(!eval_loop_condition(
        "content.get('verdict') != 'approved'",
        &c2
    ));
}

#[test]
fn loop_condition_empty_is_false() {
    assert!(!eval_loop_condition("", &json!({})));
}

#[test]
fn loop_condition_bad_expr_is_false() {
    assert!(!eval_loop_condition("this is not (valid", &json!({})));
}

#[tokio::test]
async fn writes_llm_jsonl_and_parse_usage_reads_it() {
    let d = tempdir().unwrap();
    let store = Store::new(d.path().join(".flows"));
    let phases = vec![agent("plan")];
    let provider = Mock::new(vec![artifact_usage(
        r#"{"ok": true}"#,
        Usage {
            input_tokens: 1_000_000,
            output_tokens: 1_000_000,
        },
    )]);
    let cb = Cb::default();
    run(&phases, provider, &cb, d.path(), &store).await.unwrap();

    let log = flows_core::llm_log::log_path(&store.run_dir("r1"));
    assert!(log.exists(), "llm.jsonl should be written");

    let report = flows_core::usage::parse_usage(&log, "r1", "");
    assert_eq!(report.phases.len(), 1);
    assert_eq!(report.phases[0].phase, "plan");
    assert_eq!(report.total_input(), 1_000_000);
    assert_eq!(report.total_output(), 1_000_000);
    // claude-sonnet-4-6 default: 1M in @ $3 + 1M out @ $15 = $18
    assert!((report.total_cost() - 18.0).abs() < 1e-6);
}

#[tokio::test]
async fn auto_advance_human_phase_skips_gate() {
    let d = tempdir().unwrap();
    let store = Store::new(d.path().join(".flows"));
    let mut gate = PhaseConfig::new("approve");
    gate.phase_type = PhaseType::Human;
    gate.auto_advance = true;
    let phases = vec![agent("plan"), gate];
    let provider = Mock::new(vec![artifact("{}")]);
    let cb = Cb::default();
    let completed = run(&phases, provider, &cb, d.path(), &store).await.unwrap();
    assert_eq!(completed, vec!["plan", "approve"]);
    // the interactive gate callback must NOT have been called
    assert!(cb.gates.lock().unwrap().is_empty());
}

#[tokio::test]
async fn manual_human_phase_calls_gate() {
    let d = tempdir().unwrap();
    let store = Store::new(d.path().join(".flows"));
    let mut gate = PhaseConfig::new("approve");
    gate.phase_type = PhaseType::Human;
    let phases = vec![gate];
    let cb = Cb::default();
    run(&phases, Mock::new(vec![]), &cb, d.path(), &store)
        .await
        .unwrap();
    assert_eq!(*cb.gates.lock().unwrap(), vec!["approve".to_string()]);
}

#[tokio::test]
async fn persisted_artifact_carries_run_id() {
    let d = tempdir().unwrap();
    let store = Store::new(d.path().join(".flows"));
    let provider = Mock::new(vec![artifact(r#"{"ok": true}"#)]);
    let cb = Cb::default();
    run(&[agent("plan")], provider, &cb, d.path(), &store)
        .await
        .unwrap();

    let yaml = std::fs::read_to_string(store.artifact_path("r1", "plan")).unwrap();
    assert!(
        yaml.contains("run_id: r1"),
        "artifact missing run_id:\n{yaml}"
    );
    // and the in-memory value the runner threads forward
    let a: serde_json::Value = serde_yaml_ng::from_str(&yaml).unwrap();
    assert_eq!(a["run_id"], "r1");
}

#[tokio::test]
async fn auto_test_unresolvable_stops_run_despite_retry() {
    let d = tempdir().unwrap();
    let store = Store::new(d.path().join(".flows"));
    // repo has no detectable stack → @auto:test can't resolve → fatal stop,
    // even though on_failure says retry:implement.
    let mut test = script("test", "@auto:test");
    test.on_failure = OnFailure::Retry("implement".into());
    let phases = vec![agent("implement"), test];
    let provider = Mock::new(vec![artifact("{}")]);
    let cb = Cb::default();
    let err = run(&phases, provider, &cb, d.path(), &store)
        .await
        .unwrap_err();
    assert!(matches!(err, RunError::Failed { .. }));
    assert_eq!(
        store.read_state("r1").unwrap().unwrap().status,
        RunStatus::Failed
    );
}

#[tokio::test]
async fn auto_test_resolves_and_runs_for_detected_stack() {
    let d = tempdir().unwrap();
    // make @auto:test resolve to `make test` via a Makefile that just succeeds
    std::fs::write(d.path().join("Makefile"), "test:\n\ttrue\nlint:\n\ttrue\n").unwrap();
    let store = Store::new(d.path().join(".flows"));
    let phases = vec![agent("plan"), script("test", "@auto:test")];
    let provider = Mock::new(vec![artifact("{}")]);
    let cb = Cb::default();
    let completed = run(&phases, provider, &cb, d.path(), &store).await.unwrap();
    assert!(completed.contains(&"test".to_string()));
}
