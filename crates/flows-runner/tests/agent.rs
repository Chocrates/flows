//! Agent tool-loop tests using a scripted mock Provider (no network).

use std::collections::VecDeque;
use std::path::Path;
use std::sync::Mutex;

use async_trait::async_trait;
use serde_json::Map;

use flows_core::models::{BashConfig, LLMConfig, PhaseConfig, ToolsConfig};
use flows_core::models::{ContextConfig, WebSearchConfig};
use flows_llm::types::{Completion, CompletionRequest, LlmError, ToolCall, Usage};
use flows_llm::Provider;
use flows_runner::agent::{parse_response, run_agent_phase, AgentCallbacks, NoopCallbacks};

// -- mock provider -----------------------------------------------------------

struct Mock {
    responses: Mutex<VecDeque<Completion>>,
}

impl Mock {
    fn new(responses: Vec<Completion>) -> Self {
        Self {
            responses: Mutex::new(responses.into()),
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
            .expect("mock ran out of responses"))
    }
}

fn text(content: &str) -> Completion {
    Completion {
        content: content.into(),
        reasoning: None,
        tool_calls: vec![],
        usage: Usage::default(),
        stop_reason: None,
    }
}

fn tool(name: &str, args: &str) -> Completion {
    Completion {
        content: String::new(),
        reasoning: None,
        tool_calls: vec![ToolCall {
            id: "t1".into(),
            name: name.into(),
            arguments: args.into(),
        }],
        usage: Usage::default(),
        stop_reason: None,
    }
}

fn artifact_json(body: &str) -> Completion {
    text(&format!(
        "```json\n{{\"type\": \"artifact\", \"content\": {body}}}\n```"
    ))
}

fn files_phase() -> PhaseConfig {
    let mut p = PhaseConfig::new("plan");
    p.tools = ToolsConfig {
        files: true,
        bash: Some(BashConfig::default()),
        ..Default::default()
    };
    p
}

fn empty_artifacts() -> Map<String, serde_json::Value> {
    Map::new()
}

// -- tests -------------------------------------------------------------------

