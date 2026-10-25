//! `for_each` fan-out groups in the linear runner, with a scripted mock Provider.

use std::collections::VecDeque;
use std::path::Path;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;

use flows_core::models::{
    GlobalConfig, LLMConfig, OnFailure, PhaseConfig, PhaseType, ProjectConfig, RunStatus,
};
use flows_core::state::Store;
use flows_llm::types::{Completion, CompletionRequest, LlmError, Usage};
use flows_llm::Provider;
use flows_runner::runner::{run_pipeline, RunError, RunnerCallbacks};
use serde_json::{json, Value};
use tempfile::tempdir;

// -- mocks -------------------------------------------------------------------

/// Serves canned completions in order and records each request's text.
struct Mock {
    responses: Mutex<VecDeque<Completion>>,
    requests: Arc<Mutex<Vec<String>>>,
}
impl Mock {
    fn new(r: Vec<Completion>) -> (Self, Arc<Mutex<Vec<String>>>) {
        let requests = Arc::new(Mutex::new(Vec::new()));
        (
            Self {
                responses: Mutex::new(r.into()),
                requests: requests.clone(),
            },
            requests,
        )
    }
}
#[async_trait]
impl Provider for Mock {
    async fn complete(&self, req: &CompletionRequest) -> Result<Completion, LlmError> {
        self.requests
            .lock()
            .unwrap()
            .push(format!("{}\n{:?}", req.system, req.messages));
        Ok(self
            .responses
            .lock()
            .unwrap()
            .pop_front()
            .expect("mock out of responses"))
    }
}

fn artifact(body: Value) -> Completion {
    Completion {
        content: format!(
            "```json\n{}\n```",
            json!({"type": "artifact", "content": body})
        ),
        reasoning: None,
        tool_calls: vec![],
        usage: Usage::default(),
        stop_reason: None,
    }
}

/// `(group, [(id, title)])` from `on_fan_out`.
type FanOutEvent = (String, Vec<(String, String)>);
/// `(group, id, title, index, count)` from `on_fan_out_item`.
type ItemEvent = (String, String, String, usize, usize);

