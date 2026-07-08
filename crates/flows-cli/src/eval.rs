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

use std::collections::HashMap;
use std::path::Path;
use std::process::Command;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use include_dir::{include_dir, Dir};
use serde::Deserialize;

use flows_core::config::{merge_llm, merge_profiles, parse_context_config, resolve_llm};
use flows_core::models::{ContextConfig, GlobalConfig, LLMConfig, PhaseConfig, ProjectConfig};
use flows_core::state::Store;
use flows_runner::runner::{run_pipeline, RunnerCallbacks};

use crate::eval_store::{self, TrialMeta, TrialRecord};

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

#[derive(Debug, Deserialize)]
struct FixtureSpec {
    task: String,
    #[serde(default = "default_template")]
    template: String,
    #[serde(default)]
    suite: Suite,
    #[serde(default)]
    size: Size,
    /// Shell command whose exit 0 == success. Required for coder fixtures.
    #[serde(default)]
    verify: Option<String>,
    /// Human-grading criteria shown in `review`. Required for reasoner fixtures.
    #[serde(default)]
    rubric: Option<String>,
    /// Labeled relevant files/symbols (for a future deterministic retrieval eval).
    #[serde(default)]
    #[allow(dead_code)]
    relevant: Vec<String>,
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

/// Reject fixtures whose suite lacks its required success signal.
fn validate_fixture(name: &str, spec: &FixtureSpec) -> Result<(), String> {
    match spec.suite {
        Suite::Coder if spec.verify.as_deref().unwrap_or("").trim().is_empty() => Err(format!(
            "fixture `{name}`: coder suite requires a `verify` command"
        )),
        Suite::Reasoner if spec.rubric.as_deref().unwrap_or("").trim().is_empty() => Err(format!(
            "fixture `{name}`: reasoner suite requires a `rubric`"
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

fn filtered(fx: &[Fixture], suite: Option<Suite>, size: Option<Size>) -> Vec<&Fixture> {
    fx.iter()
        .filter(|f| suite.map_or(true, |s| f.spec.suite == s))
        .filter(|f| size.map_or(true, |s| f.spec.size == s))
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
    p.context = ctx;
    (g, p)
}

#[allow(clippy::too_many_arguments)]
fn run_trial(
    rt: &tokio::runtime::Runtime,
    fixture: &Fixture,
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
        verify_pass,
        manual_pass: None,
        manual_score: None,
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

    // Phases from the fixture's template, with profile routing stripped so the
    // whole pipeline runs on the pinned model.
    let phases: Vec<PhaseConfig> = match super::resolve(work.path(), Some(&fixture.spec.template)) {
        Ok((phases, _, _, _)) => phases
            .into_iter()
            .map(|mut ph| {
                ph.model = None;
                ph
            })
            .collect(),
        Err(e) => return rec(None, 0, 0, 0, 0, Some(format!("template: {e}"))),
    };

    let (global, project) = pin_config(base_global, base_project, model_llm, ctx.clone());

    let run_id = uuid::Uuid::new_v4().to_string();
    let cb = EvalCallbacks;
    let fut = run_pipeline(
        &run_id,
        &fixture.spec.task,
        work.path(),
        &phases,
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
    let output = capture_output(work.path(), store, &run_id, &phases);
    let _ = eval_store::save_output(
        flows_dir,
        &trial_id,
        &output,
        &TrialMeta {
            task: fixture.spec.task.clone(),
            rubric: fixture.spec.rubric.clone(),
        },
    );

    // Metrics from the run's llm.jsonl.
    let log = flows_core::llm_log::log_path(&store.run_dir(&run_id));
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

// ---------------------------------------------------------------------------
// `eval run`
// ---------------------------------------------------------------------------

#[allow(clippy::too_many_arguments)]
pub fn cmd_run(
    models: Option<&str>,
    configs: Option<&str>,
    suite: &str,
    size: &str,
    fixtures_dir: Option<&str>,
    trials: usize,
) -> Result<(), String> {
    let suite_f = Suite::parse_filter(suite)?;
    let size_f = Size::parse_filter(size)?;
    let trials = trials.max(1);

    let fixtures = load_fixtures(fixtures_dir)?;
    let selected = filtered(&fixtures, suite_f, size_f);
    if selected.is_empty() {
        return Err("no fixtures match the suite/size filter".into());
    }

    // Config axis (validate names up front).
    let config_names: Vec<String> = split_csv(configs).unwrap_or_else(|| vec!["baseline".into()]);
    let mut config_ctx: Vec<(String, ContextConfig)> = Vec::new();
    for name in &config_names {
        config_ctx.push((name.clone(), parse_context_config(name)?));
    }

    // Model axis, resolved against the user's real config.
    let cwd = std::env::current_dir().map_err(|e| e.to_string())?;
    let (_, uglobal, uproject, _) = super::resolve(&cwd, None)?;
    let base = merge_llm(&uglobal, &uproject);
    let profiles = merge_profiles(&uglobal, &uproject);

    let model_tokens: Vec<String> = split_csv(models).unwrap_or_else(|| vec![base.model.clone()]);
    let mut model_cells: Vec<(String, LLMConfig)> = Vec::new();
    for token in &model_tokens {
        let eff = resolve_llm(token, &base, &profiles);
        match super::preflight_llm(&eff) {
            Ok(()) => model_cells.push((token.clone(), eff)),
            Err(e) => eprintln!("  skip model `{token}`: {e}"),
        }
    }
    if model_cells.is_empty() {
        return Err("no runnable models (all failed preflight)".into());
    }

    let flows_dir = super::flows_dir();
    let store = Store::new(&flows_dir);
    let rt = tokio::runtime::Runtime::new().map_err(|e| e.to_string())?;

    let total = model_cells.len() * config_ctx.len() * selected.len() * trials;
    println!(
        "Evaluating {} model(s) × {} config(s) × {} fixture(s) × {trials} trial(s) = {total} run(s)\n",
        model_cells.len(),
        config_ctx.len(),
        selected.len()
    );

    for (label, llm) in &model_cells {
        for (cfg_name, ctx) in &config_ctx {
            for f in &selected {
                for i in 0..trials {
                    eprint!(
                        "  {label} | {cfg_name} | {} trial {}/{trials}… ",
                        f.name,
                        i + 1
                    );
                    let t = run_trial(
                        &rt, f, label, llm, cfg_name, ctx, &uglobal, &uproject, &store, &flows_dir,
                    );
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
    if selected.iter().any(|f| f.spec.suite == Suite::Reasoner) {
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

pub fn cmd_list(fixtures_dir: Option<&str>, suite: &str, size: &str) -> Result<(), String> {
    let suite_f = Suite::parse_filter(suite)?;
    let size_f = Size::parse_filter(size)?;
    let fixtures = load_fixtures(fixtures_dir)?;
    let selected = filtered(&fixtures, suite_f, size_f);
    println!("{} fixture(s):", selected.len());
    for f in &selected {
        println!(
            "  {:<20} [{:<8} {:<6}] {}",
            f.name,
            f.spec.suite.as_str(),
            f.spec.size.as_str(),
            f.spec.task
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
    println!(
        "{} ungraded trial(s). Grade each: [p]ass / [f]ail / [s]kip.\n",
        pending.len()
    );

    for mut r in pending {
        let meta = eval_store::read_meta(&flows_dir, &r.trial);
        let output = eval_store::read_output(&flows_dir, &r.trial);
        println!("──────────────────────────────────────────────");
        println!("fixture: {}  [{} {}]", r.fixture, r.suite, r.size);
        println!("model:   {}   config: {}", r.model, r.config);
        if let Some(m) = &meta {
            println!("\ntask: {}", m.task);
            if let Some(rubric) = &m.rubric {
                println!("\nrubric:\n{rubric}");
            }
        }
        println!(
            "\n--- output ---\n{}",
            if output.is_empty() {
                "(no output)"
            } else {
                &output
            }
        );

        let verdict = prompt("\ngrade [p]ass/[f]ail/[s]kip> ");
        let pass = match verdict.trim().chars().next() {
            Some('p') | Some('P') => true,
            Some('f') | Some('F') => false,
            _ => {
                println!("skipped.\n");
                continue;
            }
        };
        let score = prompt("score 0-5 (enter to skip)> ")
            .trim()
            .parse::<u8>()
            .ok()
            .map(|n| n.min(5));
        let note = prompt("note (enter to skip)> ").trim().to_string();

        r.manual_pass = Some(pass);
        r.manual_score = score;
        if !note.is_empty() {
            r.note = Some(note);
        }
        r.ts = eval_store::now_secs();
        eval_store::append(&flows_dir, &r)?;
        println!("recorded: {}\n", if pass { "pass" } else { "fail" });
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

/// Aggregate records into groups keyed by (model, config, suite, size).
fn aggregate(records: &[TrialRecord]) -> Vec<((String, String, String, String), GroupStat)> {
    let mut map: HashMap<(String, String, String, String), GroupStat> = HashMap::new();
    for r in records {
        let key = (
            r.model.clone(),
            r.config.clone(),
            r.suite.clone(),
            r.size.clone(),
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

pub fn cmd_report(model: Option<&str>, config: Option<&str>, suite: &str) -> Result<(), String> {
    let suite_f = Suite::parse_filter(suite)?;
    let flows_dir = super::flows_dir();
    let records: Vec<TrialRecord> = eval_store::load(&flows_dir)
        .into_iter()
        .filter(|r| model.map_or(true, |m| r.model == m))
        .filter(|r| config.map_or(true, |c| r.config == c))
        .filter(|r| suite_f.map_or(true, |s| r.suite == s.as_str()))
        .collect();
    if records.is_empty() {
        println!("No results in the store (run `flows eval run` first).");
        return Ok(());
    }

    let rows = aggregate(&records);
    println!(
        "{:<16} {:<10} {:<8} {:<6} {:>9} {:>9} {:>13} {:>8} {:>7} {:>3}",
        "model", "config", "suite", "size", "pass", "in_tok", "peak_ctx", "cost", "rounds", "n"
    );
    for ((m, c, su, sz), g) in &rows {
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
            "{:<16} {:<10} {:<8} {:<6} {:>9} {:>9} {:>13} {:>8} {:>7.1} {:>3}",
            trunc(m, 16),
            trunc(c, 10),
            su,
            sz,
            pass,
            ktok(mean(&g.in_tok)),
            format!("{}±{}", ktok(mean(&g.peak)), ktok(stddev(&g.peak))),
            flows_core::pricing::format_cost(mean(&g.cost)),
            mean(&g.rounds),
            g.n
        );
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

    fn mkrec(suite: &str, verify: Option<bool>, manual: Option<bool>, peak: u64) -> TrialRecord {
        TrialRecord {
            trial: uuid::Uuid::new_v4().to_string(),
            ts: 0,
            model: "qwen".into(),
            config: "baseline".into(),
            fixture: "f".into(),
            suite: suite.into(),
            size: "small".into(),
            verify_pass: verify,
            manual_pass: manual,
            manual_score: None,
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
            verify: None,
            rubric: None,
            relevant: vec![],
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
    fn unknown_suite_size_rejected() {
        assert!(Suite::parse_filter("bogus").is_err());
        assert!(Size::parse_filter("huge").is_err());
        assert!(Suite::parse_filter("all").unwrap().is_none());
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
