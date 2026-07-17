//! `flows eval` — a model × context-config × suite × size evaluation matrix over
//! task fixtures, with a persistent results store, deferred manual validation, and
//! an offline comparison report.
//!
//! A fixture is a directory with `fixture.yml` (task / suite / size / verify /
//! rubric) and a `repo/` subtree (the starting repository). Built-in starters are
//! embedded; point `--fixtures <dir>` at your own to benchmark real tasks.
//!
//! - `eval run` runs the matrix and appends a record per trial to the store.
//! - `eval list` shows discovered fixtures (filterable by suite/size).
//! - `eval review` grades ungraded trials (the pass signal for reasoner tasks).
//! - `eval report` prints a grouped comparison table from the store.

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::process::Command;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use include_dir::{include_dir, Dir};
use serde::Deserialize;

use flows_core::config::{merge_llm, merge_profiles, parse_context_config, resolve_llm};
use flows_core::models::{
    ContextConfig, GlobalConfig, LLMConfig, PhaseConfig, PhaseType, ProjectConfig,
};
use flows_core::state::Store;
use flows_llm::{CompletionRequest, Message};
use flows_runner::runner::{run_pipeline, RunnerCallbacks};

use crate::eval_store::{self, Ratings, TrialMeta, TrialRecord};

static STARTER_FIXTURES: Dir<'_> = include_dir!("$CARGO_MANIFEST_DIR/assets/eval/fixtures");

// ---------------------------------------------------------------------------
// Fixture model
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Suite {
    #[default]
    Coder,
    Reasoner,
}
impl Suite {
    fn as_str(self) -> &'static str {
        match self {
            Suite::Coder => "coder",
            Suite::Reasoner => "reasoner",
        }
    }
    fn parse_filter(s: &str) -> Result<Option<Suite>, String> {
        match s {
            "all" | "" => Ok(None),
            "coder" => Ok(Some(Suite::Coder)),
            "reasoner" => Ok(Some(Suite::Reasoner)),
            other => Err(format!("unknown suite `{other}` (coder|reasoner|all)")),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Size {
    Small,
    #[default]
    Medium,
    Large,
}
impl Size {
    fn as_str(self) -> &'static str {
        match self {
            Size::Small => "small",
            Size::Medium => "medium",
            Size::Large => "large",
        }
    }
    fn parse_filter(s: &str) -> Result<Option<Size>, String> {
        match s {
            "all" | "" => Ok(None),
            "small" => Ok(Some(Size::Small)),
            "medium" => Ok(Some(Size::Medium)),
            "large" => Ok(Some(Size::Large)),
            other => Err(format!("unknown size `{other}` (small|medium|large|all)")),
        }
    }
}

/// How much of the pipeline a fixture exercises. Different granularities answer
/// different questions and MUST NOT be compared against each other — the report
/// groups by granularity so they never mix.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Granularity {
    /// The whole pipeline (plan→implement→…): realistic end-to-end success, but
    /// high variance — context-feature effects are confounded by orchestration.
    #[default]
    Flow,
    /// A single agent phase (one tool loop): where context delivery actually
    /// lives; the clean A/B unit for context features.
    Phase,
    /// A single model turn (one prompt → one completion, no tool loop): the
    /// cheapest, lowest-variance probe. Coder turns auto-grade via `expect`
    /// substrings; reasoner turns are graded manually.
    Turn,
}
impl Granularity {
    fn as_str(self) -> &'static str {
        match self {
            Granularity::Flow => "flow",
            Granularity::Phase => "phase",
            Granularity::Turn => "turn",
        }
    }
    fn parse_filter(s: &str) -> Result<Option<Granularity>, String> {
        match s {
            "all" | "" => Ok(None),
            "flow" => Ok(Some(Granularity::Flow)),
            "phase" => Ok(Some(Granularity::Phase)),
            "turn" => Ok(Some(Granularity::Turn)),
            other => Err(format!(
                "unknown granularity `{other}` (flow|phase|turn|all)"
            )),
        }
    }
}

#[derive(Debug, Deserialize)]
struct FixtureSpec {
    task: String,
    #[serde(default = "default_template")]
    template: String,
    #[serde(default)]
    suite: Suite,
    #[serde(default)]
    size: Size,
    #[serde(default)]
    granularity: Granularity,
    /// Shell command whose exit 0 == success. Required for flow/phase coder fixtures.
    #[serde(default)]
    verify: Option<String>,
    /// Substrings the completion must contain (turn coder auto-grade). Required
    /// for `granularity: turn` coder fixtures (there is no repo to shell-verify).
    #[serde(default)]
    expect: Vec<String>,
    /// Optional system prompt for a `turn` fixture.
    #[serde(default)]
    system: Option<String>,
    /// Max completion tokens for a `turn` fixture (default 2048).
    #[serde(default)]
    max_tokens: Option<u32>,
    /// Human-grading criteria shown in `review`. Required for reasoner fixtures.
    #[serde(default)]
    rubric: Option<String>,
    /// Labeled relevant files/symbols — the context a good run should surface.
    /// A label for now (used by the future context-delivery + retrieval evals);
    /// the eval does NOT inject them, so trials run the real flow unmodified.
    #[serde(default)]
    #[allow(dead_code)]
    relevant: Vec<String>,
    /// Context feature configs this fixture runs under, each parsed by
    /// `parse_context_config` (e.g. `[baseline, all]`, or `search+range+slim`).
    /// This is the fixture's own config axis: an on/off matrix declared in the
    /// fixture so, e.g., a reasoner fixture always runs both with and without the
    /// context features. When empty, the fixture uses the run's `--configs` axis
    /// (default `baseline`).
    #[serde(default)]
    configs: Vec<String>,
    #[serde(default = "default_timeout")]
    timeout_secs: u64,
}
fn default_template() -> String {
    "blank".into()
}
fn default_timeout() -> u64 {
    600
}

struct Fixture {
    name: String,
    spec: FixtureSpec,
    /// (path relative to the repo root, contents).
    repo_files: Vec<(String, Vec<u8>)>,
}

/// Reject fixtures whose suite/granularity lacks its required success signal.
/// Reasoner → `rubric` (manual). Coder `turn` → `expect` (substring auto-grade,
/// no repo to shell-verify). Coder `flow`/`phase` → `verify` (shell exit 0).
fn validate_fixture(name: &str, spec: &FixtureSpec) -> Result<(), String> {
    let missing = |s: Option<&String>| s.map(|x| x.trim().is_empty()).unwrap_or(true);
    match (spec.suite, spec.granularity) {
        (Suite::Reasoner, _) if missing(spec.rubric.as_ref()) => Err(format!(
            "fixture `{name}`: reasoner suite requires a `rubric`"
        )),
        (Suite::Coder, Granularity::Turn) if spec.expect.iter().all(|e| e.trim().is_empty()) => {
            Err(format!(
                "fixture `{name}`: coder `turn` requires `expect` (substrings the completion must contain)"
            ))
        }
        (Suite::Coder, Granularity::Flow | Granularity::Phase) if missing(spec.verify.as_ref()) => {
            Err(format!(
                "fixture `{name}`: coder {} requires a `verify` command",
                spec.granularity.as_str()
            ))
        }
        _ => Ok(()),
    }
}

