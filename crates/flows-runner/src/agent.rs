//! Agent phase — the LLM tool loop: build the request, stream the response,
//! dispatch tools, honour round/cost budgets, parse questions/artifacts.

use std::collections::{BTreeMap, HashMap, HashSet};
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
/// After this many consecutive *unproductive* `web_search` calls (empty results
/// or request failures), stop serving searches for the rest of the phase. The
/// DuckDuckGo backend gets rate-limited and returns empty/errors, which don't
/// count as tool *failures* — so a model hunting docs can otherwise spin forever.
const MAX_DRY_SEARCHES: u32 = 6;
/// Consecutive rounds whose only `write_file` was a *no-op* (content byte-for-byte
/// identical to what's already on disk) before we nudge the model to move on. A
/// stuck model rewriting an unchanged file writes "successfully" every round, so
/// the error/no-write guards never trip — this catches it.
const NUDGE_AFTER_NOOP_WRITES: u32 = 3;
/// If no-op writes keep coming after the nudge, abort the phase once the streak
/// reaches this — the model is wedged in a rewrite loop and won't recover.
const ABORT_AFTER_NOOP_WRITES: u32 = 6;
/// Consecutive empty final responses tolerated (with a nudge each) before the
/// phase fails — a model that keeps returning nothing is stuck.
const MAX_EMPTY_RESPONSES: u32 = 3;
/// Consecutive tool rounds that brought nothing new (deduplicated re-reads,
/// disabled or failed searches, failed fetches) before the model is told to stop
/// gathering. A thinking model can otherwise re-deliberate for many minutes per
/// round and spend each round on a call that cannot help (observed: a plan phase
/// ran 14 rounds / 107 min re-reading one file and searching a dead backend).
const STALL_NUDGE_ROUNDS: u32 = 3;
/// …after which a phase that cannot write gets no more tool calls and must reply.
const STALL_FORCE_ROUNDS: u32 = 5;
/// …and a phase that can write is aborted (a tool-less implement reply would only
/// inline its code).
const STALL_ABORT_ROUNDS: u32 = 8;
/// Weaker signal: consecutive rounds of file exploration (reads, listings,
/// searches) that turned up no file the phase had not already seen and no new
/// read. Observed: a plan phase over a 3-file repo ran 16 rounds of ever-different
/// searches that all pointed back at the one file it had read. Nudge, then force
/// a reply in phases that cannot write.
const EXPLORE_NUDGE_ROUNDS: u32 = 6;
const EXPLORE_FORCE_ROUNDS: u32 = 10;
/// Consecutive replies cut off at the output limit before the phase fails.
const MAX_CAPPED_RESPONSES: u32 = 2;
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
    /// Current tool-loop round and the phase's cap (for a `round N/T` readout).
    fn on_round(&self, _round: u32, _total: u32) {}
    fn on_thinking(&self, _text: &str) {}
    /// The assembled system prompt for this phase (emitted once, at phase start).
    fn on_system(&self, _prompt: &str) {}
    /// The first user turn for this phase (the question), emitted once at start.
    fn on_user(&self, _message: &str) {}
    /// A structural marker line for the output pane (always visible), e.g. the
    /// per-round LLM request/response boundaries.
    fn on_note(&self, _text: &str) {}
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

/// Cheap non-cryptographic hash of a string, for detecting an unchanged re-read.
fn hash_str(s: &str) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    s.hash(&mut h);
    h.finish()
}

fn error_artifact(phase: &str, idea: &str, msg: impl Into<String>) -> Artifact {
    Artifact {
        phase: phase.to_string(),
        idea: idea.to_string(),
        content: json!({ "error": msg.into() }),
        ..Artifact::default()
    }
}

