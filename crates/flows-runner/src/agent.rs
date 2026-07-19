//! Agent phase — the LLM tool loop: build the request, stream the response,
//! dispatch tools, honour round/cost budgets, parse questions/artifacts.

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::OnceLock;

use async_trait::async_trait;
use regex::Regex;
use serde_json::{json, Map, Value};

use flows_core::models::{Artifact, ContextConfig, LLMConfig, PhaseConfig, ToolsConfig};
use flows_core::pricing::calculate_cost;
use flows_core::thinking::{model_supports_thinking, resolve_thinking};
use flows_llm::types::{Completion, CompletionRequest, Message, Role, ToolCall, ToolDef};
use flows_llm::Provider;
use flows_tools::{bash, code_search, files, plugin};

/// Prefixes identifying a `questions` envelope — these are internal plumbing and
/// are suppressed from the live token stream (artifacts/prose still stream).
const QUESTIONS_PREFIXES: &[&str] = &[
    "```json\n{\"type\": \"questions\"",
    "```json\n{ \"type\": \"questions\"",
    "{\"type\": \"questions\"",
    "{ \"type\": \"questions\"",
    "{\n  \"type\": \"questions\"",
    "{\n\"type\": \"questions\"",
];

/// The token category the agent emits for a classified response.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum TokenKind {
    Normal,
    Json,
}

fn is_questions(stripped: &str) -> bool {
    QUESTIONS_PREFIXES.iter().any(|p| stripped.starts_with(p))
}

/// Splits a streamed (non-suppressed) response into Normal prose and Json runs
/// by tracking ```json … ``` fences — so the artifact block is tagged even when
/// the model wraps it in prose. Fences split across tokens are handled by
/// withholding a short trailing tail until the next token arrives.
#[derive(Default)]
struct FenceSplitter {
    in_json: bool,
    buf: String,
}

impl FenceSplitter {
    fn feed(&mut self, text: &str, emit: &dyn Fn(TokenKind, &str)) {
        self.buf.push_str(text);
        loop {
            if !self.in_json {
                if let Some(i) = self.buf.find("```json") {
                    if i > 0 {
                        emit(TokenKind::Normal, &self.buf[..i]);
                    }
                    emit(TokenKind::Json, "```json");
                    self.buf.drain(..i + "```json".len());
                    self.in_json = true;
                    continue;
                }
            } else if let Some(i) = self.buf.find("```") {
                emit(TokenKind::Json, &self.buf[..i + 3]);
                self.buf.drain(..i + 3);
                self.in_json = false;
                continue;
            }
            break;
        }
        // Withhold up to 6 trailing bytes (a possible partial fence marker),
        // cut on a char boundary so we never split a multi-byte char.
        let mut cut = self.buf.len().saturating_sub(6);
        while cut > 0 && !self.buf.is_char_boundary(cut) {
            cut -= 1;
        }
        if cut > 0 {
            let kind = if self.in_json {
                TokenKind::Json
            } else {
                TokenKind::Normal
            };
            emit(kind, &self.buf[..cut]);
            self.buf.drain(..cut);
        }
    }

    fn finish(&mut self, emit: &dyn Fn(TokenKind, &str)) {
        if !self.buf.is_empty() {
            let kind = if self.in_json {
                TokenKind::Json
            } else {
                TokenKind::Normal
            };
            emit(kind, &std::mem::take(&mut self.buf));
        }
    }
}

/// Buffers the start of a response to decide whether it's a `questions` envelope
/// (suppressed) or real output; real output flows through a [`FenceSplitter`]
/// that tags the JSON artifact block so the `j` toggle can hide it.
#[derive(Default)]
struct StreamFilter {
    buf: String,
    classified: bool,
    suppress: bool,
    splitter: FenceSplitter,
}

impl StreamFilter {
    fn feed(&mut self, token: &str, emit: &dyn Fn(TokenKind, &str)) {
        if self.classified {
            if !self.suppress {
                self.splitter.feed(token, emit);
            }
            return;
        }
        self.buf.push_str(token);
        if self.buf.trim_start().chars().count() >= 30 {
            self.suppress = is_questions(self.buf.trim_start());
            self.classified = true;
            if !self.suppress {
                // A bare `{...}` artifact (no fence) is entirely JSON.
                if self.buf.trim_start().starts_with('{') {
                    self.splitter.in_json = true;
                }
                let b = std::mem::take(&mut self.buf);
                self.splitter.feed(&b, emit);
            }
            self.buf.clear();
        }
    }

    /// Flush a short response that never reached the classification threshold.
    fn finish(&mut self, emit: &dyn Fn(TokenKind, &str)) {
        if !self.classified && !is_questions(self.buf.trim_start()) {
            if self.buf.trim_start().starts_with('{') {
                self.splitter.in_json = true;
            }
            let b = std::mem::take(&mut self.buf);
            self.splitter.feed(&b, emit);
        }
        self.splitter.finish(emit);
    }
}

/// Per-phase cap on tool-call/work rounds (default; overridable via
/// `phase.max_tool_rounds`).
const MAX_TOOL_ROUNDS: u32 = 40;
/// Default per-call completion-token cap. Generous so reasoning models (which
/// emit long hidden reasoning before the answer) aren't truncated; a phase can
/// raise it further via `phase.max_tokens`. It's a cap — short answers cost less.
const DEFAULT_MAX_TOKENS: u32 = 16384;
/// Abort after this many identical failing tool-call rounds in a row.
const MAX_REPEATED_TOOL_ERRORS: u32 = 3;
/// After a write-capable phase has started writing code, abort if this many tool
/// rounds pass with no successful `write_file` — catches *varied* flailing (each
/// command different, so the identical-signature guard never trips) where the
/// agent thrashes on builds/searches without making code progress.
const MAX_ROUNDS_WITHOUT_PROGRESS: u32 = 30;
/// Most-recent messages budgeting never trims (keeps the live turn coherent).
const BUDGET_RECENT_KEEP: usize = 4;

