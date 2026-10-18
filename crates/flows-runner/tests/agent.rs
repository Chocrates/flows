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

// -- stall / output-cap / dead-backend guards ---------------------------------

/// What one request looked like: offered tool names, whether calls were
/// forbidden, and the text of its last message.
#[derive(Debug)]
struct Seen {
    tools: Vec<String>,
    no_tool_calls: bool,
    disable_thinking: bool,
    last: String,
}

/// Scripted provider that also records every request.
struct Recorder {
    responses: Mutex<VecDeque<Completion>>,
    seen: Mutex<Vec<Seen>>,
}

impl Recorder {
    fn new(responses: Vec<Completion>) -> Self {
        Self {
            responses: Mutex::new(responses.into()),
            seen: Mutex::new(Vec::new()),
        }
    }
    fn seen(&self) -> std::sync::MutexGuard<'_, Vec<Seen>> {
        self.seen.lock().unwrap()
    }
}

#[async_trait]
impl Provider for Recorder {
    async fn complete(&self, req: &CompletionRequest) -> Result<Completion, LlmError> {
        self.seen.lock().unwrap().push(Seen {
            tools: req.tools.iter().map(|t| t.name.clone()).collect(),
            no_tool_calls: req.no_tool_calls,
            disable_thinking: req.disable_thinking,
            last: req
                .messages
                .last()
                .and_then(|m| m.content.clone())
                .unwrap_or_default(),
        });
        Ok(self
            .responses
            .lock()
            .unwrap()
            .pop_front()
            .expect("recorder ran out of responses"))
    }
}

fn capped(content: &str) -> Completion {
    Completion {
        stop_reason: Some("length".into()),
        ..text(content)
    }
}

async fn run_with(
    phase: &PhaseConfig,
    repo: &Path,
    provider: &dyn Provider,
    search: &WebSearchConfig,
) -> flows_core::models::Artifact {
    run_agent_phase(
        phase,
        "idea",
        repo,
        &empty_artifacts(),
        "feature",
        &LLMConfig::default(),
        provider,
        &NoopCallbacks,
        None,
        &ContextConfig::default(),
        search,
        0.0,
        0.0,
        None,
    )
    .await
    .artifact
}

fn read_only_phase() -> PhaseConfig {
    let mut p = PhaseConfig::new("plan");
    p.tools = ToolsConfig {
        files: true,
        read_only: true,
        ..Default::default()
    };
    p
}

fn repo_with_files() -> tempfile::TempDir {
    let d = tempfile::tempdir().unwrap();
    std::fs::write(d.path().join("a.txt"), "alpha").unwrap();
    std::fs::write(d.path().join("b.txt"), "beta").unwrap();
    d
}

const READ_A: &str = r#"{"path": "a.txt"}"#;