/// Fold a parsed `{"type":"artifact", ...}` envelope into the artifact content.
/// Compliant `{type, content:{...}}` → the `content`. Flattened envelopes (a bare
/// `content` string with sibling fields) keep the siblings, renaming `content` to
/// `answer`. Shared by the normal text-emission path and the write_file recovery.
fn artifact_content(parsed: Option<Value>) -> Value {
    parsed
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
        .unwrap_or(json!({}))
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
            description: "List files/directories at a path in the repository. Omit `path` (or pass \"\" / \".\") to list the repository root. Directories end with `/`; all paths are repo-relative.".into(),
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
    if let Some(bash) = &tools.bash {
        let mut description = "Run a shell command. It always executes from the repository \
root (your working directory) — use repo-relative paths and do NOT `cd` elsewhere \
(e.g. never `cd /tmp`); each call starts fresh at the repo root."
            .to_string();
        if bash.offline {
            description.push_str(
                " Commands have NO network access (only localhost): don't fetch \
dependencies or call external services; use what is already installed or vendored.",
            );
        }
        defs.push(ToolDef {
            name: "bash".into(),
            description,
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
    if tools.lookup {
        let ids = flows_tools::lookup::source_ids();
        defs.push(ToolDef {
            name: "lookup".into(),
            description: format!(
                "Structured lookup against a specific source's JSON API — more \
                 reliable than web_search for library/API details. Sources:\n{}",
                flows_tools::lookup::source_catalog()
            ),
            parameters: json!({
                "type": "object",
                "properties": {
                    "source": {"type": "string", "enum": ids, "description": "which source to query"},
                    "query": {"type": "string"},
                    "max": {"type": "integer", "description": "max results (default 5)"}
                },
                "required": ["source", "query"]
            }),
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
    web_search_cfg: &flows_core::models::WebSearchConfig,
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
                Some(q) => match flows_tools::search::web_search(q, web_search_cfg).await {
                    Ok(v) => serde_json::to_string(&v).unwrap_or_else(|_| v.to_string()),
                    Err(e) => format!("Error: {}", e.0),
                },
            }
        }
        "lookup" => {
            if !tools.lookup {
                return "Error: lookup tool not enabled for this phase".into();
            }
            match (str_arg(args, "source"), str_arg(args, "query")) {
                (None, _) => "Error: lookup requires a 'source' argument".into(),
                (_, None) => "Error: lookup requires a non-empty 'query' argument".into(),
                (Some(source), Some(q)) => {
                    let max = args.get("max").and_then(Value::as_u64).unwrap_or(5) as usize;
                    match flows_tools::lookup::lookup(source, q, max).await {
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
            // Count chars (not bytes) to match the `Written: … (N chars)` result —
            // otherwise non-ASCII content (e.g. ♠♥♦♣) makes the two counts disagree.
            let n = args
                .get("content")
                .and_then(Value::as_str)
                .map(|s| s.chars().count())
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
        "lookup" => format!(
            "lookup {}: {:?}",
            args.get("source").and_then(Value::as_str).unwrap_or("?"),
            args.get("query").and_then(Value::as_str).unwrap_or("")
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
            // Collapse newlines/runs of whitespace so a multi-line command renders
            // as a single status line — embedded newlines corrupt the fixed TUI
            // bottom rows. Truncate by chars (not bytes) to avoid a UTF-8 panic.
            let one_line = c.split_whitespace().collect::<Vec<_>>().join(" ");
            let short = if one_line.chars().count() > 70 {
                let head: String = one_line.chars().take(67).collect();
                format!("{head}...")
            } else {
                one_line
            };
            format!("bash: {short}")
        }
        _ => format!("{name}(...)"),
    }
}

/// Repo paths a `list_files` / `search_code` result mentions.
fn touched_paths(tool: &str, result: &str) -> Vec<String> {
    if result.starts_with("Error") || result.starts_with("No matches") {
        return Vec::new();
    }
    result
        .lines()
        .filter_map(|l| match tool {
            "list_files" => Some(l.trim()).filter(|l| !l.is_empty() && *l != "(empty)"),
            _ => l.split_once(':').map(|(p, _)| p.trim()),
        })
        .filter(|p| !p.is_empty() && !p.contains(' '))
        .map(norm_path)
        .collect()
}

fn norm_path(p: &str) -> String {
    p.trim()
        .trim_start_matches("./")
        .trim_start_matches('/')
        .trim_end_matches('/')
        .to_string()
}

/// A tool error from failing to reach the backend at all (connect / send / read),
/// as opposed to an HTTP status or an empty result.
fn backend_unreachable(result: &str) -> bool {
    result.contains("request failed") || result.contains("read failed")
}

/// An `http_get` result whose response status is 400 or above.
fn http_error_status(result: &str) -> bool {
    serde_json::from_str::<Value>(result)
        .ok()
        .and_then(|v| v.get("status").and_then(Value::as_u64))
        .is_some_and(|s| s >= 400)
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
/// Turn a `{name, arguments}` object (optionally wrapped as `{"function": {...}}`)
/// into a `ToolCall`, appending it to `out`. Returns whether one was added.
fn push_tool_call(v: &Value, out: &mut Vec<ToolCall>) -> bool {
    // OpenAI-style `{"function": {"name":.., "arguments":..}}` nests the call.
    let obj = v.get("function").unwrap_or(v);
    let Some(name) = obj.get("name").and_then(Value::as_str) else {
        return false;
    };
    let args = obj.get("arguments").cloned().unwrap_or_else(|| json!({}));
    let arguments = match &args {
        // `arguments` is sometimes a JSON string, sometimes an inline object.
        Value::String(s) => s.clone(),
        other => serde_json::to_string(other).unwrap_or_else(|_| "{}".into()),
    };
    out.push(ToolCall {
        id: format!("text-{}", out.len()),
        name: name.to_string(),
        arguments,
    });
    true
}

pub fn extract_text_tool_calls(text: &str) -> Vec<ToolCall> {
    static RE: OnceLock<Regex> = OnceLock::new();
    let re = RE.get_or_init(|| {
        Regex::new(r"(?s)<(?:tool_call|tools)>(.*?)</(?:tool_call|tools)>").unwrap()
    });
    let mut out = Vec::new();

    // 1. XML-tagged blocks: `<tool_call>{…}</tool_call>` / `<tools>{…}</tools>`.
    for cap in re.captures_iter(text) {
        if let Ok(v) = serde_json::from_str::<Value>(cap[1].trim()) {
            push_tool_call(&v, &mut out);
        }
    }
    if !out.is_empty() {
        return out;
    }

    // 2. JSON-envelope tool calls: some thinking models emit the call as plain
    //    JSON in their content instead of structured `tool_calls` —
    //    `{"type":"tool_call","name":..,"arguments":..}`, an OpenAI `function`
    //    wrapper, or a `{"tool_calls":[…]}` array. Without this they get captured
    //    as an artifact and the phase ends prematurely (no exploration / no
    //    clarifying questions).
    let mut candidates = fenced_json(text);
    if candidates.is_empty() {
        if let (Some(s), Some(e)) = (text.find('{'), text.rfind('}')) {
            if e >= s {
                candidates.push(text[s..=e].to_string());
            }
        }
    }
    for c in candidates {
        let Ok(v) = serde_json::from_str::<Value>(&c) else {
            continue;
        };
        let ty = v.get("type").and_then(Value::as_str);
        // Artifact / questions envelopes are handled elsewhere — never treat them
        // as tool calls.
        if matches!(ty, Some("artifact") | Some("questions")) {
            continue;
        }
        if let Some(arr) = v.get("tool_calls").and_then(Value::as_array) {
            for item in arr {
                push_tool_call(item, &mut out);
            }
        } else {
            // A single call: an explicit `type: tool_call|function`, or any object
            // carrying both `name` and `arguments`.
            let looks_like_call = matches!(ty, Some("tool_call") | Some("function"))
                || (v.get("name").is_some() && v.get("arguments").is_some())
                || v.get("function").is_some();
            if looks_like_call {
                push_tool_call(&v, &mut out);
            }
        }
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
/// One-line warning when this phase's shell commands run with weaker resource
/// limits than configured (no per-command cgroup on this host). Commands still
/// run — filesystem confinement is the hard boundary; limits degrade.
fn limits_note(bash: Option<&flows_core::models::BashConfig>) -> Option<String> {
    use flows_tools::sandbox::{limits_mode, LimitMode, Limits};
    let limits = Limits::from_bash(bash?);
    if limits.is_unlimited() || limits_mode() == LimitMode::Cgroup {
        return None;
    }
    Some(format!("⚠ resource limits: {}", limits_mode().describe()))
}

/// Context window (tokens) assumed for `model`: the known window, else the
/// configured `unknown_window_tokens`. Shared by loop budgeting and retrieval.
fn window_tokens(model: &str, context: &ContextConfig) -> u64 {
    flows_core::pricing::context_window(model).unwrap_or(context.unknown_window_tokens as u64)
}

/// Token budget for retrieved code: `retrieval_budget_fraction` of the window.
fn retrieval_budget(model: &str, context: &ContextConfig) -> usize {
    (window_tokens(model, context) as f64 * context.retrieval_budget_fraction).max(0.0) as usize
}

async fn build_pushed_context(
    context: &ContextConfig,
    repo_root: &Path,
    idea: &str,
    llm_model: &str,
    callbacks: &dyn AgentCallbacks,
) -> String {
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
        let (root, idea_s, model, max_chunks) = (
            repo_root.to_path_buf(),
            idea.to_string(),
            context.retrieval_model.clone(),
            context.retrieval_max_chunks,
        );
        let budget = retrieval_budget(llm_model, context);
        let (result, warnings) = tokio::task::spawn_blocking(move || {
            let mut warnings = Vec::new();
            let r = crate::retrieval::retrieve(
                &root,
                &idea_s,
                &model,
                max_chunks,
                budget,
                &mut warnings,
            );
            (r, warnings)
        })
        .await
        .unwrap_or_else(|e| (Err(format!("retrieval task panicked: {e}")), Vec::new()));
        for w in warnings {
            callbacks.on_note(&format!("⚠ {w}"));
        }
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
            Err(e) => callbacks.on_note(&format!("⚠ retrieval skipped: {e}")),
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
    web_search_cfg: &flows_core::models::WebSearchConfig,
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
    let pushed_context =
        build_pushed_context(context, repo_root, idea, &llm.model, callbacks).await;
    ctx.insert("pushed_context".into(), json!(pushed_context));
    // Per-phase scoping (2.6): expose prior artifacts exactly **once**, as the
    // `prior_artifacts` map. Templates self-select what they need (`.get('plan')`,
    // `['spec']`, `.items()`), so no phase is force-fed the whole history and the
    // old redundant per-key top-level injection is gone.
    // An aliased include's phases also see its artifacts by their short names.
    ctx.insert(
        "prior_artifacts".into(),
        Value::Object(flows_core::include::scoped_artifacts(
            phase,
            prior_artifacts,
        )),
    );
    ctx.insert("vars".into(), Value::Object(phase.vars()));
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

    // Surface the first user turn (the question) so the pane reads as a real
    // conversation, not just the assistant's replies.
    let first_user = format!("Idea: {idea}");
    callbacks.on_user(&first_user);
    let mut messages: Vec<Message> = vec![Message::user(first_user)];

    // Show an up-front context-size estimate so the token count isn't blank while
    // the first (often largest) request is in flight. Replaced by the provider's
    // exact `usage.input_tokens` once the response lands.
    callbacks.on_context(crate::budget::estimate_request_tokens(&system_prompt, &messages) as u64);
    let max_tool_rounds = phase.max_tool_rounds.unwrap_or(MAX_TOOL_ROUNDS);
    let total_rounds = phase.max_clarification_rounds + max_tool_rounds;

    let mut phase_cost = 0.0;
    let mut last_sig: Option<Vec<(String, String)>> = None;
    let mut repeated_errors = 0u32;
    // Clarification budget is spent per *question actually asked*, not per round —
    // so read-only research (tens of tool calls) doesn't starve the phase of its
    // chances to ask the human.
    let mut clarifications_asked = 0u32;
    // Consecutive unproductive web searches (empty/errored); trips the search
    // guard so a flaky backend can't drive an infinite doc-hunt loop.
    let mut dry_searches = 0u32;
    // No-progress guard (write-capable phases only): once the agent has written
    // code, count rounds with no successful `write_file`; abort if it thrashes.
    let can_write = phase.tools.files && !phase.tools.read_only;
    if let Some(note) = limits_note(phase.tools.bash.as_ref()) {
        callbacks.on_note(&note);
    }
    let mut has_written = false;
    let mut rounds_without_write = 0u32;
    // Consecutive rounds whose only write was a no-op (unchanged bytes); drives the
    // nudge/abort loop guard below.
    let mut noop_write_streak = 0u32;
    // read_file dedup cache: call-args → content hash of what we already served, so
    // an unchanged re-read returns a short pointer instead of the full file again.
    let mut read_cache: HashMap<String, u64> = HashMap::new();
    // Same for the other read-only tools, for the stall guard only: an identical
    // call returning an identical result brought nothing new.
    let mut seen_results: HashMap<String, u64> = HashMap::new();
    // Paths the phase has read, listed or seen in a search hit, and the streak
    // of exploration rounds that added none (see EXPLORE_*).
    let mut known_paths: HashSet<String> = HashSet::new();
    let mut explore_streak = 0u32;
    // Consecutive empty final responses — nudge the model to start producing, fail
    // if it keeps stalling.
    let mut empty_streak = 0u32;
    // Consecutive tool rounds with nothing new (see STALL_*), whether the model
    // has been switched to a tool-less reply, and consecutive capped replies.
    let mut stall_streak = 0u32;
    let mut forced_reply = false;
    let mut capped_streak = 0u32;
    let mut tool_defs = tool_defs;

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
            crate::budget::trim_to_budget(
                &mut messages,
                &system_prompt,
                window_tokens(&model, context),
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
            no_tool_calls: forced_reply,
            disable_thinking: !thinking_enabled,
        };
        // Mark the outbound call so the LLM I/O is visible in the pane (the
        // response marker below closes it, with exact token usage).
        callbacks.on_note(&format!(
            "→ request · round {} · {} msg · ~{} tok in",
            round_num + 1,
            messages.len(),
            crate::budget::estimate_request_tokens(&system_prompt, &messages),
        ));
        // Persistent round readout for the status bar (round N / cap).
        callbacks.on_round(round_num + 1, total_rounds);
        // Stream content tokens live, but suppress questions-envelope JSON via a
        // buffering filter. Reasoning is emitted once after the round (below), so
        // the thinking sink here is a no-op.
        let filter = std::sync::Mutex::new(StreamFilter::default());
        let emit = |kind: TokenKind, t: &str| match kind {
            TokenKind::Normal => callbacks.on_token(t),
            TokenKind::Json => callbacks.on_json(t),
        };
        let on_token = |t: &str| crate::lock(&filter).feed(t, &emit);
        let no_thinking = |_: &str| {};
        let t0 = std::time::Instant::now();
        // Race the (possibly long) streaming call against a cancel poll so `c`
        // takes effect within ~300ms even mid-response. Dropping the future aborts
        // the in-flight HTTP request. Without this, cancel only lands at the next
        // round boundary — i.e. after a 30-120s call finishes ("c doesn't cancel").
        let stream_fut = provider.complete_streaming(&req, &on_token, &no_thinking);
        tokio::pin!(stream_fut);
        let stream_result = loop {
            tokio::select! {
                r = &mut stream_fut => break r,
                _ = tokio::time::sleep(std::time::Duration::from_millis(300)) => {
                    if callbacks.cancelled() {
                        crate::lock(&filter).finish(&emit);
                        callbacks.on_note("⛔ cancelled — aborting the in-flight request");
                        return AgentResult {
                            artifact: error_artifact(&phase.name, idea, "cancelled"),
                            cost_usd: phase_cost,
                        };
                    }
                }
            }
        };
        let completion = match stream_result {
            Ok(c) => c,
            Err(e) => {
                // Surface the failure in the output pane — otherwise a dropped LLM
                // connection just makes the stream stop dead with no explanation.
                callbacks.on_note(&format!(
                    "⛔ LLM request failed (round {}): {e} — the model server may be \
                     down or unreachable; check `flows doctor`",
                    round_num + 1
                ));
                return AgentResult {
                    artifact: error_artifact(&phase.name, idea, format!("llm error: {e}")),
                    cost_usd: phase_cost,
                };
            }
        };
        crate::lock(&filter).finish(&emit);
        let latency_ms = t0.elapsed().as_millis() as i64;
        callbacks.on_note(&format!(
            "← response · in {} / out {} tok · {} ms",
            completion.usage.input_tokens, completion.usage.output_tokens, latency_ms,
        ));
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
            let msg = format!(
                "Cost limit ${max_cost_usd:.2} reached (${:.4} spent so far)",
                prior_cost_usd + phase_cost
            );
            callbacks.on_note(&format!("⛔ {msg} — stopping the phase"));
            return AgentResult {
                artifact: error_artifact(&phase.name, idea, msg),
                cost_usd: phase_cost,
            };
        }

        // Tool calls — prefer structured; fall back to any the model leaked as
        // text (some thinking models write `<tool_call>{…}</tool_call>` or
        // `<tools>{…}</tools>` in content/reasoning instead of real tool_calls).
        let tool_calls: Vec<ToolCall> = if forced_reply {
            // Tools are off: anything shaped like a call is part of the reply.
            Vec::new()
        } else if !completion.tool_calls.is_empty() {
            completion.tool_calls.clone()
        } else {
            let mut ex = extract_text_tool_calls(&completion.content);
            if let Some(r) = &completion.reasoning {
                ex.extend(extract_text_tool_calls(r));
            }
            ex
        };

        // Output-cap guard: a reply cut off at max_tokens with no tool call is
        // neither an artifact nor "empty" — the model reasoned past the limit.
        // Tell it so; fail if it happens again.
        let capped = matches!(
            completion.stop_reason.as_deref(),
            Some("length" | "max_tokens")
        );
        if tool_calls.is_empty() && capped {
            capped_streak += 1;
            let limit = req.max_tokens;
            if capped_streak >= MAX_CAPPED_RESPONSES {
                let msg = format!(
                    "the model's reply hit the max_tokens limit ({limit}) {capped_streak} times \
                     in a row before producing anything usable — it reasons past the limit. \
                     Raise the phase's max_tokens or reduce its reasoning."
                );
                callbacks.on_note(&format!("⛔ {msg}"));
                return AgentResult {
                    artifact: error_artifact(&phase.name, idea, msg),
                    cost_usd: phase_cost,
                };
            }
            callbacks.on_note(&format!(
                "⚠ reply cut off at the {limit}-token output limit — nudging the model to be brief"
            ));
            if !completion.content.trim().is_empty() {
                messages.push(Message::assistant_text(completion.content.clone()));
            }
            messages.push(Message::user(format!(
                "Your last reply was cut off at the output limit ({limit} tokens) before you \
                 produced anything usable. Keep your reasoning short: decide, then act right \
                 away — call one tool, or reply with the final artifact JSON. If you are \
                 writing a large file, write it in smaller parts."
            )));
            continue;
        }
        capped_streak = 0;

        if !tool_calls.is_empty() {
            empty_streak = 0; // productive round — reset the empty-response streak
            messages.push(Message::assistant_tools(
                (!completion.content.is_empty()).then(|| completion.content.clone()),
                tool_calls.clone(),
            ));

            let mut all_errored = true;
            let mut wrote_this_round = false;
            let mut noop_write_this_round = false;
            // Did any call bring something new? (See STALL_* above.)
            let mut new_info = false;
            // Did the round do anything besides exploring already-known files?
            let mut explored_new = false;
            for tc in &tool_calls {
                let args: Value = serde_json::from_str(&tc.arguments).unwrap_or(json!({}));
                callbacks.on_tool_call(&tc.name, &summarize_tool_call(&tc.name, &args));

                // Recovery: some models "emit" their artifact by write_file-ing the
                // envelope to a file (e.g. artifact.json) instead of returning it as
                // a reply — which never ends the phase and loops until a guard kills
                // it. If a write's content parses as a valid artifact envelope,
                // accept it as the emission and finish the phase successfully.
                if tc.name == "write_file" {
                    if let Some(parsed) = args
                        .get("content")
                        .and_then(Value::as_str)
                        .and_then(parse_response)
                        .filter(|p| p.get("type").and_then(Value::as_str) == Some("artifact"))
                    {
                        callbacks.on_note(
                            "✓ accepted artifact from a write_file (model wrote the \
                             envelope to a file instead of replying) — finishing phase",
                        );
                        return AgentResult {
                            artifact: Artifact {
                                phase: phase.name.clone(),
                                idea: idea.to_string(),
                                content: artifact_content(Some(parsed)),
                                ..Artifact::default()
                            },
                            cost_usd: phase_cost,
                        };
                    }
                }

                // Detect a no-op write up front: content byte-for-byte identical to
                // what's already on disk. Such a write "succeeds" but makes no
                // progress — don't let it reset the no-progress counter, and feed
                // the loop guard below.
                let noop_write = tc.name == "write_file"
                    && match (
                        args.get("path").and_then(Value::as_str),
                        args.get("content").and_then(Value::as_str),
                    ) {
                        (Some(p), Some(c)) => files::read_file(repo_root, p)
                            .map(|cur| cur == c)
                            .unwrap_or(false),
                        _ => false,
                    };

                // Search guard: once too many searches have come back empty/failed,
                // stop dispatching them — tell the model to proceed on its own
                // knowledge instead of looping on a flaky backend.
                if tc.name == "web_search" && dry_searches >= MAX_DRY_SEARCHES {
                    callbacks.on_tool_result(
                        &tc.name,
                        "search disabled (too many empty/failed results)",
                        true,
                    );
                    messages.push(Message::tool_result(
                        tc.id.clone(),
                        "web_search is unavailable (repeated empty/failed results). Stop \
                         searching. Implement using your existing knowledge; if a specific API \
                         detail is uncertain, make a reasonable assumption, note it, and continue."
                            .to_string(),
                    ));
                    continue;
                }
                let mut deduped = false;

                let mut result = dispatch_tool(
                    &tc.name,
                    &args,
                    &phase.tools,
                    context,
                    web_search_cfg,
                    repo_root,
                    flows_dir,
                )
                .await;
                // Read dedup: local models frequently re-read the same unchanged
                // file dozens of times (observed: 137 reads of 17 files), bloating
                // context and burning rounds. Serve a re-read of identical content
                // as a short pointer instead of the full text. A changed file (the
                // model edited it since) hashes differently, so it's served in full
                // and re-cached. Keyed on the exact call args so different ranges of
                // one file aren't collapsed together.
                if tc.name == "read_file" && !result.starts_with("Error") {
                    let key = format!("{}|{}", tc.name, tc.arguments);
                    let h = hash_str(&result);
                    if read_cache.get(&key) == Some(&h) {
                        let path = args.get("path").and_then(Value::as_str).unwrap_or("");
                        result = format!(
                            "(You already read {path:?} earlier this phase and it is unchanged \
                             ({} chars). Reuse that copy — re-reading the same file wastes context \
                             and rounds. Edit it with write_file if you need to change it.)",
                            result.chars().count()
                        );
                        deduped = true;
                    } else {
                        read_cache.insert(key, h);
                    }
                }
                let (summary, is_error) = summarize_tool_result(&result);
                callbacks.on_tool_result(&tc.name, &summary, is_error);
                // Track search productivity: a search with no result URL (empty or
                // errored) is "dry"; a real hit resets the counter.
                let mut dry = false;
                if tc.name == "web_search" {
                    if !is_error && result.contains("\"url\"") {
                        dry_searches = 0;
                    } else if is_error && backend_unreachable(&result) {
                        // The backend is down, not just empty: stop now.
                        dry_searches = MAX_DRY_SEARCHES;
                        dry = true;
                    } else {
                        dry_searches += 1;
                        dry = true;
                    }
                }
                let failed_fetch = matches!(tc.name.as_str(), "http_get" | "lookup")
                    && (is_error || http_error_status(&result));
                let repeated = matches!(
                    tc.name.as_str(),
                    "list_files" | "search_code" | "web_search" | "http_get" | "lookup"
                ) && seen_results
                    .insert(format!("{}|{}", tc.name, tc.arguments), hash_str(&result))
                    == Some(hash_str(&result));
                let fresh = !(deduped || dry || failed_fetch || repeated);
                if fresh {
                    new_info = true;
                }
                explored_new |= match tc.name.as_str() {
                    "read_file" => fresh,
                    "list_files" | "search_code" => {
                        let mut any = false;
                        for p in touched_paths(&tc.name, &result) {
                            any |= known_paths.insert(p);
                        }
                        any
                    }
                    // Anything else (bash, a write, a fetch) is not exploration.
                    _ => fresh,
                };
                if tc.name == "read_file" {
                    if let Some(p) = args.get("path").and_then(Value::as_str) {
                        known_paths.insert(norm_path(p));
                    }
                }
                if !is_error {
                    all_errored = false;
                    if tc.name == "write_file" {
                        if noop_write {
                            noop_write_this_round = true;
                        } else {
                            wrote_this_round = true;
                        }
                    }
                }
                messages.push(Message::tool_result(tc.id.clone(), result));
            }

            // Once search is off, stop offering it: a listed tool keeps getting called.
            if dry_searches >= MAX_DRY_SEARCHES && tool_defs.iter().any(|d| d.name == "web_search")
            {
                tool_defs.retain(|d| d.name != "web_search");
                callbacks.on_note(
                    "⚠ web_search disabled for the rest of this phase (backend unreachable or \
                     repeatedly empty)",
                );
            }

            // Stall guard: rounds that brought nothing new — re-reads of unchanged
            // files, dead searches, failed fetches. Nudge, then force a reply
            // (read-only phases) or abort (write-capable ones).
            if wrote_this_round || new_info {
                stall_streak = 0;
            } else {
                stall_streak += 1;
            }
            if stall_streak == STALL_NUDGE_ROUNDS {
                callbacks.on_note(
                    "⚠ stall guard: tool calls are returning nothing new — nudging the model to produce its output",
                );
                messages.push(Message::user(if can_write {
                    "Your last few tool calls returned nothing new (re-reads of unchanged files, \
                     failed searches or fetches). You already have everything these tools will \
                     give you. Stop gathering and start producing: write the next file with \
                     `write_file`; when a detail is uncertain, choose the straightforward option \
                     and note it."
                } else {
                    "Your last few tool calls returned nothing new (re-reads of unchanged files, \
                     failed searches or fetches). You already have everything these tools will \
                     give you. Stop gathering and reply now with the final artifact JSON; when a \
                     detail is uncertain, choose the straightforward option and note it as an \
                     assumption."
                }));
            }
            if wrote_this_round || explored_new {
                explore_streak = 0;
            } else {
                explore_streak += 1;
            }
            if explore_streak == EXPLORE_NUDGE_ROUNDS && stall_streak < STALL_NUDGE_ROUNDS {
                callbacks.on_note(
                    "⚠ stall guard: exploration keeps returning files already seen — nudging the model to produce its output",
                );
                messages.push(Message::user(format!(
                    "Your last {EXPLORE_NUDGE_ROUNDS} rounds of reading and searching found nothing \
                     you had not already seen. The repository has nothing more to show you for \
                     this task. Stop exploring and {}.",
                    if can_write {
                        "write the code with `write_file`"
                    } else {
                        "reply now with the final artifact JSON"
                    }
                )));
            }
            let explore_force = explore_streak >= EXPLORE_FORCE_ROUNDS;
            if (stall_streak >= STALL_FORCE_ROUNDS || explore_force) && !can_write && !forced_reply
            {
                forced_reply = true;
                callbacks.on_note(
                    "⚠ stall guard: tools switched off — the model must reply with its artifact",
                );
                messages.push(Message::user(
                    "Tools are now switched off for this phase. Reply with the final artifact \
                     JSON using what you already know.",
                ));
            }
            if stall_streak >= STALL_ABORT_ROUNDS && can_write {
                let msg = format!(
                    "aborted: {stall_streak} tool rounds in a row returned nothing new (re-reads, \
                     failed searches or fetches) — the model is stalled"
                );
                callbacks.on_note(&format!("⛔ stall guard: {msg}"));
                return AgentResult {
                    artifact: error_artifact(&phase.name, idea, msg),
                    cost_usd: phase_cost,
                };
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
                let msg = format!(
                    "aborted: no code written in {MAX_ROUNDS_WITHOUT_PROGRESS} tool rounds after \
starting — likely stuck in a build/search loop"
                );
                callbacks.on_note(&format!("⛔ loop guard: {msg}"));
                return AgentResult {
                    artifact: error_artifact(&phase.name, idea, msg),
                    cost_usd: phase_cost,
                };
            }

            // No-op-write loop guard: a model rewriting a file whose bytes never
            // change is wedged. Count consecutive rounds whose only write was a
            // no-op; nudge the model once, then abort if it keeps spinning.
            if wrote_this_round {
                noop_write_streak = 0;
            } else if noop_write_this_round {
                noop_write_streak += 1;
            } else {
                noop_write_streak = 0;
            }
            if noop_write_streak == NUDGE_AFTER_NOOP_WRITES {
                callbacks.on_note(
                    "⚠ loop guard: same file rewritten unchanged repeatedly — nudging the model to move on",
                );
                messages.push(Message::user(
                    "You have written the same file with identical content several rounds in a row; \
                     it is unchanged on disk. Stop rewriting it. If your work for this phase is \
                     complete, return the final artifact JSON as your plain reply message with no \
                     tool call — do NOT write it to a file (a `write_file` to artifact.json is not \
                     how the phase ends). Otherwise take a different action: build or test the code, \
                     edit a different file, or write genuinely new content.",
                ));
            }
            if noop_write_streak >= ABORT_AFTER_NOOP_WRITES {
                let msg = format!(
                    "aborted: stuck rewriting an unchanged file for {ABORT_AFTER_NOOP_WRITES} rounds \
— loop detected"
                );
                callbacks.on_note(&format!("⛔ loop guard: {msg}"));
                return AgentResult {
                    artifact: error_artifact(&phase.name, idea, msg),
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
                let msg = format!(
                    "aborted: tool call(s) [{}] failed identically {repeated_errors} rounds in a row without progress",
                    names.join(", ")
                );
                callbacks.on_note(&format!("⛔ loop guard: {msg}"));
                return AgentResult {
                    artifact: error_artifact(&phase.name, idea, msg),
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
                if clarifications_asked >= phase.max_clarification_rounds {
                    messages.push(Message::user("(proceeding without further clarification)"));
                } else {
                    let answer = callbacks.on_questions(&questions).await;
                    clarifications_asked += 1;
                    messages.push(Message::user(answer));
                }
                continue;
            }
            Some("artifact") => {
                let content = artifact_content(parsed);
                // No-tools models can't call write_file, so they inline files in
                // `content.files` (path → contents); write them here.
                let mut inline_wrote = false;
                if !tools_enabled {
                    if let Some(files) = content.get("files").and_then(Value::as_object) {
                        for (path, body) in files {
                            if let Some(text) = body.as_str() {
                                match files::write_file(repo_root, path, text) {
                                    Ok(_) => {
                                        inline_wrote = true;
                                        callbacks.on_tool_result(
                                            "write_file",
                                            &format!("wrote {path}"),
                                            false,
                                        )
                                    }
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
                // Empty-output guard: a write-capable phase that reports files but
                // never actually wrote any (via write_file or inline) — the model
                // inlined code in its response instead of writing it. The phase
                // would otherwise "succeed" with an empty repo, only to fail
                // confusingly downstream (lint can't find a project). Fail here with
                // an actionable message instead.
                let claimed_files = content
                    .get("files_written")
                    .and_then(Value::as_array)
                    .map(|a| !a.is_empty())
                    .unwrap_or(false);
                if can_write && claimed_files && !has_written && !inline_wrote {
                    let msg = "artifact reports files_written but no file was actually \
                               written — call write_file for each file; do not inline code \
                               in the response"
                        .to_string();
                    callbacks.on_note(&format!("⛔ {msg}"));
                    return AgentResult {
                        artifact: error_artifact(&phase.name, idea, msg),
                        cost_usd: phase_cost,
                    };
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
                // Empty final response — the model ended the round with no tool
                // call, no artifact, and no text. Observed: an implement phase that
                // read files, reasoned itself in circles, then returned "". Rather
                // than accept it as an empty artifact (a silent do-nothing
                // "success"), nudge it to start producing and continue — it usually
                // has plenty of round budget left. Fail only if it keeps stalling.
                if completion.content.trim().is_empty() {
                    empty_streak += 1;
                    if empty_streak >= MAX_EMPTY_RESPONSES {
                        let msg = format!(
                            "agent produced an empty response {empty_streak} times — no \
                             artifact, no tool call, nothing written. The phase did no work."
                        );
                        callbacks.on_note(&format!("⛔ {msg}"));
                        return AgentResult {
                            artifact: error_artifact(&phase.name, idea, msg),
                            cost_usd: phase_cost,
                        };
                    }
                    callbacks.on_note(
                        "⚠ empty response — nudging the model to act instead of deliberate",
                    );
                    let nudge = if can_write {
                        "Your last response was empty — you produced nothing. Stop \
                         deliberating and act now: call a tool. If you are implementing, \
                         write the next file with `write_file` (start with the simplest \
                         one); when a detail is ambiguous, implement the straightforward \
                         version, note the assumption in a comment, and keep going. Do not \
                         analyze edge cases before the core code exists. When all work is \
                         written to disk, emit the final artifact JSON."
                    } else {
                        "Your last response was empty — you produced nothing. Take a \
                         concrete action now: call a tool, or emit your final artifact \
                         JSON. Do not reply with empty or filler content."
                    };
                    messages.push(Message::user(nudge));
                    continue;
                }
                // Unparsed non-empty text — treat raw output as the artifact.
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

    // Exhausted the round budget without emitting an artifact — surface it in the
    // output pane (the loop-guard aborts do the same) so a killed run isn't silent.
    callbacks.on_note(&format!(
        "⛔ max rounds exceeded: hit the {total_rounds}-round cap without finishing — \
         raise max_tool_rounds or simplify the task"
    ));
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
    fn extracts_json_envelope_tool_calls() {
        // Reasoner emitted the call as bare JSON `{"type":"tool_call",...}` (not
        // structured tool_calls, not an XML tag) — must still execute, else the
        // phase ends prematurely with the JSON captured as its artifact.
        let text = "First list the files.\n{\n  \"type\": \"tool_call\",\n  \"name\": \"list_files\",\n  \"arguments\": {\"path\": \"\"}\n}";
        let calls = extract_text_tool_calls(text);
        assert_eq!(calls.len(), 1, "envelope not parsed: {calls:?}");
        assert_eq!(calls[0].name, "list_files");

        // Fenced, OpenAI `function` wrapper.
        let fenced = "```json\n{\"function\": {\"name\": \"read_file\", \"arguments\": {\"path\": \"a.rs\"}}}\n```";
        assert_eq!(extract_text_tool_calls(fenced)[0].name, "read_file");

        // A `tool_calls` array envelope.
        let arr = "{\"tool_calls\": [{\"name\": \"list_files\", \"arguments\": {}}, {\"name\": \"read_file\", \"arguments\": {\"path\":\"x\"}}]}";
        assert_eq!(extract_text_tool_calls(arr).len(), 2);

        // An artifact envelope must NOT be mistaken for a tool call.
        let artifact = "{\"type\": \"artifact\", \"content\": {\"summary\": \"done\"}}";
        assert!(extract_text_tool_calls(artifact).is_empty());
        // A questions envelope likewise.
        let questions = "{\"type\": \"questions\", \"questions\": [\"a?\", \"b?\"]}";
        assert!(extract_text_tool_calls(questions).is_empty());
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
            TokenKind::Normal => crate::lock(&normal).push_str(t),
            TokenKind::Json => crate::lock(&json).push_str(t),
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

#[cfg(test)]
mod retrieval_budget_tests {
    use super::retrieval_budget;
    use flows_core::models::ContextConfig;

    #[test]
    fn budget_is_fraction_of_known_window() {
        let ctx = ContextConfig::default(); // fraction 0.25
        assert_eq!(retrieval_budget("claude-sonnet-4-6", &ctx), 50_000);
    }

    #[test]
    fn unknown_model_uses_unknown_window_tokens() {
        let ctx = ContextConfig {
            unknown_window_tokens: 8000,
            ..ContextConfig::default()
        };
        assert_eq!(retrieval_budget("my-local-model-xyz", &ctx), 2000);
    }

    #[test]
    fn fraction_is_configurable() {
        let ctx = ContextConfig {
            unknown_window_tokens: 8000,
            retrieval_budget_fraction: 0.5,
            ..ContextConfig::default()
        };
        assert_eq!(retrieval_budget("my-local-model-xyz", &ctx), 4000);
    }
}

#[cfg(test)]
mod limits_note_tests {
    use super::limits_note;
    use flows_core::models::BashConfig;
    use flows_tools::sandbox::{limits_mode, LimitMode};

    #[test]
    fn no_note_without_bash_or_when_unlimited() {
        assert!(limits_note(None).is_none());
        let unlimited = BashConfig {
            memory_mb: Some(0),
            max_procs: Some(0),
            ..BashConfig::default()
        };
        assert!(limits_note(Some(&unlimited)).is_none());
    }

    #[test]
    fn note_matches_host_mode() {
        let note = limits_note(Some(&BashConfig::default()));
        assert_eq!(
            note.is_some(),
            limits_mode() != LimitMode::Cgroup,
            "{note:?}"
        );
    }
}