/// Hooks into the agent loop. All have no-op defaults for headless runs.
#[async_trait]
pub trait AgentCallbacks: Send + Sync {
    async fn on_questions(&self, _questions: &[String]) -> String {
        "(proceeding without further clarification)".to_string()
    }
    fn on_status(&self, _msg: &str) {}
    fn on_token(&self, _token: &str) {}
    /// A token belonging to a structured JSON artifact envelope.
    fn on_json(&self, _token: &str) {}
    /// Input tokens of the latest request (current context-window usage).
    fn on_context(&self, _input_tokens: u64) {}
    fn on_thinking(&self, _text: &str) {}
    /// The assembled system prompt for this phase (emitted once, at phase start).
    fn on_system(&self, _prompt: &str) {}
    fn on_tool_call(&self, _name: &str, _summary: &str) {}
    fn on_tool_result(&self, _name: &str, _summary: &str, _is_error: bool) {}
    /// Return true to abort the tool loop mid-phase (user cancelled the run).
    fn cancelled(&self) -> bool {
        false
    }
}

/// A no-op callbacks impl for fully headless runs.
pub struct NoopCallbacks;
#[async_trait]
impl AgentCallbacks for NoopCallbacks {}

/// Result of an agent phase: the artifact plus the USD cost it incurred (so the
/// runner can accumulate a running total for the cost cap).
#[derive(Debug, Clone)]
pub struct AgentResult {
    pub artifact: Artifact,
    pub cost_usd: f64,
}

fn error_artifact(phase: &str, idea: &str, msg: impl Into<String>) -> Artifact {
    Artifact {
        phase: phase.to_string(),
        idea: idea.to_string(),
        content: json!({ "error": msg.into() }),
        ..Artifact::default()
    }
}

// ---------------------------------------------------------------------------
// Tool definitions + dispatch
// ---------------------------------------------------------------------------

/// Render the registered tool schemas as a markdown bullet list for the prompt's
/// "Tools available" section — one line per tool: `` - `name(a, b?)` — description ``
/// (required params plain, optional suffixed `?`). Generated from `build_tool_defs`
/// so the prompt always matches exactly what the model can call.
fn tools_list_markdown(defs: &[ToolDef]) -> String {
    let mut s = String::new();
    for d in defs {
        let required: Vec<&str> = d
            .parameters
            .get("required")
            .and_then(Value::as_array)
            .map(|a| a.iter().filter_map(Value::as_str).collect())
            .unwrap_or_default();
        let sig = match d.parameters.get("properties").and_then(Value::as_object) {
            Some(props) => {
                let parts: Vec<String> = props
                    .keys()
                    .map(|k| {
                        if required.contains(&k.as_str()) {
                            k.clone()
                        } else {
                            format!("{k}?")
                        }
                    })
                    .collect();
                format!("{}({})", d.name, parts.join(", "))
            }
            None => format!("{}()", d.name),
        };
        s.push_str(&format!("- `{sig}` — {}\n", d.description));
    }
    s.trim_end().to_string()
}

pub fn build_tool_defs(tools: &ToolsConfig, ctx: &ContextConfig) -> Vec<ToolDef> {
    let mut defs = Vec::new();
    if tools.files {
        // With range reads on, read_file accepts an optional 1-indexed line
        // window so a model can pull just the relevant slice of a large file.
        let read_file_params = if ctx.range_reads {
            json!({"type": "object", "properties": {
                "path": {"type": "string"},
                "start_line": {"type": "integer", "description": "Optional 1-indexed first line to read."},
                "end_line": {"type": "integer", "description": "Optional 1-indexed last line (inclusive). Requires start_line."}
            }, "required": ["path"]})
        } else {
            json!({"type": "object", "properties": {"path": {"type": "string"}}, "required": ["path"]})
        };
        let read_file_desc = if ctx.range_reads {
            "Read a file from the project repository. Pass start_line/end_line to read only a line range instead of the whole file."
        } else {
            "Read a file from the project repository."
        };
        defs.push(ToolDef {
            name: "read_file".into(),
            description: read_file_desc.into(),
            parameters: read_file_params,
        });
        defs.push(ToolDef {
            name: "list_files".into(),
            description: "List files/directories at a path in the repository. Omit `path` (or pass \"\" / \".\") to list the repository root.".into(),
            parameters: json!({"type": "object", "properties": {"path": {"type": "string", "description": "Repo-relative directory; empty or omitted = root."}}}),
        });
        // Repo-scoped regex search — find code by pattern without reading whole
        // files. Available whenever `files` is on (a read, so it survives
        // read-only phases); gated on the search_tool context flag.
        if ctx.search_tool {
            defs.push(ToolDef {
                name: "search_code".into(),
                description: "Search the repository for a regex pattern, returning `path:line: match` results. Use this to locate code without reading whole files.".into(),
                parameters: json!({"type": "object", "properties": {
                    "pattern": {"type": "string", "description": "Regular expression to search for."},
                    "scope": {"type": "string", "description": "Optional repo-relative directory to limit the search; empty = whole repo."},
                    "cap": {"type": "integer", "description": "Optional max matches to return (default 100)."}
                }, "required": ["pattern"]}),
            });
        }
        // Write tools are withheld in read-only phases (plan / spec / design).
        if !tools.read_only {
            defs.push(ToolDef {
                name: "write_file".into(),
                description: "Write (create or overwrite) a file in the project repository.".into(),
                parameters: json!({"type": "object", "properties": {"path": {"type": "string"}, "content": {"type": "string"}}, "required": ["path", "content"]}),
            });
            defs.push(ToolDef {
                name: "make_dir".into(),
                description: "Create a directory (and any parents) in the repository.".into(),
                parameters: json!({"type": "object", "properties": {"path": {"type": "string"}}, "required": ["path"]}),
            });
        }
    }
    if tools.bash.is_some() {
        defs.push(ToolDef {
            name: "bash".into(),
            description: "Run a shell command. It always executes from the repository \
root (your working directory) — use repo-relative paths and do NOT `cd` elsewhere \
(e.g. never `cd /tmp`); each call starts fresh at the repo root.".into(),
            parameters: json!({"type": "object", "properties": {"command": {"type": "string"}}, "required": ["command"]}),
        });
    }
    if tools.http_get {
        defs.push(ToolDef {
            name: "http_get".into(),
            description: "Fetch a URL over HTTP(S) (GET only, public hosts only).".into(),
            parameters: json!({"type": "object", "properties": {"url": {"type": "string"}}, "required": ["url"]}),
        });
    }
    if tools.web_search {
        defs.push(ToolDef {
            name: "web_search".into(),
            description: "Search the web and return titles, URLs, and snippets.".into(),
            parameters: json!({"type": "object", "properties": {"query": {"type": "string"}}, "required": ["query"]}),
        });
    }
    defs
}