#[derive(Default)]
struct Cb {
    started: Mutex<Vec<String>>,
    skipped: Mutex<Vec<String>>,
    fan_outs: Mutex<Vec<FanOutEvent>>,
    items: Mutex<Vec<ItemEvent>>,
}
#[async_trait]
impl RunnerCallbacks for Cb {
    async fn on_questions(&self, _p: &str, _q: &[String]) -> String {
        "answer".into()
    }
    async fn on_human_gate(&self, _p: &str, _i: &str) -> String {
        "approved".into()
    }
    fn on_phase_start(&self, phase: &str, _i: usize, _t: usize) {
        self.started.lock().unwrap().push(phase.into());
    }
    fn on_phase_skipped(&self, phase: &str, _reason: &str) {
        self.skipped.lock().unwrap().push(phase.into());
    }
    fn on_fan_out(&self, group: &str, items: &[(String, String)]) {
        self.fan_outs
            .lock()
            .unwrap()
            .push((group.into(), items.to_vec()));
    }
    fn on_fan_out_item(&self, group: &str, id: &str, title: &str, index: usize, count: usize) {
        self.items
            .lock()
            .unwrap()
            .push((group.into(), id.into(), title.into(), index, count));
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

fn group(name: &str, nested: Vec<PhaseConfig>) -> PhaseConfig {
    let mut g = PhaseConfig::new(name);
    g.for_each = Some("artifacts.design.tasks".into());
    g.as_var = Some("task".into());
    g.phases = nested;
    g
}

fn tasks(ids: &[&str]) -> Value {
    json!({
        "tasks": ids
            .iter()
            .map(|id| json!({"id": id, "title": format!("Task {id}")}))
            .collect::<Vec<_>>()
    })
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

fn group_artifact(store: &Store, name: &str) -> Value {
    let text = std::fs::read_to_string(store.artifact_path("r1", name)).unwrap();
    let a: Value = serde_yaml_ng::from_str(&text).unwrap();
    a["content"].clone()
}

fn started(cb: &Cb) -> Vec<String> {
    cb.started.lock().unwrap().clone()
}

// -- tests -------------------------------------------------------------------

#[tokio::test]
async fn items_run_in_order_then_the_rest() {
    let d = tempdir().unwrap();
    let store = Store::new(d.path().join(".flows"));
    let (mock, _) = Mock::new(vec![
        artifact(tasks(&["1", "2", "3"])),
        artifact(json!({"summary": "did 1", "files_written": ["a.rs"]})),
        artifact(json!({"summary": "did 2", "files_written": ["a.rs", "b.rs"]})),
        artifact(json!({"summary": "did 3"})),
    ]);
    let cb = Cb::default();
    let phases = [
        agent("design"),
        group("implement", vec![agent("code"), script("check", "true")]),
        script("ship", "true"),
    ];
    let done = run(&phases, mock, &cb, d.path(), &store).await.unwrap();

    assert_eq!(
        started(&cb),
        [
            "design",
            "implement",
            "implement.1.code",
            "implement.1.check",
            "implement.2.code",
            "implement.2.check",
            "implement.3.code",
            "implement.3.check",
            "ship"
        ]
    );
    assert!(done.contains(&"implement.3.check".to_string()));
    let g = group_artifact(&store, "implement");
    assert_eq!(g["completed"], json!(["1", "2", "3"]));
    assert_eq!(g["items"][1]["title"], "Task 2");
    // Item results roll up, so `prior_artifacts['implement'].summary` works.
    assert_eq!(
        g["summary"],
        "[1 Task 1] did 1\n[2 Task 2] did 2\n[3 Task 3] did 3"
    );
    assert_eq!(g["files_written"], json!(["a.rs", "b.rs"]));
    let state = store.read_state("r1").unwrap().unwrap();
    assert_eq!(state.status, RunStatus::Completed);
    assert_eq!(state.fan_outs["implement"], ["1", "2", "3"]);

    let fan = cb.fan_outs.lock().unwrap();
    assert_eq!(fan[0].0, "implement");
    assert_eq!(fan[0].1[2], ("3".to_string(), "Task 3".to_string()));
    let items = cb.items.lock().unwrap();
    assert_eq!(items.len(), 3);
    assert_eq!(
        items[1],
        ("implement".into(), "2".into(), "Task 2".into(), 2, 3)
    );
}

#[tokio::test]
async fn empty_list_skips_the_group() {
    let d = tempdir().unwrap();
    let store = Store::new(d.path().join(".flows"));
    let (mock, _) = Mock::new(vec![artifact(json!({"tasks": []}))]);
    let cb = Cb::default();
    let phases = [
        agent("design"),
        group("implement", vec![agent("code")]),
        script("ship", "true"),
    ];
    run(&phases, mock, &cb, d.path(), &store).await.unwrap();
    assert_eq!(started(&cb), ["design", "implement", "ship"]);
    assert_eq!(*cb.skipped.lock().unwrap(), ["implement"]);
}

#[tokio::test]
async fn non_list_fails_at_the_group() {
    let d = tempdir().unwrap();
    let store = Store::new(d.path().join(".flows"));
    let (mock, _) = Mock::new(vec![artifact(json!({"tasks": "do it all"}))]);
    let cb = Cb::default();
    let phases = [agent("design"), group("implement", vec![agent("code")])];
    match run(&phases, mock, &cb, d.path(), &store).await {
        Err(RunError::Failed { phase, error }) => {
            assert_eq!(phase, "implement");
            assert!(
                error.contains("artifacts.design.tasks") && error.contains("a string"),
                "{error}"
            );
        }
        other => panic!("expected failure, got {other:?}"),
    }
}

#[tokio::test]
async fn prompts_see_the_item_and_finished_items() {
    let d = tempdir().unwrap();
    std::fs::write(
        d.path().join("code.md"),
        "ITEM {{ vars.task.title }} ({{ vars.task_index }}/{{ vars.task_count }}) \
         DONE[{% for t in vars.task_done %}{{ t.id }}={{ t.summary }};{% endfor %}]",
    )
    .unwrap();
    let store = Store::new(d.path().join(".flows"));
    let (mock, requests) = Mock::new(vec![
        artifact(tasks(&["a", "b"])),
        artifact(json!({"summary": "parser done"})),
        artifact(json!({"summary": "cli done"})),
    ]);
    let mut code = agent("code");
    code.prompt = Some("code.md".into());
    let cb = Cb::default();
    run(
        &[agent("design"), group("implement", vec![code])],
        mock,
        &cb,
        d.path(),
        &store,
    )
    .await
    .unwrap();
    let reqs = requests.lock().unwrap();
    assert!(reqs[1].contains("ITEM Task a (1/2) DONE[]"), "{}", reqs[1]);
    assert!(
        reqs[2].contains("ITEM Task b (2/2) DONE[a=parser done;]"),
        "{}",
        reqs[2]
    );
}

#[tokio::test]
async fn item_sees_its_own_artifacts_by_short_name() {
    let d = tempdir().unwrap();
    let store = Store::new(d.path().join(".flows"));
    let (mock, _) = Mock::new(vec![artifact(tasks(&["1", "2", "3"]))]);
    let mut only_two = script("b", "true");
    only_two.when = Some("artifacts.a.stdout.strip() == '2'".into());
    let cb = Cb::default();
    let phases = [
        agent("design"),
        group(
            "implement",
            vec![script("a", "echo {{ vars.task.id }}"), only_two],
        ),
    ];
    run(&phases, mock, &cb, d.path(), &store).await.unwrap();
    assert_eq!(
        *cb.skipped.lock().unwrap(),
        ["implement.1.b", "implement.3.b"]
    );
}

#[tokio::test]
async fn retry_reruns_only_the_failing_item() {
    let d = tempdir().unwrap();
    let store = Store::new(d.path().join(".flows"));
    let (mock, _) = Mock::new(vec![
        artifact(tasks(&["1", "2"])),
        artifact(json!({"summary": "1"})),
        artifact(json!({"summary": "2"})),
        artifact(json!({"summary": "2 again"})),
    ]);
    // Item 2's check fails once, then passes.
    let mut check = script(
        "check",
        "test {{ vars.task.id }} != 2 || test -f retried || { touch retried; exit 1; }",
    );
    check.on_failure = OnFailure::Retry("code".into());
    let cb = Cb::default();
    let phases = [
        agent("design"),
        group("implement", vec![agent("code"), check]),
    ];
    run(&phases, mock, &cb, d.path(), &store).await.unwrap();
    assert_eq!(
        started(&cb),
        [
            "design",
            "implement",
            "implement.1.code",
            "implement.1.check",
            "implement.2.code",
            "implement.2.check",
            "implement.2.code",
            "implement.2.check",
        ]
    );
    // The retry stays inside the item: one item event per item.
    assert_eq!(cb.items.lock().unwrap().len(), 2);
}

fn failing_two() -> Vec<PhaseConfig> {
    vec![script("check", "test {{ vars.task.id }} != 2")]
}

fn dep_tasks() -> Value {
    json!({"tasks": [
        {"id": "1"},
        {"id": "2"},
        {"id": "3", "depends_on": ["2"]},
        {"id": "4"},
    ]})
}

#[tokio::test]
async fn failing_item_stops_the_run_by_default() {
    let d = tempdir().unwrap();
    let store = Store::new(d.path().join(".flows"));
    let (mock, _) = Mock::new(vec![artifact(dep_tasks())]);
    let cb = Cb::default();
    let phases = [agent("design"), group("implement", failing_two())];
    match run(&phases, mock, &cb, d.path(), &store).await {
        Err(RunError::Failed { phase, .. }) => assert_eq!(phase, "implement.2.check"),
        other => panic!("expected failure, got {other:?}"),
    }
    assert!(!started(&cb).contains(&"implement.3.check".to_string()));
}

#[tokio::test]
async fn group_continue_skips_dependents_only() {
    let d = tempdir().unwrap();
    let store = Store::new(d.path().join(".flows"));
    let (mock, _) = Mock::new(vec![artifact(dep_tasks())]);
    let cb = Cb::default();
    let mut g = group("implement", failing_two());
    g.on_failure = OnFailure::Continue;
    let phases = [agent("design"), g, script("ship", "true")];
    run(&phases, mock, &cb, d.path(), &store).await.unwrap();

    let s = started(&cb);
    assert!(s.contains(&"implement.4.check".to_string()));
    assert!(s.contains(&"ship".to_string()));
    assert!(!s.contains(&"implement.3.check".to_string()));
    assert_eq!(*cb.skipped.lock().unwrap(), ["implement.3.check"]);
    let a = group_artifact(&store, "implement");
    assert_eq!(a["completed"], json!(["1", "4"]));
    assert_eq!(a["failed"], json!(["2"]));
    assert_eq!(a["skipped"], json!(["3"]));
}

#[tokio::test]
async fn re_entry_keeps_the_frozen_items() {
    let d = tempdir().unwrap();
    let store = Store::new(d.path().join(".flows"));
    let (mock, _) = Mock::new(vec![
        artifact(tasks(&["1", "2"])),
        artifact(json!({"verdict": "needs_fixes"})),
        // The redesign now has three tasks — the group keeps its two.
        artifact(tasks(&["1", "2", "3"])),
        artifact(json!({"verdict": "approved"})),
    ]);
    let mut verify = agent("verify");
    verify.loop_back_to = Some("design".into());
    verify.loop_condition = "content.verdict != 'approved'".into();
    let cb = Cb::default();
    let phases = [
        agent("design"),
        group("implement", vec![script("check", "true")]),
        verify,
    ];
    run(&phases, mock, &cb, d.path(), &store).await.unwrap();
    let s = started(&cb);
    assert_eq!(s.iter().filter(|p| *p == "implement.1.check").count(), 2);
    assert!(!s.contains(&"implement.3.check".to_string()));
    let state = store.read_state("r1").unwrap().unwrap();
    assert_eq!(state.fan_outs["implement"], ["1", "2"]);
}

#[tokio::test]
async fn re_entry_before_any_item_finished_re_evaluates() {
    let d = tempdir().unwrap();
    let store = Store::new(d.path().join(".flows"));
    let (mock, _) = Mock::new(vec![
        artifact(json!({"tasks": []})),
        artifact(json!({"verdict": "needs_fixes"})),
        artifact(tasks(&["1", "2", "3"])),
        artifact(json!({"verdict": "approved"})),
    ]);
    let mut verify = agent("verify");
    verify.loop_back_to = Some("design".into());
    verify.loop_condition = "content.verdict != 'approved'".into();
    let cb = Cb::default();
    let phases = [
        agent("design"),
        group("implement", vec![script("check", "true")]),
        verify,
    ];
    run(&phases, mock, &cb, d.path(), &store).await.unwrap();
    assert!(started(&cb).contains(&"implement.3.check".to_string()));
    let state = store.read_state("r1").unwrap().unwrap();
    assert_eq!(state.fan_outs["implement"], ["1", "2", "3"]);
}

#[tokio::test]
async fn software_template_runs_one_item_per_design_task() {
    let d = tempdir().unwrap();
    let repo = d.path().join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    // `@auto:test` / `@auto:lint` resolve to these targets.
    std::fs::write(repo.join("Makefile"), "test:\n\ttrue\nlint:\n\ttrue\n").unwrap();
    let flows = d.path().join("flows");
    let store = Store::new(d.path().join(".flows"));
    let (phases, _) =
        flows_core::templates::load_template(Some("software"), &repo, Some(&flows)).unwrap();

    let (mock, requests) = Mock::new(vec![
        artifact(json!({"goals": ["a cli"]})),
        artifact(json!({"title": "t", "requirements": [
            {"id": "REQ-1", "text": "parse args"},
            {"id": "REQ-2", "text": "print totals"}
        ]})),
        artifact(json!({"architecture": "small", "tasks": [
            {"id": "1", "title": "Skeleton", "requirements": ["REQ-1"]},
            {"id": "2", "title": "Totals", "requirements": ["REQ-2"], "depends_on": ["1"]}
        ]})),
        artifact(json!({"summary": "skeleton built", "files_written": []})),
        artifact(json!({"summary": "totals built", "files_written": []})),
        artifact(json!({"verdict": "approved", "unmet_requirements": []})),
    ]);
    let provider: Arc<dyn Provider> = Arc::new(mock);
    let factory = move |_: &LLMConfig| provider.clone();
    let cb = Cb::default();
    let done = run_pipeline(
        "r1",
        "a totals cli",
        &repo,
        &phases,
        "software",
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
        &cb,
        &store,
        None,
        Some(&flows),
    )
    .await
    .unwrap();

    let s = started(&cb);
    let pos = |n: &str| {
        s.iter()
            .position(|p| p == n)
            .unwrap_or_else(|| panic!("{n} not run: {s:?}"))
    };
    assert!(pos("implement.1.code") < pos("implement.1.check"));
    assert!(pos("implement.1.check") < pos("implement.2.code"));
    assert!(pos("implement.2.check") < pos("lint"));
    assert!(pos("test") < pos("verify"));
    // Nothing failed, so the fix pass is skipped.
    assert!(cb.skipped.lock().unwrap().contains(&"fix".to_string()));
    assert!(done.contains(&"commit".to_string()));

    // Each code agent saw only its own task's requirement, and task 2 saw task 1.
    let reqs = requests.lock().unwrap();
    assert!(reqs[3].contains("1 of 2: Skeleton") && reqs[3].contains("parse args"));
    assert!(!reqs[3].contains("print totals"));
    assert!(reqs[4].contains("2 of 2: Totals") && reqs[4].contains("skeleton built"));
    // verify sees the rolled-up summary of the tasks.
    assert!(reqs[5].contains("[2 Totals] totals built"), "{}", reqs[5]);
}