#[tokio::test]
async fn returns_artifact_from_json_response() {
    let provider = Mock::new(vec![artifact_json(r#"{"summary": "done"}"#)]);
    let art = run_agent_phase(
        &files_phase(),
        "build a thing",
        Path::new("/tmp"),
        &empty_artifacts(),
        "feature",
        &LLMConfig::default(),
        &provider,
        &NoopCallbacks,
        None,
        &ContextConfig::default(),
        &WebSearchConfig::default(),
        0.0,
        0.0,
        None,
    )
    .await
    .artifact;
    assert_eq!(art.content["summary"], "done");
    assert_eq!(art.phase, "plan");
}

#[tokio::test]
async fn dispatches_write_file_then_returns_artifact() {
    let d = tempfile::tempdir().unwrap();
    let provider = Mock::new(vec![
        tool("write_file", r#"{"path": "out.txt", "content": "hello"}"#),
        artifact_json(r#"{"wrote": true}"#),
    ]);
    let art = run_agent_phase(
        &files_phase(),
        "idea",
        d.path(),
        &empty_artifacts(),
        "feature",
        &LLMConfig::default(),
        &provider,
        &NoopCallbacks,
        None,
        &ContextConfig::default(),
        &WebSearchConfig::default(),
        0.0,
        0.0,
        None,
    )
    .await
    .artifact;
    assert_eq!(art.content["wrote"], true);
    assert_eq!(
        std::fs::read_to_string(d.path().join("out.txt")).unwrap(),
        "hello"
    );
}

#[tokio::test]
async fn reads_file_via_tool() {
    let d = tempfile::tempdir().unwrap();
    std::fs::write(d.path().join("hello.txt"), "world").unwrap();
    let provider = Mock::new(vec![
        tool("read_file", r#"{"path": "hello.txt"}"#),
        artifact_json(r#"{"read": "ok"}"#),
    ]);
    let art = run_agent_phase(
        &files_phase(),
        "idea",
        d.path(),
        &empty_artifacts(),
        "feature",
        &LLMConfig::default(),
        &provider,
        &NoopCallbacks,
        None,
        &ContextConfig::default(),
        &WebSearchConfig::default(),
        0.0,
        0.0,
        None,
    )
    .await
    .artifact;
    assert_eq!(art.content["read"], "ok");
}

#[derive(Default)]
struct CaptureCb {
    results: Mutex<Vec<String>>,
}
#[async_trait]
impl AgentCallbacks for CaptureCb {
    fn on_tool_result(&self, _name: &str, summary: &str, _is_error: bool) {
        self.results.lock().unwrap().push(summary.to_string());
    }
}

#[tokio::test]
async fn reread_of_unchanged_file_is_deduped() {
    let d = tempfile::tempdir().unwrap();
    std::fs::write(d.path().join("hello.txt"), "world").unwrap();
    let provider = Mock::new(vec![
        tool("read_file", r#"{"path": "hello.txt"}"#),
        tool("read_file", r#"{"path": "hello.txt"}"#), // identical re-read
        artifact_json(r#"{"read": "ok"}"#),
    ]);
    let cb = CaptureCb::default();
    run_agent_phase(
        &files_phase(),
        "idea",
        d.path(),
        &empty_artifacts(),
        "feature",
        &LLMConfig::default(),
        &provider,
        &cb,
        None,
        &ContextConfig::default(),
        &WebSearchConfig::default(),
        0.0,
        0.0,
        None,
    )
    .await;
    let results = cb.results.lock().unwrap();
    assert!(
        results[0].contains("world"),
        "first read full: {:?}",
        results[0]
    );
    assert!(
        results[1].contains("already read"),
        "second read should be deduped: {:?}",
        results[1]
    );
}

struct AnswerCb(&'static str);
#[async_trait]
impl AgentCallbacks for AnswerCb {
    async fn on_questions(&self, _q: &[String]) -> String {
        self.0.to_string()
    }
}

#[tokio::test]
async fn asks_questions_then_proceeds() {
    let provider = Mock::new(vec![
        text(
            r#"```json
{"type": "questions", "questions": ["which color?"]}
```"#,
        ),
        artifact_json(r#"{"done": 1}"#),
    ]);
    let art = run_agent_phase(
        &files_phase(),
        "idea",
        Path::new("/tmp"),
        &empty_artifacts(),
        "feature",
        &LLMConfig::default(),
        &provider,
        &AnswerCb("blue"),
        None,
        &ContextConfig::default(),
        &WebSearchConfig::default(),
        0.0,
        0.0,
        None,
    )
    .await
    .artifact;
    assert_eq!(art.content["done"], 1);
}

#[tokio::test]
async fn max_rounds_exceeded_returns_error() {
    // always questions, never an artifact → exhausts the (small) budget
    let mut phase = files_phase();
    phase.max_clarification_rounds = 1;
    phase.max_tool_rounds = Some(1);
    let q = text(
        r#"```json
{"type": "questions", "questions": ["q?"]}
```"#,
    );
    let provider = Mock::new(vec![q.clone(), q.clone(), q.clone(), q]);
    let art = run_agent_phase(
        &phase,
        "idea",
        Path::new("/tmp"),
        &empty_artifacts(),
        "feature",
        &LLMConfig::default(),
        &provider,
        &NoopCallbacks,
        None,
        &ContextConfig::default(),
        &WebSearchConfig::default(),
        0.0,
        0.0,
        None,
    )
    .await
    .artifact;
    assert_eq!(art.content["error"], "max rounds exceeded");
}

#[tokio::test]
async fn degenerate_tool_loop_aborts() {
    // repeatedly call read_file with no path → identical error every round
    let bad = tool("read_file", "{}");
    let provider = Mock::new(vec![
        bad.clone(),
        bad.clone(),
        bad.clone(),
        bad.clone(),
        bad,
    ]);
    let art = run_agent_phase(
        &files_phase(),
        "idea",
        Path::new("/tmp"),
        &empty_artifacts(),
        "feature",
        &LLMConfig::default(),
        &provider,
        &NoopCallbacks,
        None,
        &ContextConfig::default(),
        &WebSearchConfig::default(),
        0.0,
        0.0,
        None,
    )
    .await
    .artifact;
    assert!(art.content["error"].as_str().unwrap().contains("aborted"));
}

#[tokio::test]
async fn noop_write_loop_aborts() {
    // Model writes the same file with identical content every round. The first
    // write lands new content; every write after is a byte-for-byte no-op — the
    // error/no-write guards can't see it (the write "succeeds"), so the no-op
    // guard must catch it and abort with a loop-detected error.
    let d = tempfile::tempdir().unwrap();
    let w = tool(
        "write_file",
        r#"{"path": "Cargo.toml", "content": "[workspace]"}"#,
    );
    let provider = Mock::new(vec![
        w.clone(),
        w.clone(),
        w.clone(),
        w.clone(),
        w.clone(),
        w.clone(),
        w.clone(),
        w.clone(),
        w.clone(),
        w,
    ]);
    let art = run_agent_phase(
        &files_phase(),
        "idea",
        d.path(),
        &empty_artifacts(),
        "feature",
        &LLMConfig::default(),
        &provider,
        &NoopCallbacks,
        None,
        &ContextConfig::default(),
        &WebSearchConfig::default(),
        0.0,
        0.0,
        None,
    )
    .await
    .artifact;
    let err = art.content["error"].as_str().unwrap();
    assert!(err.contains("aborted"), "unexpected error: {err}");
    assert!(err.contains("loop detected"), "unexpected error: {err}");
}

#[tokio::test]
async fn write_file_artifact_envelope_is_accepted() {
    // Model "emits" its artifact by write_file-ing the envelope to a file instead
    // of returning it as a reply. The recovery path should accept it and finish the
    // phase with that content — not loop.
    let d = tempfile::tempdir().unwrap();
    let provider = Mock::new(vec![tool(
        "write_file",
        r#"{"path": "artifact.json", "content": "{\"type\": \"artifact\", \"content\": {\"summary\": \"done via file\"}}"}"#,
    )]);
    let art = run_agent_phase(
        &files_phase(),
        "idea",
        d.path(),
        &empty_artifacts(),
        "feature",
        &LLMConfig::default(),
        &provider,
        &NoopCallbacks,
        None,
        &ContextConfig::default(),
        &WebSearchConfig::default(),
        0.0,
        0.0,
        None,
    )
    .await
    .artifact;
    assert_eq!(art.content["summary"], "done via file");
}

#[tokio::test]
async fn artifact_claiming_files_without_writing_fails() {
    // Write-capable phase emits an artifact reporting files_written but never called
    // write_file — the model inlined code instead. Must fail, not "succeed" empty.
    let d = tempfile::tempdir().unwrap();
    let provider = Mock::new(vec![artifact_json(
        r#"{"files_written": ["src/main.rs", "Cargo.toml"], "summary": "done"}"#,
    )]);
    let art = run_agent_phase(
        &files_phase(),
        "idea",
        d.path(),
        &empty_artifacts(),
        "feature",
        &LLMConfig::default(),
        &provider,
        &NoopCallbacks,
        None,
        &ContextConfig::default(),
        &WebSearchConfig::default(),
        0.0,
        0.0,
        None,
    )
    .await
    .artifact;
    let err = art.content["error"].as_str().unwrap();
    assert!(err.contains("no file was actually written"), "{err}");
}

#[tokio::test]
async fn artifact_with_real_writes_succeeds() {
    // Same shape, but the model actually wrote a file first — must NOT trip the guard.
    let d = tempfile::tempdir().unwrap();
    let provider = Mock::new(vec![
        tool(
            "write_file",
            r#"{"path": "src/main.rs", "content": "fn main(){}"}"#,
        ),
        artifact_json(r#"{"files_written": ["src/main.rs"], "summary": "done"}"#),
    ]);
    let art = run_agent_phase(
        &files_phase(),
        "idea",
        d.path(),
        &empty_artifacts(),
        "feature",
        &LLMConfig::default(),
        &provider,
        &NoopCallbacks,
        None,
        &ContextConfig::default(),
        &WebSearchConfig::default(),
        0.0,
        0.0,
        None,
    )
    .await
    .artifact;
    assert_eq!(art.content["summary"], "done");
}

#[tokio::test]
async fn repeated_empty_responses_fail() {
    // Model keeps returning empty (reasoned itself out, wrote nothing). Each empty
    // is nudged; after MAX_EMPTY_RESPONSES the phase fails — never accepted as an
    // empty `raw_output` artifact.
    let provider = Mock::new(vec![text(""), text("  \n "), text("   ")]);
    let art = run_agent_phase(
        &files_phase(),
        "idea",
        Path::new("/tmp"),
        &empty_artifacts(),
        "feature",
        &LLMConfig::default(),
        &provider,
        &NoopCallbacks,
        None,
        &ContextConfig::default(),
        &WebSearchConfig::default(),
        0.0,
        0.0,
        None,
    )
    .await
    .artifact;
    let err = art.content["error"].as_str().unwrap_or("");
    assert!(err.contains("empty response"), "unexpected: {art:?}");
}

#[tokio::test]
async fn empty_response_then_recovers() {
    // One empty response gets nudged; the model then produces a real artifact and
    // the phase succeeds — a single hiccup must not kill the run.
    let provider = Mock::new(vec![text(""), artifact_json(r#"{"ok": true}"#)]);
    let art = run_agent_phase(
        &files_phase(),
        "idea",
        Path::new("/tmp"),
        &empty_artifacts(),
        "feature",
        &LLMConfig::default(),
        &provider,
        &NoopCallbacks,
        None,
        &ContextConfig::default(),
        &WebSearchConfig::default(),
        0.0,
        0.0,
        None,
    )
    .await
    .artifact;
    assert_eq!(art.content["ok"], true, "should recover: {art:?}");
}

#[tokio::test]
async fn cost_cap_aborts() {
    let provider = Mock::new(vec![artifact_json(r#"{"x": 1}"#)]);
    let art = run_agent_phase(
        &files_phase(),
        "idea",
        Path::new("/tmp"),
        &empty_artifacts(),
        "feature",
        &LLMConfig::default(),
        &provider,
        &NoopCallbacks,
        None,
        &ContextConfig::default(),
        &WebSearchConfig::default(),
        1.0, // max_cost
        5.0, // prior_cost already over
        None,
    )
    .await
    .artifact;
    assert!(art.content["error"]
        .as_str()
        .unwrap()
        .contains("Cost limit"));
}

// -- parse_response ----------------------------------------------------------

#[test]
fn parse_fenced_artifact() {
    let v =
        parse_response("prose\n```json\n{\"type\": \"artifact\", \"content\": {\"a\": 1}}\n```")
            .unwrap();
    assert_eq!(v["type"], "artifact");
    assert_eq!(v["content"]["a"], 1);
}

#[test]
fn parse_bare_questions() {
    let v = parse_response(r#"{"type": "questions", "questions": ["x"]}"#).unwrap();
    assert_eq!(v["type"], "questions");
}

#[test]
fn parse_non_structured_is_none() {
    assert!(parse_response("just some prose, no json").is_none());
    assert!(parse_response(r#"{"type": "other", "x": 1}"#).is_none());
}

// A provider that panics if the model is ever called.
struct NeverProvider;
#[async_trait]
impl Provider for NeverProvider {
    async fn complete(&self, _req: &CompletionRequest) -> Result<Completion, LlmError> {
        panic!("provider must not be called after cancellation");
    }
}

struct CancelledCb;
#[async_trait]
impl AgentCallbacks for CancelledCb {
    fn cancelled(&self) -> bool {
        true
    }
}

// Provider whose response takes a long time — stands in for a slow local model.
struct SlowProvider;
#[async_trait]
impl Provider for SlowProvider {
    async fn complete(&self, _req: &CompletionRequest) -> Result<Completion, LlmError> {
        tokio::time::sleep(std::time::Duration::from_secs(5)).await;
        Ok(Completion {
            content: "too late".into(),
            reasoning: None,
            tool_calls: vec![],
            usage: Usage::default(),
            stop_reason: None,
        })
    }
}

// Not cancelled at the round-top check, then cancelled while the stream is in
// flight — so only the mid-stream race can catch it.
struct CancelAfterFirstCheck(std::sync::atomic::AtomicUsize);
#[async_trait]
impl AgentCallbacks for CancelAfterFirstCheck {
    fn cancelled(&self) -> bool {
        self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst) > 0
    }
}

#[tokio::test]
async fn cancel_mid_stream_aborts_without_waiting_out_the_call() {
    let start = std::time::Instant::now();
    let result = run_agent_phase(
        &files_phase(),
        "idea",
        Path::new("/tmp"),
        &empty_artifacts(),
        "feature",
        &LLMConfig::default(),
        &SlowProvider,
        &CancelAfterFirstCheck(std::sync::atomic::AtomicUsize::new(0)),
        None,
        &ContextConfig::default(),
        &WebSearchConfig::default(),
        0.0,
        0.0,
        None,
    )
    .await;
    assert_eq!(
        result
            .artifact
            .content
            .get("error")
            .and_then(|e| e.as_str()),
        Some("cancelled")
    );
    // Must abort mid-stream (~300ms poll), not wait out the 5s call.
    assert!(
        start.elapsed() < std::time::Duration::from_secs(3),
        "cancel took too long: {:?}",
        start.elapsed()
    );
}

#[tokio::test]
async fn cancelled_aborts_before_calling_model() {
    let result = run_agent_phase(
        &files_phase(),
        "idea",
        Path::new("/tmp"),
        &empty_artifacts(),
        "feature",
        &LLMConfig::default(),
        &NeverProvider,
        &CancelledCb,
        None,
        &ContextConfig::default(),
        &WebSearchConfig::default(),
        0.0,
        0.0,
        None,
    )
    .await;
    // returns a cancelled marker, zero cost, and never touched the provider
    assert_eq!(
        result
            .artifact
            .content
            .get("error")
            .and_then(|e| e.as_str()),
        Some("cancelled")
    );
    assert_eq!(result.cost_usd, 0.0);
}

// Provider that records the system prompt it was sent.
struct Capture {
    system: Mutex<String>,
}
#[async_trait]
impl Provider for Capture {
    async fn complete(&self, req: &CompletionRequest) -> Result<Completion, LlmError> {
        *self.system.lock().unwrap() = req.system.clone();
        Ok(artifact_json(r#"{"ok": true}"#))
    }
}

#[tokio::test]
async fn agents_md_is_appended_to_system_prompt() {
    let repo = tempfile::tempdir().unwrap();
    let flows = tempfile::tempdir().unwrap();
    std::fs::write(
        repo.path().join("AGENTS.md"),
        "Always use `make test` for tests.",
    )
    .unwrap();
    std::fs::write(flows.path().join("AGENTS.md"), "Global: prefer make.").unwrap();

    let provider = Capture {
        system: Mutex::new(String::new()),
    };
    let _ = run_agent_phase(
        &files_phase(),
        "idea",
        repo.path(),
        &empty_artifacts(),
        "feature",
        &LLMConfig::default(),
        &provider,
        &NoopCallbacks,
        Some(flows.path()),
        &ContextConfig::default(),
        &WebSearchConfig::default(),
        0.0,
        0.0,
        None,
    )
    .await;

    let sys = provider.system.lock().unwrap().clone();
    assert!(
        sys.contains("AGENTS.md"),
        "system prompt should mark the AGENTS.md section"
    );
    assert!(
        sys.contains("Always use `make test`"),
        "project AGENTS.md missing"
    );
    assert!(
        sys.contains("Global: prefer make"),
        "global AGENTS.md missing"
    );
}

#[test]
fn read_only_tools_omit_write_defs() {
    use flows_core::models::{ContextConfig, ToolsConfig};
    let ctx = ContextConfig::default();
    let ro = ToolsConfig {
        files: true,
        read_only: true,
        ..Default::default()
    };
    let names: Vec<String> = flows_runner::agent::build_tool_defs(&ro, &ctx)
        .into_iter()
        .map(|d| d.name)
        .collect();
    assert!(names.contains(&"read_file".to_string()));
    assert!(names.contains(&"list_files".to_string()));
    assert!(!names.contains(&"write_file".to_string()));
    assert!(!names.contains(&"make_dir".to_string()));
    // rw still has writes
    let rw = ToolsConfig {
        files: true,
        ..Default::default()
    };
    let rw_names: Vec<String> = flows_runner::agent::build_tool_defs(&rw, &ctx)
        .into_iter()
        .map(|d| d.name)
        .collect();
    assert!(rw_names.contains(&"write_file".to_string()));
}

#[test]
fn context_flags_gate_search_and_range() {
    use flows_core::models::{ContextConfig, ToolsConfig};
    let tools = ToolsConfig {
        files: true,
        read_only: true, // search_code must survive read-only phases
        ..Default::default()
    };
    // Baseline: no search_code tool, read_file has no range args.
    let base = flows_runner::agent::build_tool_defs(&tools, &ContextConfig::default());
    assert!(!base.iter().any(|d| d.name == "search_code"));
    let rf = base.iter().find(|d| d.name == "read_file").unwrap();
    assert!(rf.parameters["properties"].get("start_line").is_none());

    // Flags on: search_code appears, read_file gains the line window.
    let on = ContextConfig {
        search_tool: true,
        range_reads: true,
        ..ContextConfig::default()
    };
    let defs = flows_runner::agent::build_tool_defs(&tools, &on);
    assert!(defs.iter().any(|d| d.name == "search_code"));
    let rf = defs.iter().find(|d| d.name == "read_file").unwrap();
    assert!(rf.parameters["properties"].get("start_line").is_some());
}

#[test]
fn offline_bash_tells_the_model() {
    use flows_core::models::{BashConfig, ContextConfig, ToolsConfig};
    let desc = |offline: bool| {
        let tools = ToolsConfig {
            bash: Some(BashConfig {
                offline,
                ..BashConfig::default()
            }),
            ..Default::default()
        };
        flows_runner::agent::build_tool_defs(&tools, &ContextConfig::default())
            .into_iter()
            .find(|d| d.name == "bash")
            .unwrap()
            .description
    };
    assert!(desc(true).contains("NO network access"));
    assert!(!desc(false).contains("network"));
}