fn str_arg<'a>(args: &'a Value, key: &str) -> Option<&'a str> {
    args.get(key)
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
}

/// Dispatch a tool call, returning the model-facing result string. Errors are
/// returned as `"Error: ..."` strings (helpful, not cryptic) rather than raised.
async fn dispatch_tool(
    name: &str,
    args: &Value,
    tools: &ToolsConfig,
    ctx: &ContextConfig,
    repo_root: &Path,
    flows_dir: Option<&Path>,
) -> String {
    let err = |e: flows_tools::ToolError| format!("Error: {}", e.0);
    // Defense in depth: even if the model calls a write tool it wasn't given,
    // refuse it in a read-only phase.
    if tools.read_only && matches!(name, "write_file" | "make_dir") {
        return format!(
            "Error: {name} is disabled in this phase (read-only — this phase plans/reviews, it does not modify files)"
        );
    }
    match name {
        "read_file" => match str_arg(args, "path") {
            None => "Error: read_file requires a non-empty 'path' string argument".into(),
            Some(p) => {
                // A line window is honored only when range reads are enabled and
                // the model actually passed one; otherwise read the whole file.
                let start = args.get("start_line").and_then(Value::as_u64);
                let end = args.get("end_line").and_then(Value::as_u64);
                if ctx.range_reads && (start.is_some() || end.is_some()) {
                    let start = start.unwrap_or(1) as usize;
                    // Default to a bounded window when only start_line is given.
                    let end = end.map(|e| e as usize).unwrap_or(start + 199);
                    files::read_file_range(repo_root, p, start, end).unwrap_or_else(err)
                } else {
                    files::read_file(repo_root, p).unwrap_or_else(err)
                }
            }
        },
        "search_code" => {
            if !ctx.search_tool {
                return "Error: search_code tool not enabled for this phase".into();
            }
            match str_arg(args, "pattern") {
                None => "Error: search_code requires a non-empty 'pattern' string argument".into(),
                Some(pattern) => {
                    let scope = args.get("scope").and_then(Value::as_str).unwrap_or("");
                    let cap = args.get("cap").and_then(Value::as_u64).unwrap_or(0) as usize;
                    code_search::search_code(repo_root, pattern, scope, cap).unwrap_or_else(err)
                }
            }
        }
        "write_file" => {
            let Some(p) = str_arg(args, "path") else {
                return "Error: write_file requires a non-empty 'path' string argument".into();
            };
            let Some(c) = args.get("content").and_then(Value::as_str) else {
                return "Error: write_file requires a 'content' string argument".into();
            };
            files::write_file(repo_root, p, c).unwrap_or_else(err)
        }
        "make_dir" => match str_arg(args, "path") {
            None => "Error: make_dir requires a non-empty 'path' string argument".into(),
            Some(p) => files::make_dir(repo_root, p).unwrap_or_else(err),
        },
        "list_files" => {
            let p = args.get("path").and_then(Value::as_str).unwrap_or(".");
            files::list_files(repo_root, p).unwrap_or_else(err)
        }
        "bash" => {
            let Some(cfg) = &tools.bash else {
                return "Error: bash tool not enabled for this phase".into();
            };
            match str_arg(args, "command") {
                None => "Error: bash requires a non-empty 'command' string argument".into(),
                Some(cmd) => bash::run_bash(cmd, repo_root, cfg).unwrap_or_else(err),
            }
        }
        "http_get" => {
            if !tools.http_get {
                return "Error: http_get tool not enabled for this phase".into();
            }
            match str_arg(args, "url") {
                None => "Error: http_get requires a non-empty 'url' string argument".into(),
                Some(url) => match flows_tools::http::http_get(url).await {
                    Ok(v) => serde_json::to_string(&v).unwrap_or_else(|_| v.to_string()),
                    Err(e) => format!("Error: {}", e.0),
                },
            }
        }
        "web_search" => {
            if !tools.web_search {
                return "Error: web_search tool not enabled for this phase".into();
            }
            match str_arg(args, "query") {
                None => "Error: web_search requires a non-empty 'query' string argument".into(),
                Some(q) => {
                    let cfg = flows_core::models::WebSearchConfig::default();
                    match flows_tools::search::web_search(q, &cfg).await {
                        Ok(v) => serde_json::to_string(&v).unwrap_or_else(|_| v.to_string()),
                        Err(e) => format!("Error: {}", e.0),
                    }
                }
            }
        }
        other => {
            // plugin fallback
            if !tools.plugins.is_empty() {
                if let Some(defn) = plugin::load_plugin_tool(other, flows_dir) {
                    return plugin::run_plugin_tool(&defn, args, repo_root).unwrap_or_else(err);
                }
            }
            format!("Error: unknown tool {other:?}")
        }
    }
}