/// Resolve a fixture's template into its phases (profile routing stripped so the
/// whole run stays on the pinned model). No repo needed — template resolution
/// reads the named template, not the fixture repo.
fn resolve_fixture_phases(template: &str) -> Result<Vec<PhaseConfig>, String> {
    let tmp = tempfile::tempdir().map_err(|e| e.to_string())?;
    let (phases, _, _, _) = super::resolve(tmp.path(), Some(template))?;
    Ok(phases
        .into_iter()
        .map(|mut p| {
            p.model = None;
            p
        })
        .collect())
}

/// Number of agent (LLM) phases in a resolved template.
fn agent_phase_count(phases: &[PhaseConfig]) -> usize {
    phases
        .iter()
        .filter(|p| matches!(p.phase_type, PhaseType::Agent))
        .count()
}

/// Validate a fixture's declared granularity against its resolved phases:
/// `phase` must be a single agent phase; `turn` is reserved.
fn validate_granularity(
    name: &str,
    gran: Granularity,
    template: &str,
    phases: &[PhaseConfig],
) -> Result<(), String> {
    match gran {
        // `turn` runs no pipeline — validated by validate_fixture (needs expect/rubric).
        Granularity::Turn => Ok(()),
        Granularity::Phase if agent_phase_count(phases) != 1 => Err(format!(
            "fixture `{name}`: granularity `phase` requires exactly one agent phase, \
but template `{template}` has {}",
            agent_phase_count(phases)
        )),
        _ => Ok(()),
    }
}

// ---------------------------------------------------------------------------
// Fixture loading
// ---------------------------------------------------------------------------

fn collect_dir_files(dir: &Dir, strip: &str, out: &mut Vec<(String, Vec<u8>)>) {
    for f in dir.files() {
        let full = f.path().to_string_lossy();
        if let Some(rel) = full.strip_prefix(strip) {
            out.push((
                rel.trim_start_matches('/').to_string(),
                f.contents().to_vec(),
            ));
        }
    }
    for d in dir.dirs() {
        collect_dir_files(d, strip, out);
    }
}