#[tokio::test]
async fn capped_reply_is_nudged_not_taken_as_artifact() {
    let provider = Recorder::new(vec![
        capped("Let me think about every edge case of"),
        artifact_json(r#"{"ok": true}"#),
    ]);
    let art = run_with(
        &files_phase(),
        Path::new("/tmp"),
        &provider,
        &WebSearchConfig::default(),
    )
    .await;
    assert_eq!(art.content["ok"], true, "{art:?}");
    let seen = provider.seen();
    assert!(seen[1].last.contains("output limit"), "{seen:?}");
}

#[tokio::test]
async fn capped_twice_fails_naming_max_tokens() {
    let provider = Recorder::new(vec![capped(""), capped("still thinking")]);
    let art = run_with(
        &files_phase(),
        Path::new("/tmp"),
        &provider,
        &WebSearchConfig::default(),
    )
    .await;
    let err = art.content["error"].as_str().unwrap_or("");
    assert!(err.contains("max_tokens"), "{art:?}");
}

#[tokio::test]
async fn unreachable_search_backend_drops_the_tool() {
    let mut phase = read_only_phase();
    phase.tools.web_search = true;
    // Nothing listens on port 1: the request fails to connect.
    let search = WebSearchConfig {
        backend: "searxng".into(),
        base_url: "http://127.0.0.1:1".into(),
        ..Default::default()
    };
    let provider = Recorder::new(vec![
        tool("web_search", r#"{"query": "rust duration"}"#),
        artifact_json(r#"{"ok": true}"#),
    ]);
    let art = run_with(&phase, Path::new("/tmp"), &provider, &search).await;
    assert_eq!(art.content["ok"], true, "{art:?}");
    let seen = provider.seen();
    assert!(seen[0].tools.contains(&"web_search".to_string()));
    assert!(
        !seen[1].tools.contains(&"web_search".to_string()),
        "{seen:?}"
    );
}

#[tokio::test]
async fn one_letter_lookups_are_refused_and_stall_the_phase() {
    let mut phase = read_only_phase();
    phase.tools.lookup = true;
    // A degenerating model walking the alphabet: every query is refused before
    // any request goes out, so the stall guard nudges, then forces a reply.
    let mut script: Vec<Completion> = ["x", "q", "p", "o", "n"]
        .iter()
        .map(|q| {
            tool(
                "lookup",
                &format!(r#"{{"source": "crates_io", "query": "{q}"}}"#),
            )
        })
        .collect();
    script.push(artifact_json(r#"{"ok": true}"#));
    let provider = Recorder::new(script);
    let art = run_with(
        &phase,
        Path::new("/tmp"),
        &provider,
        &WebSearchConfig::default(),
    )
    .await;
    assert_eq!(art.content["ok"], true, "{art:?}");
    let seen = provider.seen();
    assert_eq!(seen.len(), 6);
    assert!(seen[1].last.contains("too short"), "{seen:?}");
    assert!(seen[3].last.contains("nothing new"), "{seen:?}");
    assert!(seen[5].no_tool_calls, "{seen:?}");
}

#[tokio::test]
async fn refused_shell_commands_count_as_stalled() {
    let repo = repo_with_files();
    let mut phase = files_phase();
    phase.tools.bash = Some(BashConfig {
        allowed: vec!["go*".into()],
        ..Default::default()
    });
    // Different commands each round, all refused by the allowlist.
    let script: Vec<Completion> = (0..9)
        .map(|i| tool("bash", &format!(r#"{{"command": "ls -la dir{i}"}}"#)))
        .collect();
    let provider = Recorder::new(script);
    let art = run_with(&phase, repo.path(), &provider, &WebSearchConfig::default()).await;
    let err = art.content["error"].as_str().unwrap_or("");
    assert!(err.contains("nothing new"), "{art:?}");
    assert!(provider.seen()[1].last.contains("`list_files`"));
}

#[tokio::test]
async fn stalled_read_only_phase_is_nudged_then_forced_to_reply() {
    let repo = repo_with_files();
    // One fresh read, then five re-reads of the unchanged file.
    let mut script: Vec<Completion> = (0..6).map(|_| tool("read_file", READ_A)).collect();
    script.push(artifact_json(r#"{"ok": true}"#));
    let provider = Recorder::new(script);
    let art = run_with(
        &read_only_phase(),
        repo.path(),
        &provider,
        &WebSearchConfig::default(),
    )
    .await;
    assert_eq!(art.content["ok"], true, "{art:?}");
    let seen = provider.seen();
    assert_eq!(seen.len(), 7);
    // Third stalled round (request 5) carries the nudge.
    assert!(seen[4].last.contains("nothing new"), "{seen:?}");
    // Fifth stalled round switches tools off for the reply.
    assert!(seen[..6].iter().all(|s| !s.no_tool_calls));
    assert!(seen[6].no_tool_calls);
    assert!(seen[6].last.contains("switched off"), "{seen:?}");
}

#[tokio::test]
async fn stalled_write_capable_phase_is_aborted() {
    let repo = repo_with_files();
    let script: Vec<Completion> = (0..9).map(|_| tool("read_file", READ_A)).collect();
    let provider = Recorder::new(script);
    let art = run_with(
        &files_phase(),
        repo.path(),
        &provider,
        &WebSearchConfig::default(),
    )
    .await;
    let err = art.content["error"].as_str().unwrap_or("");
    assert!(err.contains("nothing new"), "{art:?}");
    assert!(provider.seen().iter().all(|s| !s.no_tool_calls));
}

#[tokio::test]
async fn fresh_read_resets_the_stall_count() {
    let repo = repo_with_files();
    let read_b = r#"{"path": "b.txt"}"#;
    let provider = Recorder::new(vec![
        tool("read_file", READ_A),
        tool("read_file", READ_A),
        tool("read_file", READ_A),
        tool("read_file", read_b),
        tool("read_file", READ_A),
        tool("read_file", READ_A),
        artifact_json(r#"{"ok": true}"#),
    ]);
    let art = run_with(
        &read_only_phase(),
        repo.path(),
        &provider,
        &WebSearchConfig::default(),
    )
    .await;
    assert_eq!(art.content["ok"], true, "{art:?}");
    assert!(
        provider
            .seen()
            .iter()
            .all(|s| !s.last.contains("nothing new")),
        "no nudge expected"
    );
}

#[tokio::test]
async fn repeated_identical_listing_counts_as_stalled() {
    let repo = repo_with_files();
    let list = r#"{"path": "."}"#;
    let phase = read_only_phase();
    let provider = Recorder::new(vec![
        tool("list_files", list),
        tool("list_files", list),
        tool("list_files", list),
        tool("list_files", list),
        artifact_json(r#"{"ok": true}"#),
    ]);
    let art = run_with(&phase, repo.path(), &provider, &WebSearchConfig::default()).await;
    assert_eq!(art.content["ok"], true, "{art:?}");
    let seen = provider.seen();
    assert!(seen[4].last.contains("nothing new"), "{seen:?}");
}

#[tokio::test]
async fn exploring_only_known_files_is_nudged_then_forced() {
    let repo = repo_with_files();
    // Read both files, then ten listings that differ in their arguments (so
    // none is an identical repeat) but only ever show the two known files.
    let mut script = vec![
        tool("read_file", READ_A),
        tool("read_file", r#"{"path": "b.txt"}"#),
    ];
    for n in 0..10 {
        script.push(tool("list_files", &format!(r#"{{"path": ".", "n": {n}}}"#)));
    }
    script.push(artifact_json(r#"{"ok": true}"#));
    let provider = Recorder::new(script);
    let art = run_with(
        &read_only_phase(),
        repo.path(),
        &provider,
        &WebSearchConfig::default(),
    )
    .await;
    assert_eq!(art.content["ok"], true, "{art:?}");
    let seen = provider.seen();
    // Six listings that add nothing → nudge; ten → tools off for the reply.
    assert!(
        seen[8].last.contains("nothing you had not already seen"),
        "{seen:?}"
    );
    assert!(seen[..12].iter().all(|s| !s.no_tool_calls));
    assert!(seen[12].no_tool_calls, "{seen:?}");
    assert_eq!(seen.len(), 13);
}

#[tokio::test]
async fn thinking_false_reaches_the_request() {
    let mut phase = read_only_phase();
    let provider = Recorder::new(vec![artifact_json(r#"{"ok": true}"#)]);
    run_with(
        &phase,
        Path::new("/tmp"),
        &provider,
        &WebSearchConfig::default(),
    )
    .await;
    assert!(
        !provider.seen()[0].disable_thinking,
        "default leaves thinking alone"
    );

    phase.thinking = Some(false);
    let provider = Recorder::new(vec![artifact_json(r#"{"ok": true}"#)]);
    run_with(
        &phase,
        Path::new("/tmp"),
        &provider,
        &WebSearchConfig::default(),
    )
    .await;
    assert!(provider.seen()[0].disable_thinking);
}

#[tokio::test]
async fn tool_call_written_as_text_after_tools_are_off_is_not_the_artifact() {
    let repo = repo_with_files();
    let mut script: Vec<Completion> = (0..6).map(|_| tool("read_file", READ_A)).collect();
    // Tools are now off; the model writes the call out in Qwen's XML instead.
    script.push(text(
        "<tool_call>\n<function=read_file>\n<parameter=path>\na.txt\n</parameter>\n</function>\n</tool_call>",
    ));
    script.push(artifact_json(r#"{"ok": true}"#));
    let provider = Recorder::new(script);
    let art = run_with(
        &read_only_phase(),
        repo.path(),
        &provider,
        &WebSearchConfig::default(),
    )
    .await;
    assert_eq!(art.content["ok"], true, "{art:?}");
    let seen = provider.seen();
    assert!(seen[7].no_tool_calls);
    assert!(
        seen[7].last.contains("that tool call was not run"),
        "{seen:?}"
    );
}