fn summarize_tool_call(name: &str, args: &Value) -> String {
    match name {
        "read_file" => format!(
            "read_file {:?}",
            args.get("path").and_then(Value::as_str).unwrap_or("")
        ),
        "write_file" => {
            let n = args
                .get("content")
                .and_then(Value::as_str)
                .map(str::len)
                .unwrap_or(0);
            format!(
                "write_file {:?} ({n} chars)",
                args.get("path").and_then(Value::as_str).unwrap_or("")
            )
        }
        "list_files" => format!(
            "list_files {:?}",
            args.get("path").and_then(Value::as_str).unwrap_or(".")
        ),
        "search_code" => format!(
            "search_code {:?}{}",
            args.get("pattern").and_then(Value::as_str).unwrap_or(""),
            args.get("scope")
                .and_then(Value::as_str)
                .filter(|s| !s.is_empty())
                .map(|s| format!(" in {s:?}"))
                .unwrap_or_default()
        ),
        "bash" => {
            let c = args.get("command").and_then(Value::as_str).unwrap_or("");
            let short = if c.len() > 70 {
                format!("{}...", &c[..67])
            } else {
                c.to_string()
            };
            format!("bash: {short}")
        }
        _ => format!("{name}(...)"),
    }
}

fn summarize_tool_result(result: &str) -> (String, bool) {
    let is_error = result.starts_with("Error:");
    let first = result.trim().lines().next().unwrap_or("");
    let preview = if first.len() > 150 {
        format!("{}...", &first[..147])
    } else {
        first.to_string()
    };
    (preview, is_error)
}

// ---------------------------------------------------------------------------
// Response parsing
// ---------------------------------------------------------------------------

/// Belt-and-suspenders for models that emit tool calls as **text** instead of
/// structured `tool_calls` — some thinking models drift and write
/// `<tool_call>{…}</tool_call>` (or the declaration tag `<tools>{…}</tools>`)
/// inside their content/reasoning, which the server never converts. Extract those
/// blocks into real `ToolCall`s so the call still executes instead of stalling.
pub fn extract_text_tool_calls(text: &str) -> Vec<ToolCall> {
    static RE: OnceLock<Regex> = OnceLock::new();
    let re = RE.get_or_init(|| {
        Regex::new(r"(?s)<(?:tool_call|tools)>(.*?)</(?:tool_call|tools)>").unwrap()
    });
    let mut out = Vec::new();
    for cap in re.captures_iter(text) {
        let inner = cap[1].trim();
        let Ok(v) = serde_json::from_str::<Value>(inner) else {
            continue;
        };
        let Some(name) = v.get("name").and_then(Value::as_str) else {
            continue;
        };
        let args = v.get("arguments").cloned().unwrap_or_else(|| json!({}));
        let arguments = match &args {
            Value::String(s) => s.clone(),
            other => serde_json::to_string(other).unwrap_or_else(|_| "{}".into()),
        };
        out.push(ToolCall {
            id: format!("text-{}", out.len()),
            name: name.to_string(),
            arguments,
        });
    }
    out
}

fn fenced_json(content: &str) -> Vec<String> {
    static RE: OnceLock<Regex> = OnceLock::new();
    let re = RE.get_or_init(|| Regex::new(r"(?s)```(?:json)?\s*(\{.*?\})\s*```").unwrap());
    re.captures_iter(content)
        .map(|c| c[1].to_string())
        .collect()
}

/// Parse a text response into a structured `{type: artifact|questions, ...}`
/// object, or None if it isn't one.
pub fn parse_response(content: &str) -> Option<Value> {
    let mut candidates = fenced_json(content);
    if candidates.is_empty() {
        if let (Some(s), Some(e)) = (content.find('{'), content.rfind('}')) {
            if e >= s {
                candidates.push(content[s..=e].to_string());
            }
        }
    }
    for c in candidates {
        if let Ok(v) = serde_json::from_str::<Value>(&c) {
            if matches!(
                v.get("type").and_then(Value::as_str),
                Some("artifact") | Some("questions")
            ) {
                return Some(v);
            }
        }
    }
    None
}

// ---------------------------------------------------------------------------
// Main loop
// ---------------------------------------------------------------------------

/// Append one round of the human-readable prompt transcript to
/// `<dir>/transcript.txt`. The system prompt is written once (round 0); each round
/// logs the newly-appended input messages and the model's response, so the file
/// shows exactly the prompt the model saw. Best-effort; errors ignored.
fn append_transcript_round(
    dir: &Path,
    phase: &str,
    round: u32,
    system: &str,
    new_messages: &[Message],
    completion: &Completion,
) {
    use std::io::Write;
    let mut s = String::new();
    if round == 0 {
        s.push_str(&format!("\n========== PHASE: {phase} ==========\n"));
        // Surface the task at the very top for review. It's delivered as the first
        // user message (logged below), which otherwise sits *after* the long system
        // prompt — so a reviewer scrolling from the top sees the repo tree before
        // ever seeing what the task is.
        if let Some(task) = new_messages
            .iter()
            .find(|m| m.role == Role::User)
            .and_then(|m| m.content.as_deref())
        {
            s.push_str(&format!("TASK: {}\n", task.trim()));
        }
    }
    s.push_str(&format!(
        "\n----- ROUND {round} REQUEST ({} in tok) -----\n",
        completion.usage.input_tokens
    ));
    if round == 0 {
        // The system prompt is part of the request — sent on every round (the
        // `in tok` count includes it each time). Shown once here; it persists.
        s.push_str(&format!("[system]\n{system}\n\n"));
    }
    for m in new_messages {
        // Assistant messages (the model's own content + tool calls) are the
        // RESPONSE of the previous round, already logged in that round's RESPONSE
        // block — skip them here so each tool call is logged exactly once.
        if m.role == Role::Assistant {
            continue;
        }
        let role = match m.role {
            Role::System => "system",
            Role::User => "user",
            Role::Assistant => "assistant",
            Role::Tool => "tool",
        };
        if let Some(c) = &m.content {
            if !c.trim().is_empty() {
                s.push_str(&format!("[{role}] {c}\n"));
            }
        }
        for tc in &m.tool_calls {
            s.push_str(&format!("[{role} → call] {}({})\n", tc.name, tc.arguments));
        }
    }
    s.push_str(&format!(
        "----- ROUND {round} RESPONSE ({} out tok) -----\n{}\n",
        completion.usage.output_tokens, completion.content
    ));
    for tc in &completion.tool_calls {
        s.push_str(&format!(
            "[response → call] {}({})\n",
            tc.name, tc.arguments
        ));
    }
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(dir.join("transcript.txt"))
    {
        let _ = f.write_all(s.as_bytes());
    }
}