fn load_embedded() -> Result<Vec<Fixture>, String> {
    let mut out = Vec::new();
    for dir in STARTER_FIXTURES.dirs() {
        let name = dir
            .path()
            .file_name()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        let Some(spec_file) = STARTER_FIXTURES.get_file(format!("{name}/fixture.yml")) else {
            continue;
        };
        let spec: FixtureSpec = serde_yaml_ng::from_slice(spec_file.contents())
            .map_err(|e| format!("fixture `{name}`: {e}"))?;
        validate_fixture(&name, &spec)?;
        let mut repo_files = Vec::new();
        if let Some(repo) = STARTER_FIXTURES.get_dir(format!("{name}/repo")) {
            collect_dir_files(repo, &format!("{name}/repo"), &mut repo_files);
        }
        out.push(Fixture {
            name,
            spec,
            repo_files,
        });
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(out)
}

fn walk_disk(base: &Path, root: &Path, out: &mut Vec<(String, Vec<u8>)>) {
    let Ok(entries) = std::fs::read_dir(root) else {
        return;
    };
    for e in entries.flatten() {
        let p = e.path();
        if p.is_dir() {
            walk_disk(base, &p, out);
        } else if let Ok(bytes) = std::fs::read(&p) {
            if let Ok(rel) = p.strip_prefix(base) {
                out.push((rel.to_string_lossy().into_owned(), bytes));
            }
        }
    }
}

fn load_disk(dir: &Path) -> Result<Vec<Fixture>, String> {
    let mut out = Vec::new();
    let entries = std::fs::read_dir(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    for e in entries.flatten() {
        let fdir = e.path();
        let spec_path = fdir.join("fixture.yml");
        if !spec_path.is_file() {
            continue;
        }
        let text = std::fs::read(&spec_path).map_err(|e| e.to_string())?;
        let spec: FixtureSpec = serde_yaml_ng::from_slice(&text)
            .map_err(|e| format!("{}: {e}", spec_path.display()))?;
        let name = fdir
            .file_name()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        validate_fixture(&name, &spec)?;
        let repo = fdir.join("repo");
        let mut repo_files = Vec::new();
        if repo.is_dir() {
            walk_disk(&repo, &repo, &mut repo_files);
        }
        out.push(Fixture {
            name,
            spec,
            repo_files,
        });
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(out)
}

fn load_fixtures(fixtures_dir: Option<&str>) -> Result<Vec<Fixture>, String> {
    let fx = match fixtures_dir {
        Some(d) => load_disk(Path::new(d))?,
        None => load_embedded()?,
    };
    if fx.is_empty() {
        return Err("no fixtures found (point --fixtures at a dir of fixture folders)".into());
    }
    Ok(fx)
}

fn filtered(
    fx: &[Fixture],
    suite: Option<Suite>,
    size: Option<Size>,
    gran: Option<Granularity>,
) -> Vec<&Fixture> {
    fx.iter()
        .filter(|f| suite.map_or(true, |s| f.spec.suite == s))
        .filter(|f| size.map_or(true, |s| f.spec.size == s))
        .filter(|f| gran.map_or(true, |g| f.spec.granularity == g))
        .collect()
}

// ---------------------------------------------------------------------------
// Running one trial
// ---------------------------------------------------------------------------

/// Headless callbacks: auto-proceed through questions and human gates so a
/// fixture run never blocks; emit nothing.
struct EvalCallbacks;
#[async_trait]
impl RunnerCallbacks for EvalCallbacks {
    async fn on_questions(&self, _phase: &str, _questions: &[String]) -> String {
        "Proceed with your best assumption; do not ask for more input.".into()
    }
    async fn on_human_gate(&self, _phase: &str, _instructions: &str) -> String {
        String::new() // empty = approve
    }
}

/// Materialize the fixture repo into a fresh temp dir and `git init` it.
fn materialize(fixture: &Fixture) -> Result<tempfile::TempDir, String> {
    let dir = tempfile::Builder::new()
        .prefix("flows-eval-")
        .tempdir()
        .map_err(|e| e.to_string())?;
    for (rel, bytes) in &fixture.repo_files {
        let path = dir.path().join(rel);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        std::fs::write(&path, bytes).map_err(|e| e.to_string())?;
    }
    // Best-effort git init so commit phases + gitignore + diff capture behave.
    let _ = Command::new("git")
        .args(["init", "-q"])
        .current_dir(dir.path())
        .status();
    let _ = Command::new("git")
        .args(["config", "user.email", "eval@flows.local"])
        .current_dir(dir.path())
        .status();
    let _ = Command::new("git")
        .args(["config", "user.name", "flows eval"])
        .current_dir(dir.path())
        .status();
    let _ = Command::new("git")
        .args(["add", "-A"])
        .current_dir(dir.path())
        .status();
    let _ = Command::new("git")
        .args(["commit", "-q", "-m", "fixture baseline", "--allow-empty"])
        .current_dir(dir.path())
        .status();
    Ok(dir)
}

/// Run the fixture's `verify` command in the work dir; exit 0 == success.
fn verify(work: &Path, cmd: &str) -> bool {
    Command::new("sh")
        .arg("-c")
        .arg(cmd)
        .current_dir(work)
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Capture the agent's changes as a diff (staged so new files show).
fn capture_diff(work: &Path) -> String {
    let _ = Command::new("git")
        .args(["add", "-A"])
        .current_dir(work)
        .status();
    Command::new("git")
        .args(["--no-pager", "diff", "--cached"])
        .current_dir(work)
        .output()
        .ok()
        .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
        .unwrap_or_default()
}

/// Collect a trial's reviewable output: each phase's artifact text (the reasoner
/// signal) followed by the repo diff (the coder signal). Reads run artifacts, so
/// call before the run is deleted.
fn capture_output(work: &Path, store: &Store, run_id: &str, phases: &[PhaseConfig]) -> String {
    let mut s = String::new();
    for ph in phases {
        let path = store.artifact_path(run_id, &ph.name);
        if let Ok(text) = std::fs::read_to_string(&path) {
            if !text.trim().is_empty() {
                s.push_str(&format!(
                    "=== artifact: {} ===\n{}\n\n",
                    ph.name,
                    text.trim()
                ));
            }
        }
    }
    let diff = capture_diff(work);
    if !diff.trim().is_empty() {
        s.push_str(&format!("=== diff ===\n{diff}\n"));
    }
    s
}

/// Build the oracle payload: each of the fixture's `relevant` entries that is an
/// existing file in the materialized repo, read and fenced with its path. Symbol
/// labels (entries that aren't files) are skipped. Empty when nothing resolves.
fn oracle_blob(fixture: &Fixture, work: &Path) -> String {
    fixture
        .spec
        .relevant
        .iter()
        .filter_map(|rel| {
            let p = work.join(rel);
            std::fs::read_to_string(&p)
                .ok()
                .map(|c| format!("### {rel}\n```\n{}\n```", c.trim_end()))
        })
        .collect::<Vec<_>>()
        .join("\n\n")
}

/// Pin a run to one model + one context config, ignoring profile routing (every
/// phase runs on `model`). Returns synthetic (global, project) configs.
fn pin_config(
    base_global: &GlobalConfig,
    base_project: &ProjectConfig,
    model: &LLMConfig,
    ctx: ContextConfig,
) -> (GlobalConfig, ProjectConfig) {
    let mut g = base_global.clone();
    g.llm = model.clone();
    g.models = HashMap::new();
    let mut p = base_project.clone();
    p.llm = model.clone();
    p.models = HashMap::new();
    // Start from the user's context config (so retrieval_model / max_chunks from
    // `~/.flows/config.yml` survive) and overlay only the token-selected flags.
    // Seed from global (where users set retrieval_model); a project `context`
    // block overrides the retrieval settings when present.
    let mut merged = base_global.context.clone();
    if !base_project.context.retrieval_model.is_empty() {
        merged.retrieval_model = base_project.context.retrieval_model.clone();
        merged.retrieval_max_chunks = base_project.context.retrieval_max_chunks;
        merged.retrieval_reindex_threshold = base_project.context.retrieval_reindex_threshold;
    }
    merged.search_tool = ctx.search_tool;
    merged.range_reads = ctx.range_reads;
    merged.slim_tree = ctx.slim_tree;
    merged.repo_map = ctx.repo_map;
    merged.budgeting = ctx.budgeting;
    merged.retrieval = ctx.retrieval;
    merged.oracle = ctx.oracle;
    p.context = merged;
    (g, p)
}

#[allow(clippy::too_many_arguments)]
fn run_trial(
    rt: &tokio::runtime::Runtime,
    fixture: &Fixture,
    phases: &[PhaseConfig],
    model_label: &str,
    model_llm: &LLMConfig,
    config_name: &str,
    ctx: &ContextConfig,
    base_global: &GlobalConfig,
    base_project: &ProjectConfig,
    store: &Store,
    flows_dir: &Path,
) -> TrialRecord {
    let trial_id = uuid::Uuid::new_v4().to_string();
    let started = Instant::now();

    let rec = |verify_pass, in_tok, out_tok, peak, rounds, note: Option<String>| TrialRecord {
        trial: trial_id.clone(),
        ts: eval_store::now_secs(),
        model: model_label.to_string(),
        config: config_name.to_string(),
        fixture: fixture.name.clone(),
        suite: fixture.spec.suite.as_str().to_string(),
        size: fixture.spec.size.as_str().to_string(),
        granularity: fixture.spec.granularity.as_str().to_string(),
        verify_pass,
        manual_pass: None,
        manual_score: None,
        ratings: None,
        note,
        in_tok,
        out_tok,
        peak_ctx: peak,
        cost: 0.0,
        rounds,
        secs: started.elapsed().as_secs_f64(),
    };

    let work = match materialize(fixture) {
        Ok(d) => d,
        Err(e) => return rec(None, 0, 0, 0, 0, Some(format!("setup: {e}"))),
    };

    let (global, project) = pin_config(base_global, base_project, model_llm, ctx.clone());

    // Oracle push (stopgap): make the fixture's labeled `relevant` files available
    // to the runner via an env var, so a no-tool model gets the code up front. The
    // runner reads it only when `context.oracle` is set. Cleared after the run.
    // Trials run sequentially, so the process-global env var is safe here.
    if ctx.oracle {
        std::env::set_var("FLOWS_ORACLE_CONTEXT", oracle_blob(fixture, work.path()));
    }

    let run_id = uuid::Uuid::new_v4().to_string();
    let cb = EvalCallbacks;
    let fut = run_pipeline(
        &run_id,
        &fixture.spec.task,
        work.path(),
        phases,
        &fixture.spec.template,
        &global,
        &project,
        &flows_runner::default_provider_factory,
        &cb,
        store,
        None,
        Some(flows_dir),
    );
    let timed = rt.block_on(async {
        tokio::time::timeout(Duration::from_secs(fixture.spec.timeout_secs), fut).await
    });
    let run_note = match timed {
        Ok(Ok(_)) => None,
        Ok(Err(e)) => Some(format!("run: {e}")),
        Err(_) => Some("timeout".into()),
    };
    if ctx.oracle {
        std::env::remove_var("FLOWS_ORACLE_CONTEXT");
    }

    // Verify (coder pass signal). None when the fixture defines no verifier.
    let verify_pass = fixture
        .spec
        .verify
        .as_deref()
        .filter(|c| !c.trim().is_empty())
        .map(|c| verify(work.path(), c));

    // Capture the trial output (phase artifacts + repo diff) BEFORE deleting the
    // run — reasoner tasks produce text artifacts, not repo changes. Save it +
    // task/rubric for later manual review.
    let output = capture_output(work.path(), store, &run_id, phases);
    let _ = eval_store::save_output(
        flows_dir,
        &trial_id,
        &output,
        &TrialMeta {
            task: fixture.spec.task.clone(),
            rubric: fixture.spec.rubric.clone(),
        },
    );

    // Preserve the run's artifacts before deleting it. `run-log.jsonl` = per-round
    // tokens/cost/latency trace; `transcript.txt` = the assembled prompt each round
    // (written by the agent loop under FLOWS_TRANSCRIPT, set by cmd_run).
    let run_dir = store.run_dir(&run_id);
    let log = flows_core::llm_log::log_path(&run_dir);
    eval_store::save_run_log(flows_dir, &trial_id, &log);
    let run_transcript =
        std::fs::read_to_string(run_dir.join("transcript.txt")).unwrap_or_default();
    if !run_transcript.is_empty() {
        let _ = eval_store::save_transcript(flows_dir, &trial_id, &run_transcript);
    }
    let usage = flows_core::usage::parse_usage(&log, &run_id, "");
    let _ = store.delete_run(&run_id); // don't pollute the user's run history

    let mut out = rec(
        verify_pass,
        usage.total_input(),
        usage.total_output(),
        usage.peak_input_tokens,
        usage.phases.iter().map(|p| p.rounds).sum(),
        run_note,
    );
    out.cost = usage.total_cost();
    out
}

/// A turn coder passes when the completion contains every `expect` substring.
fn expect_match(content: &str, expect: &[String]) -> bool {
    !expect.is_empty() && expect.iter().all(|e| content.contains(e.as_str()))
}

/// Run one `turn` trial: a single prompt → single completion, no tool loop, no
/// repo. Coder → auto-graded by `expect`; reasoner → graded manually later.
fn run_turn_trial(
    rt: &tokio::runtime::Runtime,
    fixture: &Fixture,
    model_label: &str,
    model_llm: &LLMConfig,
    config_name: &str,
    flows_dir: &Path,
) -> TrialRecord {
    let trial_id = uuid::Uuid::new_v4().to_string();
    let started = Instant::now();

    let provider = flows_runner::default_provider_factory(model_llm);
    let req = CompletionRequest {
        model: model_llm.model.clone(),
        system: fixture.spec.system.clone().unwrap_or_default(),
        messages: vec![Message::user(fixture.spec.task.clone())],
        tools: vec![],
        max_tokens: fixture.spec.max_tokens.unwrap_or(16384),
        thinking_budget: if model_llm.thinking == Some(true) {
            model_llm.thinking_budget_tokens
        } else {
            None
        },
    };

    let timed = rt.block_on(async {
        tokio::time::timeout(
            Duration::from_secs(fixture.spec.timeout_secs),
            provider.complete(&req),
        )
        .await
    });
    let (content, in_tok, out_tok, note) = match timed {
        Ok(Ok(c)) => (c.content, c.usage.input_tokens, c.usage.output_tokens, None),
        Ok(Err(e)) => (String::new(), 0, 0, Some(format!("llm: {e}"))),
        Err(_) => (String::new(), 0, 0, Some("timeout".into())),
    };

    // Coder turn: auto-grade by `expect`. Reasoner turn: manual (None now).
    let verify_pass = match fixture.spec.suite {
        Suite::Coder => Some(expect_match(&content, &fixture.spec.expect)),
        Suite::Reasoner => None,
    };

    let _ = eval_store::save_output(
        flows_dir,
        &trial_id,
        &content,
        &TrialMeta {
            task: fixture.spec.task.clone(),
            rubric: fixture.spec.rubric.clone(),
        },
    );
    // Full transcript: the exact input context the model received + its response.
    let transcript = format!(
        "=== SYSTEM ===\n{}\n\n=== USER ===\n{}\n\n=== RESPONSE ({}, {in_tok} in / {out_tok} out tok) ===\n{}\n",
        if req.system.is_empty() { "(none)" } else { &req.system },
        fixture.spec.task,
        model_llm.model,
        if content.is_empty() { "(no response)" } else { &content },
    );
    let _ = eval_store::save_transcript(flows_dir, &trial_id, &transcript);

    TrialRecord {
        trial: trial_id,
        ts: eval_store::now_secs(),
        model: model_label.to_string(),
        config: config_name.to_string(),
        fixture: fixture.name.clone(),
        suite: fixture.spec.suite.as_str().to_string(),
        size: fixture.spec.size.as_str().to_string(),
        granularity: Granularity::Turn.as_str().to_string(),
        verify_pass,
        manual_pass: None,
        manual_score: None,
        ratings: None,
        note,
        in_tok,
        out_tok,
        peak_ctx: in_tok, // single turn: peak input = the one request
        cost: 0.0,
        rounds: 1,
        secs: started.elapsed().as_secs_f64(),
    }
}

// ---------------------------------------------------------------------------
// `eval run`
// ---------------------------------------------------------------------------

#[allow(clippy::too_many_arguments)]
pub fn cmd_run(
    models: Option<&str>,
    configs: Option<&str>,
    suite: &str,
    size: &str,
    granularity: &str,
    fixtures_dir: Option<&str>,
    trials: usize,
) -> Result<(), String> {
    let suite_f = Suite::parse_filter(suite)?;
    let size_f = Size::parse_filter(size)?;
    let gran_f = Granularity::parse_filter(granularity)?;
    let trials = trials.max(1);

    let fixtures = load_fixtures(fixtures_dir)?;
    let selected = filtered(&fixtures, suite_f, size_f, gran_f);
    if selected.is_empty() {
        return Err("no fixtures match the suite/size/granularity filter".into());
    }

    // Prepare each fixture. `turn` runs no pipeline (None phases). Others resolve
    // + validate their phases once (granularity check: `phase` = one agent phase).
    // Invalid fixtures are dropped with a message rather than aborting the run.
    let mut prepared: Vec<(&Fixture, Option<Vec<PhaseConfig>>)> = Vec::new();
    for f in &selected {
        if f.spec.granularity == Granularity::Turn {
            prepared.push((f, None));
            continue;
        }
        match resolve_fixture_phases(&f.spec.template).and_then(|ph| {
            validate_granularity(&f.name, f.spec.granularity, &f.spec.template, &ph).map(|_| ph)
        }) {
            Ok(ph) => prepared.push((f, Some(ph))),
            Err(e) => eprintln!("  skip fixture `{}`: {e}", f.name),
        }
    }
    if prepared.is_empty() {
        return Err("no runnable fixtures (all failed phase/granularity validation)".into());
    }

    // Config axis. `--configs` is the run default; a fixture that declares its
    // own `configs:` overrides it (its own on/off matrix). Resolve one axis per
    // prepared fixture, validating every config token up front.
    let default_config_names: Vec<String> =
        split_csv(configs).unwrap_or_else(|| vec!["baseline".into()]);
    let mut fixture_axes: Vec<Vec<(String, ContextConfig)>> = Vec::with_capacity(prepared.len());
    for (f, _) in &prepared {
        let names = if f.spec.configs.is_empty() {
            &default_config_names
        } else {
            &f.spec.configs
        };
        let mut axis = Vec::with_capacity(names.len());
        for name in names {
            let ctx =
                parse_context_config(name).map_err(|e| format!("fixture `{}`: {e}", f.name))?;
            axis.push((name.clone(), ctx));
        }
        fixture_axes.push(axis);
    }

    // Model axis, resolved against the user's real config.
    let cwd = std::env::current_dir().map_err(|e| e.to_string())?;
    let (_, uglobal, uproject, _) = super::resolve(&cwd, None)?;
    let base = merge_llm(&uglobal, &uproject);
    let profiles = merge_profiles(&uglobal, &uproject);

    // Each model token is `model`, `model:suite`, or a bare suite keyword. Suite
    // restriction (which fixtures a model runs) resolves as: explicit CLI `:suite`
    // wins; else the profile's `suites:` tag; else no restriction. So
    // `models: { qwen3-coder: { suites: [coder] } }` auto-pairs, and
    // `--models qwen3-coder:reasoner` overrides for one run. A bare `coder`/
    // `reasoner` token that is NOT a profile name expands to every configured
    // profile tagged with that suite (`--models coder` = all coder models).
    let model_tokens: Vec<String> = split_csv(models).unwrap_or_else(|| vec![base.model.clone()]);
    let mut model_cells: Vec<(String, LLMConfig, Option<Vec<Suite>>)> = Vec::new();
    type Cells = Vec<(String, LLMConfig, Option<Vec<Suite>>)>;
    let push_cell = |mtok: String, cli_suite: Option<Suite>, cells: &mut Cells| {
        let eff = resolve_llm(&mtok, &base, &profiles);
        // CLI `:suite` overrides the profile tag; else use the profile's `suites`.
        let allowed: Option<Vec<Suite>> = if let Some(su) = cli_suite {
            Some(vec![su])
        } else {
            let tagged: Vec<Suite> = eff
                .suites
                .iter()
                .filter_map(|s| Suite::parse_filter(s).ok().flatten())
                .collect();
            (!tagged.is_empty()).then_some(tagged)
        };
        match super::preflight_llm(&eff) {
            Ok(()) => cells.push((mtok, eff, allowed)),
            Err(e) => eprintln!("  skip model `{mtok}`: {e}"),
        }
    };
    for token in &model_tokens {
        // `model:suite` — explicit model restricted to a suite.
        if let Some((m, s)) = token.split_once(':') {
            let su = Suite::parse_filter(s.trim())?
                .ok_or_else(|| format!("model `{token}`: suite must be coder or reasoner"))?;
            push_cell(m.trim().to_string(), Some(su), &mut model_cells);
            continue;
        }
        let tok = token.trim();
        // A bare suite keyword that is NOT also a profile name expands to every
        // profile tagged with that suite.
        if !profiles.contains_key(tok) {
            if let Ok(Some(su)) = Suite::parse_filter(tok) {
                let mut names: Vec<String> = profiles
                    .iter()
                    .filter(|(_, cfg)| {
                        cfg.suites
                            .iter()
                            .any(|s| Suite::parse_filter(s).ok().flatten() == Some(su))
                    })
                    .map(|(name, _)| name.clone())
                    .collect();
                if names.is_empty() {
                    return Err(format!(
                        "--models `{tok}`: no configured model is tagged suites: [{tok}]"
                    ));
                }
                names.sort();
                for name in names {
                    push_cell(name, None, &mut model_cells);
                }
                continue;
            }
        }
        // Plain profile name or literal model id.
        push_cell(tok.to_string(), None, &mut model_cells);
    }
    if model_cells.is_empty() {
        return Err("no runnable models (all failed preflight)".into());
    }

    let flows_dir = super::flows_dir();
    let store = Store::new(&flows_dir);
    let rt = tokio::runtime::Runtime::new().map_err(|e| e.to_string())?;

    // Capture the assembled prompt each round (phase/flow) for review. Scoped to
    // eval — the agent loop only writes a transcript when this is set.
    std::env::set_var("FLOWS_TRANSCRIPT", "1");

    // Total accounts for per-model suite restrictions and per-fixture config axes.
    let total: usize = model_cells
        .iter()
        .map(|(_, _, allowed)| {
            prepared
                .iter()
                .zip(&fixture_axes)
                .filter(|((f, _), _)| allowed.as_ref().map_or(true, |v| v.contains(&f.spec.suite)))
                .map(|(_, axis)| axis.len())
                .sum::<usize>()
        })
        .sum::<usize>()
        * trials;
    // Guard the common mismatch: fixtures survive their suite filter but every
    // model is suite-restricted away from them (e.g. `--suite reasoner` with a
    // coder-tagged default model), yielding a silent 0-run pass.
    if total == 0 {
        use std::collections::BTreeSet;
        let want: BTreeSet<&str> = prepared
            .iter()
            .map(|(f, _)| f.spec.suite.as_str())
            .collect();
        let have: BTreeSet<&str> = model_cells
            .iter()
            .flat_map(|(_, _, allowed)| match allowed {
                Some(v) => v.iter().map(|s| s.as_str()).collect::<Vec<_>>(),
                None => vec!["<any>"],
            })
            .collect();
        let join = |s: &BTreeSet<&str>| s.iter().copied().collect::<Vec<_>>().join(", ");
        let suite = want.iter().next().copied().unwrap_or("reasoner");
        return Err(format!(
            "0 runs: the selected fixtures need suite(s) [{}] but the model(s) cover [{}]. \
             Name a model tagged for that suite, e.g. `--models <model>` or `--models <model>:{suite}`.",
            join(&want),
            join(&have),
        ));
    }
    println!(
        "Evaluating {} model(s) × {} fixture(s) (per-fixture config axis) × {trials} trial(s) = {total} run(s)\n",
        model_cells.len(),
        prepared.len(),
    );

    for (label, llm, allowed) in &model_cells {
        for ((f, phases), axis) in prepared.iter().zip(&fixture_axes) {
            // A model restricted to suites skips fixtures outside them.
            if let Some(v) = allowed {
                if !v.contains(&f.spec.suite) {
                    continue;
                }
            }
            for (cfg_name, ctx) in axis {
                for i in 0..trials {
                    eprint!(
                        "  {label} | {cfg_name} | {} trial {}/{trials}… ",
                        f.name,
                        i + 1
                    );
                    let t = match phases {
                        Some(ph) => run_trial(
                            &rt, f, ph, label, llm, cfg_name, ctx, &uglobal, &uproject, &store,
                            &flows_dir,
                        ),
                        None => run_turn_trial(&rt, f, label, llm, cfg_name, &flows_dir),
                    };
                    let sig = match t.verify_pass {
                        Some(true) => "pass",
                        Some(false) => "FAIL",
                        None => "n/a", // reasoner → graded later
                    };
                    eprintln!(
                        "{sig} ({:.0}s{})",
                        t.secs,
                        t.note
                            .as_deref()
                            .map(|n| format!(", {n}"))
                            .unwrap_or_default()
                    );
                    eval_store::append(&flows_dir, &t)?;
                }
            }
        }
    }

    println!(
        "\nStored to {}. Run `flows eval report` to compare.",
        eval_store::store_dir(&flows_dir).display()
    );
    if prepared
        .iter()
        .any(|(f, _)| f.spec.suite == Suite::Reasoner)
    {
        println!("Reasoner trials need grading: `flows eval review`.");
    }
    Ok(())
}

fn split_csv(s: Option<&str>) -> Option<Vec<String>> {
    let s = s?;
    let v: Vec<String> = s
        .split(',')
        .map(|t| t.trim().to_string())
        .filter(|t| !t.is_empty())
        .collect();
    if v.is_empty() {
        None
    } else {
        Some(v)
    }
}

// ---------------------------------------------------------------------------
// `eval list`
// ---------------------------------------------------------------------------

pub fn cmd_list(
    fixtures_dir: Option<&str>,
    suite: &str,
    size: &str,
    granularity: &str,
) -> Result<(), String> {
    let suite_f = Suite::parse_filter(suite)?;
    let size_f = Size::parse_filter(size)?;
    let gran_f = Granularity::parse_filter(granularity)?;
    let fixtures = load_fixtures(fixtures_dir)?;
    let selected = filtered(&fixtures, suite_f, size_f, gran_f);
    println!("{} fixture(s):", selected.len());
    for f in &selected {
        // Show only the first line of the task, clipped, so multi-line tasks
        // don't wreck the listing.
        let summary = f
            .spec
            .task
            .lines()
            .find(|l| !l.trim().is_empty())
            .unwrap_or("");
        println!(
            "  {:<20} [{:<8} {:<6} {:<5}] {}",
            f.name,
            f.spec.suite.as_str(),
            f.spec.size.as_str(),
            f.spec.granularity.as_str(),
            trunc(summary.trim(), 72)
        );
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// `eval review`
// ---------------------------------------------------------------------------

pub fn cmd_review(suite: &str, model: Option<&str>) -> Result<(), String> {
    let suite_f = Suite::parse_filter(suite)?;
    let flows_dir = super::flows_dir();
    let records = eval_store::load(&flows_dir);
    let pending: Vec<TrialRecord> = records
        .into_iter()
        .filter(|r| r.manual_pass.is_none())
        .filter(|r| suite_f.map_or(true, |s| r.suite == s.as_str()))
        .filter(|r| model.map_or(true, |m| r.model == m))
        .collect();

    if pending.is_empty() {
        println!("No ungraded trials.");
        return Ok(());
    }

    // Interactive terminal → full-screen review TUI (transcript stays visible
    // while rating). Otherwise fall back to the line-based prompt flow below.
    use std::io::IsTerminal;
    if std::io::stdout().is_terminal() && std::io::stdin().is_terminal() {
        let items: Vec<crate::eval_tui::ReviewItem> = pending
            .into_iter()
            .map(|r| {
                let transcript = eval_store::read_transcript(&flows_dir, &r.trial);
                let output = eval_store::read_output(&flows_dir, &r.trial);
                let meta = eval_store::read_meta(&flows_dir, &r.trial);
                let content = if !transcript.is_empty() {
                    transcript
                } else if !output.is_empty() {
                    output
                } else {
                    "(no output)".to_string()
                };
                let is_turn = r.granularity == Granularity::Turn.as_str();
                let (task, body) = match meta {
                    Some(m) => {
                        let mut b = String::new();
                        if let Some(rub) = &m.rubric {
                            b.push_str(&format!("--- rubric ---\n{rub}\n\n"));
                        }
                        b.push_str(&content);
                        (m.task, b)
                    }
                    None => (String::new(), content),
                };
                crate::eval_tui::ReviewItem {
                    record: r,
                    transcript: body,
                    task,
                    is_turn,
                }
            })
            .collect();
        let n = crate::eval_tui::run_review(&flows_dir, items).map_err(|e| e.to_string())?;
        println!("graded {n} trial(s).");
        return Ok(());
    }

    println!(
        "{} ungraded trial(s). Grade each dimension 1-5.\n",
        pending.len()
    );

    for mut r in pending {
        let meta = eval_store::read_meta(&flows_dir, &r.trial);
        let output = eval_store::read_output(&flows_dir, &r.trial);
        let transcript = eval_store::read_transcript(&flows_dir, &r.trial);

        // Build the whole display (header + task/rubric + the full transcript) and
        // show it in a scrollable pager so long phase/flow prompts are reviewable.
        // Turn trials carry a full transcript (input + response); phase/flow whose
        // prompt was captured also do — otherwise fall back to the produced output.
        let mut body = String::new();
        body.push_str(&format!(
            "fixture: {}  [{} {} {}]\nmodel:   {}   config: {}\n",
            r.fixture, r.suite, r.size, r.granularity, r.model, r.config
        ));
        if let Some(m) = &meta {
            body.push_str(&format!("\ntask: {}\n", m.task));
            if let Some(rubric) = &m.rubric {
                body.push_str(&format!("\nrubric:\n{rubric}\n"));
            }
        }
        let content = if !transcript.is_empty() {
            &transcript
        } else if !output.is_empty() {
            &output
        } else {
            "(no output)"
        };
        body.push_str(&format!(
            "\n--- transcript (input + response) ---\n{content}\n"
        ));
        show_scrollable(&body);

        // Rate every dimension 1-5 (no skipping). tool_use is n/a for a turn
        // trial (no tools). Pass is derived from correctness (>= 3).
        let is_turn = r.granularity == Granularity::Turn.as_str();
        println!("\nrate each 1-5 (5 = best):");
        let ratings = Ratings {
            correctness: prompt_rating("correctness (output actually right?)"),
            efficiency: prompt_rating("efficiency (no wasted rounds/tokens?)"),
            tool_use: if is_turn {
                None
            } else {
                Some(prompt_rating(
                    "tool use (right tools, valid args, recovers?)",
                ))
            },
            grounding: prompt_rating("grounding (used real context, no hallucination?)"),
            instruction: prompt_rating("instruction following (constraints + format?)"),
        };
        let note = prompt("note (optional, enter to skip)> ")
            .trim()
            .to_string();

        let pass = ratings.correctness >= 3;
        r.manual_pass = Some(pass);
        r.manual_score = Some(ratings.mean().round() as u8);
        r.ratings = Some(ratings);
        if !note.is_empty() {
            r.note = Some(note);
        }
        r.ts = eval_store::now_secs();
        eval_store::append(&flows_dir, &r)?;
        println!(
            "recorded: {} (mean {:.1})\n",
            if pass { "pass" } else { "fail" },
            ratings.mean()
        );
    }
    Ok(())
}

fn prompt(msg: &str) -> String {
    use std::io::Write;
    print!("{msg}");
    let _ = std::io::stdout().flush();
    let mut line = String::new();
    let _ = std::io::stdin().read_line(&mut line);
    line
}

/// Show text in a scrollable pager (`$PAGER`, else `less -RFX`) so long
/// transcripts can be paged. Falls back to a plain print when stdout is not a
/// terminal (piped/CI) or no pager is available. The pager reads keys from the
/// tty, so the rating prompts on stdin still work after it exits.
fn show_scrollable(content: &str) {
    use std::io::{IsTerminal, Write};
    if !std::io::stdout().is_terminal() {
        println!("{content}");
        return;
    }
    // `-R` raw colors, `-F` skip the pager when it all fits one screen. (No `-X`:
    // it suppresses the terminal init less needs, which breaks arrow/wheel scroll.)
    let pager = std::env::var("PAGER").unwrap_or_else(|_| "less -R -F".to_string());
    let child = std::process::Command::new("sh")
        .arg("-c")
        .arg(&pager)
        .stdin(std::process::Stdio::piped())
        .spawn();
    match child {
        Ok(mut c) => {
            if let Some(mut stdin) = c.stdin.take() {
                let _ = stdin.write_all(content.as_bytes());
                // stdin drops here → EOF to the pager.
            }
            let _ = c.wait();
        }
        Err(_) => println!("{content}"),
    }
}

/// Prompt for a 1-5 rating, re-asking until a valid number is entered (no skip).
fn prompt_rating(label: &str) -> u8 {
    loop {
        let raw = prompt(&format!("  {label} [1-5]> "));
        match raw.trim().parse::<u8>() {
            Ok(n) if (1..=5).contains(&n) => return n,
            _ => println!("  enter a number 1-5."),
        }
    }
}

// ---------------------------------------------------------------------------
// `eval report`
// ---------------------------------------------------------------------------

/// A trial's success per its suite: coder → verifier, reasoner → manual grade.
/// `None` = ungraded (reasoner without a grade) and is excluded from pass rate.
fn trial_success(r: &TrialRecord) -> Option<bool> {
    match r.suite.as_str() {
        "reasoner" => r.manual_pass,
        _ => r.verify_pass,
    }
}

#[derive(Default)]
struct GroupStat {
    n: usize,
    graded: usize,
    passes: usize,
    in_tok: Vec<f64>,
    peak: Vec<f64>,
    cost: Vec<f64>,
    rounds: Vec<f64>,
    secs: Vec<f64>,
    // Per-dimension manual ratings (empty until trials are reviewed).
    correctness: Vec<f64>,
    efficiency: Vec<f64>,
    tool_use: Vec<f64>,
    grounding: Vec<f64>,
    instruction: Vec<f64>,
}

impl GroupStat {
    /// Compact `C/E/T/G/I` rating means (`-` for an unrated dimension).
    fn ratings_cell(&self) -> String {
        let d = |v: &[f64]| {
            if v.is_empty() {
                "-".to_string()
            } else {
                format!("{:.1}", mean(v))
            }
        };
        format!(
            "{}/{}/{}/{}/{}",
            d(&self.correctness),
            d(&self.efficiency),
            d(&self.tool_use),
            d(&self.grounding),
            d(&self.instruction),
        )
    }
}

fn mean(v: &[f64]) -> f64 {
    if v.is_empty() {
        0.0
    } else {
        v.iter().sum::<f64>() / v.len() as f64
    }
}
fn stddev(v: &[f64]) -> f64 {
    if v.len() < 2 {
        return 0.0;
    }
    let m = mean(v);
    (v.iter().map(|x| (x - m).powi(2)).sum::<f64>() / (v.len() - 1) as f64).sqrt()
}

type GroupKey = (String, String, String, String, String);

/// Aggregate records into groups keyed by (model, config, suite, size,
/// granularity). Granularity is part of the key so levels never mix.
fn aggregate(records: &[TrialRecord]) -> Vec<(GroupKey, GroupStat)> {
    let mut map: HashMap<GroupKey, GroupStat> = HashMap::new();
    for r in records {
        let gran = if r.granularity.is_empty() {
            "flow".to_string() // older records predate the field
        } else {
            r.granularity.clone()
        };
        let key = (
            r.model.clone(),
            r.config.clone(),
            r.suite.clone(),
            r.size.clone(),
            gran,
        );
        let g = map.entry(key).or_default();
        g.n += 1;
        if let Some(ok) = trial_success(r) {
            g.graded += 1;
            if ok {
                g.passes += 1;
            }
        }
        g.in_tok.push(r.in_tok as f64);
        g.peak.push(r.peak_ctx as f64);
        g.cost.push(r.cost);
        g.rounds.push(r.rounds as f64);
        g.secs.push(r.secs);
        if let Some(rt) = &r.ratings {
            g.correctness.push(rt.correctness as f64);
            g.efficiency.push(rt.efficiency as f64);
            g.grounding.push(rt.grounding as f64);
            g.instruction.push(rt.instruction as f64);
            if let Some(t) = rt.tool_use {
                g.tool_use.push(t as f64);
            }
        }
    }
    let mut rows: Vec<_> = map.into_iter().collect();
    rows.sort_by(|a, b| a.0.cmp(&b.0));
    rows
}

fn ktok(n: f64) -> String {
    if n >= 1000.0 {
        format!("{:.1}k", n / 1000.0)
    } else {
        format!("{n:.0}")
    }
}

pub fn cmd_report(
    model: Option<&str>,
    config: Option<&str>,
    suite: &str,
    granularity: &str,
) -> Result<(), String> {
    let suite_f = Suite::parse_filter(suite)?;
    let gran_f = Granularity::parse_filter(granularity)?;
    let flows_dir = super::flows_dir();
    let records: Vec<TrialRecord> = eval_store::load(&flows_dir)
        .into_iter()
        .filter(|r| model.map_or(true, |m| r.model == m))
        .filter(|r| config.map_or(true, |c| r.config == c))
        .filter(|r| suite_f.map_or(true, |s| r.suite == s.as_str()))
        .filter(|r| gran_f.map_or(true, |g| r.granularity == g.as_str()))
        .collect();
    if records.is_empty() {
        println!("No results in the store (run `flows eval run` first).");
        return Ok(());
    }

    let rows = aggregate(&records);
    println!(
        "{:<16} {:<10} {:<8} {:<6} {:<6} {:>9} {:>9} {:>13} {:>8} {:>7} {:>3}  {:<13}",
        "model",
        "config",
        "suite",
        "size",
        "gran",
        "pass",
        "in_tok",
        "peak_ctx",
        "cost",
        "rounds",
        "n",
        "ratings C/E/T/G/I"
    );
    for ((m, c, su, sz, gr), g) in &rows {
        let pass = if g.graded == 0 {
            "ungraded".to_string()
        } else {
            format!(
                "{:.0}% {}/{}",
                g.passes as f64 / g.graded as f64 * 100.0,
                g.passes,
                g.graded
            )
        };
        println!(
            "{:<16} {:<10} {:<8} {:<6} {:<6} {:>9} {:>9} {:>13} {:>8} {:>7.1} {:>3}  {:<13}",
            trunc(m, 16),
            trunc(c, 10),
            su,
            sz,
            gr,
            pass,
            ktok(mean(&g.in_tok)),
            format!("{}±{}", ktok(mean(&g.peak)), ktok(stddev(&g.peak))),
            flows_core::pricing::format_cost(mean(&g.cost)),
            mean(&g.rounds),
            g.n,
            g.ratings_cell(),
        );
    }
    println!("\n(ratings: Correctness / Efficiency / Tool-use / Grounding / Instruction, 1-5 means; `-` = unrated)");
    Ok(())
}

// ---------------------------------------------------------------------------
// `eval clean`
// ---------------------------------------------------------------------------

pub fn cmd_clean(
    model: Option<&str>,
    fixture: Option<&str>,
    suite: &str,
    granularity: &str,
    yes: bool,
) -> Result<(), String> {
    let suite_f = Suite::parse_filter(suite)?;
    let gran_f = Granularity::parse_filter(granularity)?;
    let flows_dir = super::flows_dir();

    let records = eval_store::load(&flows_dir);
    if records.is_empty() {
        println!("Eval store is empty.");
        return Ok(());
    }

    let no_filter = model.is_none() && fixture.is_none() && suite_f.is_none() && gran_f.is_none();
    let matching: Vec<&TrialRecord> = records
        .iter()
        .filter(|r| model.map_or(true, |m| r.model == m))
        .filter(|r| fixture.map_or(true, |fx| r.fixture == fx))
        .filter(|r| suite_f.map_or(true, |s| r.suite == s.as_str()))
        .filter(|r| gran_f.map_or(true, |g| r.granularity == g.as_str()))
        .collect();
    if matching.is_empty() {
        println!("No trials match the filter.");
        return Ok(());
    }

    if !yes {
        let what = if no_filter {
            format!("ALL {} trial(s) + saved outputs", matching.len())
        } else {
            format!("{} matching trial(s)", matching.len())
        };
        let ans = prompt(&format!("Delete {what} from the eval store? [y/N] "));
        if !ans.trim().eq_ignore_ascii_case("y") {
            println!("aborted.");
            return Ok(());
        }
    }

    if no_filter {
        eval_store::wipe(&flows_dir)?;
        println!("Wiped the eval store.");
    } else {
        let ids: HashSet<String> = matching.iter().map(|r| r.trial.clone()).collect();
        let n = eval_store::remove_trials(&flows_dir, &ids)?;
        println!("Removed {n} trial(s).");
    }
    Ok(())
}

fn trunc(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        s.to_string()
    } else {
        format!("{}…", s.chars().take(n - 1).collect::<String>())
    }
}

// ---------------------------------------------------------------------------
// Tests (structural — no live model)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn oracle_blob_reads_relevant_files_skips_symbols_and_missing() {
        let d = tempfile::tempdir().unwrap();
        std::fs::write(d.path().join("a.rs"), "fn a() {}\n").unwrap();
        let spec: FixtureSpec = serde_yaml_ng::from_str(
            "task: t\nsuite: reasoner\ngranularity: phase\nrubric: r\n\
             relevant:\n  - a.rs\n  - some_symbol_name\n  - missing.rs\n",
        )
        .unwrap();
        let fx = Fixture {
            name: "f".into(),
            spec,
            repo_files: vec![],
        };
        let blob = oracle_blob(&fx, d.path());
        assert!(blob.contains("### a.rs") && blob.contains("fn a() {}"));
        // Symbol label + missing file contribute nothing.
        assert!(!blob.contains("some_symbol_name"));
        assert!(!blob.contains("missing.rs"));
    }

    fn mkrec(suite: &str, verify: Option<bool>, manual: Option<bool>, peak: u64) -> TrialRecord {
        TrialRecord {
            trial: uuid::Uuid::new_v4().to_string(),
            ts: 0,
            model: "qwen".into(),
            config: "baseline".into(),
            fixture: "f".into(),
            suite: suite.into(),
            size: "small".into(),
            granularity: "flow".into(),
            verify_pass: verify,
            manual_pass: manual,
            manual_score: None,
            ratings: None,
            note: None,
            in_tok: 100,
            out_tok: 10,
            peak_ctx: peak,
            cost: 0.0,
            rounds: 2,
            secs: 1.0,
        }
    }

    #[test]
    fn embedded_starters_load_and_validate() {
        let fx = load_embedded().unwrap();
        assert!(fx.iter().any(|f| f.name == "hello-file"));
        assert!(fx
            .iter()
            .all(|f| validate_fixture(&f.name, &f.spec).is_ok()));
    }

    #[test]
    fn coder_needs_verify_reasoner_needs_rubric() {
        let coder = FixtureSpec {
            task: "t".into(),
            template: "blank".into(),
            suite: Suite::Coder,
            size: Size::Small,
            granularity: Granularity::Flow,
            verify: None,
            expect: vec![],
            system: None,
            max_tokens: None,
            rubric: None,
            relevant: vec![],
            configs: vec![],
            timeout_secs: 1,
        };
        assert!(validate_fixture("c", &coder).is_err());
        let reasoner = FixtureSpec {
            suite: Suite::Reasoner,
            verify: Some("true".into()),
            rubric: None,
            ..coder
        };
        assert!(validate_fixture("r", &reasoner).is_err());
    }

    #[test]
    fn turn_coder_needs_expect_and_scores_by_substring() {
        // coder turn with no expect → invalid.
        let mut t = FixtureSpec {
            task: "write is_prime".into(),
            template: "blank".into(),
            suite: Suite::Coder,
            size: Size::Small,
            granularity: Granularity::Turn,
            verify: None,
            expect: vec![],
            system: None,
            max_tokens: None,
            rubric: None,
            relevant: vec![],
            configs: vec![],
            timeout_secs: 1,
        };
        assert!(validate_fixture("t", &t).is_err());
        // with expect → valid (no verify needed for a turn).
        t.expect = vec!["fn is_prime".into()];
        assert!(validate_fixture("t", &t).is_ok());
        // substring scoring: all must be present.
        assert!(expect_match(
            "pub fn is_prime(n: u64) -> bool { true }",
            &t.expect
        ));
        assert!(!expect_match("nope", &t.expect));
        assert!(!expect_match("anything", &[])); // empty expect never passes
    }

    fn agent(name: &str) -> PhaseConfig {
        PhaseConfig::new(name)
    }

    #[test]
    fn granularity_phase_requires_single_agent_phase() {
        let one = vec![agent("run")];
        let two = vec![agent("plan"), agent("implement")];
        // phase → ok with one agent phase, err with two.
        assert!(validate_granularity("f", Granularity::Phase, "blank", &one).is_ok());
        assert!(validate_granularity("f", Granularity::Phase, "feature", &two).is_err());
        // flow → any count ok.
        assert!(validate_granularity("f", Granularity::Flow, "feature", &two).is_ok());
        // turn → runs no pipeline, so the phase-count check does not apply.
        assert!(validate_granularity("f", Granularity::Turn, "blank", &one).is_ok());
    }

    #[test]
    fn fixture_declares_its_own_config_axis() {
        // A fixture can carry a `configs:` on/off matrix; absent → empty (run
        // default). Each token must parse via `parse_context_config`.
        let yml = "task: t\nsuite: reasoner\ngranularity: phase\nrubric: r\n\
                   configs:\n  - baseline\n  - all\n";
        let spec: FixtureSpec = serde_yaml_ng::from_str(yml).unwrap();
        assert_eq!(
            spec.configs,
            vec!["baseline".to_string(), "all".to_string()]
        );
        for name in &spec.configs {
            assert!(parse_context_config(name).is_ok());
        }
        // Omitted → empty, so the fixture falls back to the run's `--configs`.
        let spec: FixtureSpec =
            serde_yaml_ng::from_str("task: t\nsuite: coder\nverify: 'true'\n").unwrap();
        assert!(spec.configs.is_empty());
    }

    #[test]
    fn unknown_suite_size_rejected() {
        assert!(Suite::parse_filter("bogus").is_err());
        assert!(Size::parse_filter("huge").is_err());
        assert!(Suite::parse_filter("all").unwrap().is_none());
        assert!(Granularity::parse_filter("bogus").is_err());
        assert_eq!(
            Granularity::parse_filter("phase").unwrap(),
            Some(Granularity::Phase)
        );
    }

    #[test]
    fn success_source_by_suite() {
        // coder → verifier decides; manual ignored.
        assert_eq!(
            trial_success(&mkrec("coder", Some(true), None, 1)),
            Some(true)
        );
        // reasoner → manual decides; verifier ignored; ungraded = None.
        assert_eq!(trial_success(&mkrec("reasoner", Some(true), None, 1)), None);
        assert_eq!(
            trial_success(&mkrec("reasoner", None, Some(false), 1)),
            Some(false)
        );
    }

    #[test]
    fn aggregate_means_and_spread() {
        let recs = vec![
            mkrec("coder", Some(true), None, 100),
            mkrec("coder", Some(false), None, 300),
        ];
        let rows = aggregate(&recs);
        assert_eq!(rows.len(), 1);
        let g = &rows[0].1;
        assert_eq!(g.n, 2);
        assert_eq!(g.graded, 2);
        assert_eq!(g.passes, 1);
        assert_eq!(mean(&g.peak), 200.0);
        assert!(stddev(&g.peak) > 0.0); // spread visible across trials
    }
}