/// Assemble the "pushed" context block injected into the prompt: an up-front code
/// payload for models that don't (or can't) pull via tools. Sources:
/// - **oracle** (eval stopgap): the eval sets `FLOWS_ORACLE_CONTEXT` from a
///   fixture's labeled `relevant` files; injected verbatim under a clear header.
/// - **retrieval** (Tier 3): embed the repo + idea at the run's LLM endpoint and
///   inject the top-`retrieval_max_chunks` most similar chunks.
///
/// Returns "" when neither is enabled/available. Retrieval failures degrade to an
/// empty retrieval section (logged to stderr) rather than aborting the phase.
async fn build_pushed_context(context: &ContextConfig, repo_root: &Path, idea: &str) -> String {
    let mut out = String::new();

    if context.oracle {
        if let Ok(blob) = std::env::var("FLOWS_ORACLE_CONTEXT") {
            if !blob.trim().is_empty() {
                out.push_str(
                    "# Reference code (provided up front)\n\n\
                     The following files are the code relevant to this task, given \
                     directly so you can answer without reading them:\n\n",
                );
                out.push_str(blob.trim());
                out.push_str("\n\n");
            }
        }
    }

    if context.retrieval {
        // Local ONNX embedding — blocking CPU work (and a first-run model
        // download), so run it off the async runtime.
        let (root, idea_s, model, max_chunks, threshold) = (
            repo_root.to_path_buf(),
            idea.to_string(),
            context.retrieval_model.clone(),
            context.retrieval_max_chunks,
            context.retrieval_reindex_threshold,
        );
        let result = tokio::task::spawn_blocking(move || {
            crate::retrieval::retrieve(&root, &idea_s, &model, max_chunks, threshold)
        })
        .await
        .unwrap_or_else(|e| Err(format!("retrieval task panicked: {e}")));
        match result {
            Ok(chunks) if !chunks.trim().is_empty() => {
                out.push_str(
                    "# Retrieved code (most relevant to the task)\n\n\
                     These repository excerpts were selected as most relevant — \
                     use them to ground your answer:\n\n",
                );
                out.push_str(chunks.trim());
                out.push_str("\n\n");
            }
            Ok(_) => {}
            Err(e) => eprintln!("  retrieval skipped: {e}"),
        }
    }

    out.trim().to_string()
}

#[allow(clippy::too_many_arguments)]
pub async fn run_agent_phase(
    phase: &PhaseConfig,
    idea: &str,
    repo_root: &Path,
    prior_artifacts: &Map<String, Value>,
    template_name: &str,
    llm: &LLMConfig,
    provider: &dyn Provider,
    callbacks: &dyn AgentCallbacks,
    flows_dir: Option<&Path>,
    context: &ContextConfig,
    max_cost_usd: f64,
    prior_cost_usd: f64,
    logger: Option<&flows_core::LlmLogger>,
) -> AgentResult {
    if let Some(log) = logger {
        log.phase_start(&phase.name);
    }
    let prompt_template = crate::prompts::load_prompt(phase, template_name, repo_root, flows_dir);

    // context for prompt rendering
    // Models that can't function-call (e.g. many reasoning models) get NO tool
    // schemas and no tool guidance — the prompt templates gate their tool
    // sections on `tools_enabled` so a no-tool model is never told to call tools.
    let tools_enabled = llm.supports_tools != Some(false);
    // Build the tool schemas first so the prompt's tools section is generated from
    // the REAL registered toolset (single source of truth) — no hand-maintained
    // list that drifts (e.g. `research_ro`'s http_get/web_search going unlisted).
    let tool_defs = if tools_enabled {
        build_tool_defs(&phase.tools, context)
    } else {
        Vec::new()
    };

    let mut ctx = Map::new();
    ctx.insert("idea".into(), json!(idea));
    ctx.insert("tools_enabled".into(), json!(tools_enabled));
    ctx.insert("tools_list".into(), json!(tools_list_markdown(&tool_defs)));
    // Slim tree (depth-bounded, tightly capped, points at tools) for small
    // models when enabled; otherwise the full baseline tree.
    let repo_tree = if context.slim_tree {
        crate::repo_context::build_slim_tree(repo_root, 200, 2)
    } else {
        crate::repo_context::build_repo_tree(repo_root, 500)
    };
    ctx.insert("repo_tree".into(), json!(repo_tree));
    // Repo map (2.3): symbol signatures ranked by idea overlap, injected alongside
    // the tree (tree = layout, map = symbols). Push context — helps no-tool models.
    let repo_map = if context.repo_map {
        crate::repo_map::build_repo_map(repo_root, idea, context.repo_map_max_tokens)
    } else {
        String::new()
    };
    ctx.insert("repo_map".into(), json!(repo_map));
    ctx.insert("phase_name".into(), json!(phase.name));
    // Pushed context: code delivered up front (not pulled via tools), so a model
    // that can't call tools still gets grounding. Two sources feed one variable:
    // the eval oracle (labeled reference files) and semantic retrieval (top-k
    // embedded chunks). Concatenated when both are on.
    let pushed_context = build_pushed_context(context, repo_root, idea).await;
    ctx.insert("pushed_context".into(), json!(pushed_context));
    // Per-phase scoping (2.6): expose prior artifacts exactly **once**, as the
    // `prior_artifacts` map. Templates self-select what they need (`.get('plan')`,
    // `['spec']`, `.items()`), so no phase is force-fed the whole history and the
    // old redundant per-key top-level injection is gone.
    ctx.insert(
        "prior_artifacts".into(),
        Value::Object(prior_artifacts.clone()),
    );
    let mut system_prompt = crate::prompts::render_prompt(&prompt_template, &Value::Object(ctx))
        .unwrap_or(prompt_template);

    // Append project/global AGENTS.md instructions so the agent honours repo
    // conventions (e.g. "use `make test` for all tooling").
    if let Some(docs) = crate::prompts::load_agent_docs(repo_root, flows_dir) {
        system_prompt.push_str("\n\n# Project instructions (AGENTS.md)\n\n");
        system_prompt.push_str(&docs);
    }

    // No-tool models get no schemas; instead the agent tells them to inline any
    // file writes in the artifact's `content.files` map (written to disk after).
    if !tools_enabled {
        system_prompt.push_str(
            "\n\n# No tools available\n\n\
You cannot call tools. Do not attempt tool/function calls — respond only with the \
JSON artifact. To create or modify files, include them in the artifact under \
`content.files` as a map of repository-relative path → the file's full new \
contents. Example:\n\
```json\n{\"type\": \"artifact\", \"content\": {\"files\": {\"src/x.rs\": \"...full contents...\"}, \"summary\": \"...\"}}\n```",
        );
    } else if context.search_tool || context.range_reads {
        // The tools themselves are listed in the prompt's tools section (generated
        // from the schemas); this is just usage *strategy* for the context tools.
        system_prompt.push_str(
            "\n\n# Finding code efficiently\n\n\
Prefer `search_code` + range reads (`read_file` with start_line/end_line) over \
reading whole files — locate the relevant code, then read just that slice, to keep \
your context small.",
        );
    }
    // `llm` is already the phase's resolved config (profile/literal resolved by
    // the runner), so its model is the one to send — no per-phase re-derivation.
    let model = llm.model.clone();
    let (thinking_enabled, thinking_budget) = resolve_thinking(phase, llm);
    let thinking_budget =
        (thinking_enabled && model_supports_thinking(&model)).then_some(thinking_budget);

    // Surface the assembled system prompt once (the UI shows it under a toggle).
    callbacks.on_system(&system_prompt);

    let mut messages: Vec<Message> = vec![Message::user(format!("Idea: {idea}"))];
    let max_tool_rounds = phase.max_tool_rounds.unwrap_or(MAX_TOOL_ROUNDS);
    let total_rounds = phase.max_clarification_rounds + max_tool_rounds;

    let mut phase_cost = 0.0;
    let mut last_sig: Option<Vec<(String, String)>> = None;
    let mut repeated_errors = 0u32;
    // No-progress guard (write-capable phases only): once the agent has written
    // code, count rounds with no successful `write_file`; abort if it thrashes.
    let can_write = phase.tools.files && !phase.tools.read_only;
    let mut has_written = false;
    let mut rounds_without_write = 0u32;

    // Optional full-prompt transcript (what the model actually saw each round),
    // gated by `FLOWS_TRANSCRIPT` so normal runs aren't bloated. The eval harness
    // sets it to capture the assembled context for review. `transcript_logged`
    // tracks how many messages are already written, so each round logs only the
    // newly-appended input (system + tools are written once at round 0).
    let transcript_dir: Option<std::path::PathBuf> = if std::env::var("FLOWS_TRANSCRIPT").is_ok() {
        logger.and_then(|l| l.dir()).map(|d| d.to_path_buf())
    } else {
        None
    };
    let mut transcript_logged = 0usize;

    for round_num in 0..total_rounds {
        // Mid-phase cancellation: bail out of the tool loop promptly rather than
        // waiting for the whole phase to finish.
        if callbacks.cancelled() {
            return AgentResult {
                artifact: error_artifact(&phase.name, idea, "cancelled"),
                cost_usd: phase_cost,
            };
        }

        let status = if round_num > 0 {
            format!("thinking (round {})...", round_num + 1)
        } else {
            "thinking...".to_string()
        };
        callbacks.on_status(&status);

        // Context-window budgeting (2.4): before sending, trim old tool results if
        // the request approaches the window. Preserves system prompt, task, and the
        // most recent turns. No-op unless the `budgeting` flag is on.
        if context.budgeting {
            let window = flows_core::pricing::context_window(&model)
                .unwrap_or(context.unknown_window_tokens as u64);
            crate::budget::trim_to_budget(
                &mut messages,
                &system_prompt,
                window,
                context.budget_fraction,
                BUDGET_RECENT_KEEP,
            );
        }

        let req = CompletionRequest {
            model: model.clone(),
            system: system_prompt.clone(),
            messages: messages.clone(),
            tools: tool_defs.clone(),
            max_tokens: phase.max_tokens.unwrap_or(DEFAULT_MAX_TOKENS),
            thinking_budget,
        };
        // Stream content tokens live, but suppress questions-envelope JSON via a
        // buffering filter. Reasoning is emitted once after the round (below), so
        // the thinking sink here is a no-op.
        let filter = std::sync::Mutex::new(StreamFilter::default());
        let emit = |kind: TokenKind, t: &str| match kind {
            TokenKind::Normal => callbacks.on_token(t),
            TokenKind::Json => callbacks.on_json(t),
        };
        let on_token = |t: &str| filter.lock().unwrap().feed(t, &emit);
        let no_thinking = |_: &str| {};
        let t0 = std::time::Instant::now();
        let completion = match provider
            .complete_streaming(&req, &on_token, &no_thinking)
            .await
        {
            Ok(c) => c,
            Err(e) => {
                return AgentResult {
                    artifact: error_artifact(&phase.name, idea, format!("llm error: {e}")),
                    cost_usd: phase_cost,
                }
            }
        };
        filter.lock().unwrap().finish(&emit);
        let latency_ms = t0.elapsed().as_millis() as i64;
        if let Some(log) = logger {
            log.response(
                &phase.name,
                round_num,
                &model,
                completion.usage.input_tokens,
                completion.usage.output_tokens,
                latency_ms,
            );
        }
        if let Some(dir) = &transcript_dir {
            append_transcript_round(
                dir,
                &phase.name,
                round_num,
                &system_prompt,
                &messages[transcript_logged..],
                &completion,
            );
            transcript_logged = messages.len();
        }

        // Surface the request's input tokens as the live context-window size.
        if completion.usage.input_tokens > 0 {
            callbacks.on_context(completion.usage.input_tokens);
        }

        if let Some(reasoning) = &completion.reasoning {
            callbacks.on_thinking(reasoning);
        }
        phase_cost += calculate_cost(
            &model,
            completion.usage.input_tokens,
            completion.usage.output_tokens,
        );

        // Mid-phase cost ceiling.
        if max_cost_usd > 0.0 && prior_cost_usd + phase_cost >= max_cost_usd {
            return AgentResult {
                artifact: error_artifact(
                    &phase.name,
                    idea,
                    format!(
                        "Cost limit ${max_cost_usd:.2} reached (${:.4} spent so far)",
                        prior_cost_usd + phase_cost
                    ),
                ),
                cost_usd: phase_cost,
            };
        }

        // Tool calls — prefer structured; fall back to any the model leaked as
        // text (some thinking models write `<tool_call>{…}</tool_call>` or
        // `<tools>{…}</tools>` in content/reasoning instead of real tool_calls).
        let tool_calls: Vec<ToolCall> = if !completion.tool_calls.is_empty() {
            completion.tool_calls.clone()
        } else {
            let mut ex = extract_text_tool_calls(&completion.content);
            if let Some(r) = &completion.reasoning {
                ex.extend(extract_text_tool_calls(r));
            }
            ex
        };

        if !tool_calls.is_empty() {
            messages.push(Message::assistant_tools(
                (!completion.content.is_empty()).then(|| completion.content.clone()),
                tool_calls.clone(),
            ));

            let mut all_errored = true;
            let mut wrote_this_round = false;
            for tc in &tool_calls {
                let args: Value = serde_json::from_str(&tc.arguments).unwrap_or(json!({}));
                callbacks.on_tool_call(&tc.name, &summarize_tool_call(&tc.name, &args));
                let result =
                    dispatch_tool(&tc.name, &args, &phase.tools, context, repo_root, flows_dir)
                        .await;
                let (summary, is_error) = summarize_tool_result(&result);
                callbacks.on_tool_result(&tc.name, &summary, is_error);
                if !is_error {
                    all_errored = false;
                    if tc.name == "write_file" {
                        wrote_this_round = true;
                    }
                }
                messages.push(Message::tool_result(tc.id.clone(), result));
            }

            // No-progress guard: once code has been written, thrashing many rounds
            // without another successful write means the agent is stuck.
            if wrote_this_round {
                has_written = true;
                rounds_without_write = 0;
            } else if has_written {
                rounds_without_write += 1;
            }
            if can_write && has_written && rounds_without_write >= MAX_ROUNDS_WITHOUT_PROGRESS {
                return AgentResult {
                    artifact: error_artifact(
                        &phase.name,
                        idea,
                        format!(
                            "aborted: no code written in {MAX_ROUNDS_WITHOUT_PROGRESS} tool rounds \
after starting — likely stuck in a build/search loop"
                        ),
                    ),
                    cost_usd: phase_cost,
                };
            }

            // Degenerate-loop guard.
            let sig: Vec<(String, String)> = tool_calls
                .iter()
                .map(|tc| (tc.name.clone(), tc.arguments.clone()))
                .collect();
            repeated_errors = if all_errored && last_sig.as_ref() == Some(&sig) {
                repeated_errors + 1
            } else if all_errored {
                1
            } else {
                0
            };
            last_sig = Some(sig);
            if repeated_errors >= MAX_REPEATED_TOOL_ERRORS {
                let names: BTreeMap<&str, ()> =
                    tool_calls.iter().map(|tc| (tc.name.as_str(), ())).collect();
                let names: Vec<&str> = names.into_keys().collect();
                return AgentResult {
                    artifact: error_artifact(
                        &phase.name,
                        idea,
                        format!(
                            "aborted: tool call(s) [{}] failed identically {repeated_errors} rounds in a row without progress",
                            names.join(", ")
                        ),
                    ),
                    cost_usd: phase_cost,
                };
            }
            continue;
        }

        // Text response.
        messages.push(Message::assistant_text(completion.content.clone()));
        let parsed = parse_response(&completion.content);

        match parsed
            .as_ref()
            .and_then(|p| p.get("type").and_then(Value::as_str))
        {
            Some("questions") => {
                let questions: Vec<String> = parsed
                    .as_ref()
                    .and_then(|p| p.get("questions"))
                    .and_then(Value::as_array)
                    .map(|a| {
                        a.iter()
                            .filter_map(|q| q.as_str().map(str::to_string))
                            .collect()
                    })
                    .unwrap_or_default();
                if round_num >= phase.max_clarification_rounds {
                    messages.push(Message::user("(proceeding without further clarification)"));
                } else {
                    let answer = callbacks.on_questions(&questions).await;
                    messages.push(Message::user(answer));
                }
                continue;
            }
            Some("artifact") => {
                // Extract the artifact body. Compliant: `{type, content: {...}}` →
                // use `content`. But models often flatten, emitting the answer as a
                // bare `content` string with sibling fields (`references`, …) — keep
                // those instead of silently dropping them: fold everything (minus
                // `type`) into the content, renaming a bare `content` value to
                // `answer` so it reads as a field.
                let content = parsed
                    .and_then(|mut p| p.as_object_mut().map(std::mem::take))
                    .map(|mut o| {
                        o.remove("type");
                        if o.len() == 1 && o.contains_key("content") {
                            o.remove("content").unwrap()
                        } else {
                            if let Some(c) = o.remove("content") {
                                o.insert("answer".into(), c);
                            }
                            Value::Object(o)
                        }
                    })
                    .unwrap_or(json!({}));
                // No-tools models can't call write_file, so they inline files in
                // `content.files` (path → contents); write them here.
                if !tools_enabled {
                    if let Some(files) = content.get("files").and_then(Value::as_object) {
                        for (path, body) in files {
                            if let Some(text) = body.as_str() {
                                match files::write_file(repo_root, path, text) {
                                    Ok(_) => callbacks.on_tool_result(
                                        "write_file",
                                        &format!("wrote {path}"),
                                        false,
                                    ),
                                    Err(e) => callbacks.on_tool_result(
                                        "write_file",
                                        &format!("{path}: {e}"),
                                        true,
                                    ),
                                }
                            }
                        }
                    }
                }
                return AgentResult {
                    artifact: Artifact {
                        phase: phase.name.clone(),
                        idea: idea.to_string(),
                        content,
                        ..Artifact::default()
                    },
                    cost_usd: phase_cost,
                };
            }
            _ => {
                // Unparsed — treat raw output as the artifact.
                return AgentResult {
                    artifact: Artifact {
                        phase: phase.name.clone(),
                        idea: idea.to_string(),
                        content: json!({ "raw_output": completion.content }),
                        ..Artifact::default()
                    },
                    cost_usd: phase_cost,
                };
            }
        }
    }

    AgentResult {
        artifact: error_artifact(&phase.name, idea, "max rounds exceeded"),
        cost_usd: phase_cost,
    }
}

#[cfg(test)]
mod tool_list_tests {
    use super::{build_tool_defs, extract_text_tool_calls, tools_list_markdown};
    use flows_core::models::{resolve_preset, ContextConfig};

    #[test]
    fn extracts_leaked_text_tool_calls() {
        // A thinking model leaked the call as text with the declaration tag.
        let text = "So let's list the crates.\n<tools>\n{\"name\": \"list_files\", \"arguments\": {\"path\": \"crates\"}}\n</tools>\n";
        let calls = extract_text_tool_calls(text);
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].name, "list_files");
        assert_eq!(calls[0].arguments, r#"{"path":"crates"}"#);
        // Also the proper call tag.
        let calls = extract_text_tool_calls(
            "<tool_call>{\"name\":\"read_file\",\"arguments\":{\"path\":\"a.rs\"}}</tool_call>",
        );
        assert_eq!(calls[0].name, "read_file");
        // No tool blocks → nothing.
        assert!(extract_text_tool_calls("just some prose, no tools").is_empty());
    }

    #[test]
    fn tools_list_surfaces_full_research_ro_toolset() {
        // research_ro registers read_file + list_files + http_get + web_search —
        // all must appear (the old static template listed only the first two).
        let tools = resolve_preset("research_ro").unwrap();
        let defs = build_tool_defs(&tools, &ContextConfig::default());
        let md = tools_list_markdown(&defs);
        for t in ["read_file", "list_files", "http_get", "web_search"] {
            assert!(md.contains(t), "tools_list missing `{t}`:\n{md}");
        }
        // Read-only phase → no write_file.
        assert!(!md.contains("write_file"));
        // search_code only when the flag is on.
        assert!(!md.contains("search_code"));
        let with_search = build_tool_defs(
            &tools,
            &ContextConfig {
                search_tool: true,
                ..ContextConfig::default()
            },
        );
        assert!(tools_list_markdown(&with_search).contains("search_code"));
    }
}

#[cfg(test)]
mod stream_filter_tests {
    use super::{StreamFilter, TokenKind};
    use std::sync::Mutex;

    /// Returns (all emitted text, text emitted as Json).
    fn collect2(chunks: &[&str]) -> (String, String) {
        let normal = Mutex::new(String::new());
        let json = Mutex::new(String::new());
        let emit = |kind: TokenKind, t: &str| match kind {
            TokenKind::Normal => normal.lock().unwrap().push_str(t),
            TokenKind::Json => json.lock().unwrap().push_str(t),
        };
        let mut f = StreamFilter::default();
        for c in chunks {
            f.feed(c, &emit);
        }
        f.finish(&emit);
        let n = normal.into_inner().unwrap();
        let j = json.into_inner().unwrap();
        (format!("{n}{j}"), j)
    }

    fn collect(chunks: &[&str]) -> String {
        collect2(chunks).0
    }

    #[test]
    fn prose_is_emitted() {
        let out = collect(&["Here is a long line of prose output that exceeds threshold."]);
        assert!(out.contains("prose output"));
    }

    #[test]
    fn questions_envelope_is_suppressed() {
        // a questions JSON response must not reach the token stream
        let out = collect(&[
            "```json\n",
            "{\"type\": \"questions\", \"questions\": [\"which color?\"]}",
            "\n```",
        ]);
        assert_eq!(out, "");
    }

    #[test]
    fn artifact_envelope_is_tagged_json() {
        // artifacts DO stream, but tagged Json so the `j` toggle can hide them
        let (all, json) = collect2(&[
            "```json\n{\"type\": \"artifact\", \"content\": {\"summary\": \"done here\"}}\n```",
        ]);
        assert!(all.contains("artifact"));
        assert!(json.contains("artifact"), "artifact should be Json-tagged");
    }

    #[test]
    fn prose_then_fenced_json_splits() {
        // model wraps the artifact in prose — prose stays Normal, the ```json
        // block is tagged Json
        let (_all, json) = collect2(&[
            "Here is the plan you asked for, in detail:\n",
            "```json\n{\"type\": \"artifact\", \"content\": {}}\n```",
        ]);
        assert!(json.contains("artifact"), "fenced json should be tagged");
        assert!(!json.contains("Here is the plan"), "prose must stay Normal");
    }

    #[test]
    fn fence_split_across_tokens_is_still_tagged() {
        // the ```json marker arrives split across two tokens
        let (_all, json) = collect2(&[
            "some prose leading up to the block here ``",
            "`json\n{\"x\": 1}\n```",
        ]);
        assert!(
            json.contains("\"x\""),
            "json tagged even when the fence is split"
        );
    }

    #[test]
    fn bare_json_object_is_tagged_json() {
        let (_all, json) = collect2(&["{\"summary\": \"a fairly long json artifact body here\"}"]);
        assert!(json.contains("summary"));
    }

    #[test]
    fn prose_is_not_tagged_json() {
        let (_all, json) =
            collect2(&["Here is a long line of prose output that exceeds threshold."]);
        assert_eq!(json, "");
    }

    #[test]
    fn short_prose_flushes_on_finish() {
        let out = collect(&["hi there"]); // under 30 chars, never classified
        assert_eq!(out, "hi there");
    }
}
