//! Pipeline runner. Port of `run_pipeline` / `_run_linear` / `_run_dag` from
//! `flows/runner.py`. Linear and DAG (parallel via depends_on) execution.

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::{Arc, Mutex as StdMutex};

use async_trait::async_trait;
use futures_util::stream::{FuturesUnordered, StreamExt};
use regex::Regex;
use serde_json::{json, Map, Value};

use flows_core::config::{merge_llm, merge_profiles, resolve_llm};
use flows_core::fan_out::Item;
use flows_core::models::{
    Artifact, FanOutItem, GlobalConfig, LLMConfig, OnFailure, PhaseConfig, PhaseType,
    ProjectConfig, RunMeta, RunState, RunStatus,
};
use flows_core::pipeline::{is_dag, validate_pipeline};
use flows_core::state::Store;
use flows_llm::Provider;

/// Builds (or reuses) a [`Provider`] for a resolved [`LLMConfig`]. Production
/// passes [`default_provider_factory`] (dispatch by model → Anthropic/OpenAI);
/// tests inject a mock. `Arc` so a provider can be shared across phases that
/// resolve to the same config.
pub type ProviderFactory<'a> = &'a (dyn Fn(&LLMConfig) -> Arc<dyn Provider> + Send + Sync);

/// The production provider factory: `flows_llm::make_provider` per resolved
/// config, wrapped in an `Arc` for the runner's per-config cache.
pub fn default_provider_factory(llm: &LLMConfig) -> Arc<dyn Provider> {
    Arc::from(flows_llm::make_provider(llm))
}

use crate::agent::{run_agent_phase, AgentCallbacks};

#[derive(Debug, thiserror::Error)]
pub enum RunError {
    #[error("invalid pipeline: {0}")]
    Validation(String),
    #[error("phase {phase:?} failed: {error}")]
    Failed { phase: String, error: String },
    #[error("{0}")]
    Other(String),
}

/// Full run lifecycle hooks. All non-async hooks have no-op defaults.
#[async_trait]
pub trait RunnerCallbacks: Send + Sync {
    async fn on_questions(&self, phase: &str, questions: &[String]) -> String;
    async fn on_human_gate(&self, phase: &str, instructions: &str) -> String;
    fn on_status(&self, _phase: &str, _message: &str) {}
    fn on_token(&self, _phase: &str, _token: &str) {}
    /// A token belonging to a structured JSON artifact envelope.
    fn on_json(&self, _phase: &str, _token: &str) {}
    fn on_thinking(&self, _phase: &str, _text: &str) {}
    /// The assembled system prompt for a phase (emitted once, at phase start).
    fn on_system(&self, _phase: &str, _prompt: &str) {}
    /// The first user turn (the question) for a phase, emitted once at start.
    fn on_user(&self, _phase: &str, _message: &str) {}
    /// A structural marker line for the output pane (LLM request/response marks).
    fn on_note(&self, _phase: &str, _text: &str) {}
    /// Running total USD cost after a phase completes.
    fn on_cost(&self, _total_usd: f64) {}
    /// Input tokens of the latest request (current context-window usage).
    fn on_context(&self, _input_tokens: u64) {}
    /// Current tool-loop round and the phase's cap (for a `round N/T` readout).
    fn on_round(&self, _round: u32, _total: u32) {}
    fn on_tool_call(&self, _phase: &str, _name: &str, _summary: &str) {}
    fn on_tool_result(&self, _phase: &str, _name: &str, _summary: &str, _is_error: bool) {}
    fn on_phase_start(&self, _phase: &str, _index: usize, _total: usize) {}
    fn on_phase_end(&self, _phase: &str, _artifact: Option<&Artifact>) {}
    /// The phase's `when` condition was false; it did not run. Fired instead of
    /// `on_phase_start` / `on_phase_end`.
    fn on_phase_skipped(&self, _phase: &str, _reason: &str) {}
    fn on_loop_back(&self, _from: &str, _to: &str, _iteration: u32, _max: u32) {}
    /// A `for_each` group expanded into these `(id, title)` items, in run order.
    fn on_fan_out(&self, _group: &str, _items: &[(String, String)]) {}
    /// A fan-out item is starting (`index` is 1-based).
    fn on_fan_out_item(&self, _group: &str, _id: &str, _title: &str, _index: usize, _count: usize) {
    }
    fn on_script_result(&self, _phase: &str, _exit_code: i32, _stdout: &str, _stderr: &str) {}
    fn on_run_complete(&self, _run_id: &str, _completed_phases: &[String]) {}
    fn on_run_failed(&self, _run_id: &str, _phase: &str, _error: &str) {}
}

struct PhaseCb<'a> {
    cb: &'a dyn RunnerCallbacks,
    phase: String,
    store: &'a Store,
    run_id: &'a str,
}

#[async_trait]
impl AgentCallbacks for PhaseCb<'_> {
    async fn on_questions(&self, q: &[String]) -> String {
        self.cb.on_questions(&self.phase, q).await
    }
    fn on_status(&self, m: &str) {
        self.cb.on_status(&self.phase, m);
    }
    fn on_token(&self, t: &str) {
        self.cb.on_token(&self.phase, t);
    }
    fn on_json(&self, t: &str) {
        self.cb.on_json(&self.phase, t);
    }
    fn on_context(&self, input_tokens: u64) {
        self.cb.on_context(input_tokens);
    }
    fn on_round(&self, round: u32, total: u32) {
        self.cb.on_round(round, total);
    }
    fn on_thinking(&self, t: &str) {
        self.cb.on_thinking(&self.phase, t);
    }
    fn on_system(&self, p: &str) {
        self.cb.on_system(&self.phase, p);
    }
    fn on_user(&self, m: &str) {
        self.cb.on_user(&self.phase, m);
    }
    fn on_note(&self, t: &str) {
        self.cb.on_note(&self.phase, t);
    }
    fn on_tool_call(&self, n: &str, s: &str) {
        self.cb.on_tool_call(&self.phase, n, s);
    }
    fn on_tool_result(&self, n: &str, s: &str, e: bool) {
        self.cb.on_tool_result(&self.phase, n, s, e);
    }
    fn cancelled(&self) -> bool {
        self.store.is_cancelled(self.run_id)
    }
}

/// Shared run context. `providers` is a per-resolved-config provider cache
/// (interior mutability so concurrent DAG phases can share it).
struct Ctx<'a> {
    run_id: &'a str,
    idea: &'a str,
    repo_root: &'a Path,
    phases: &'a [PhaseConfig],
    template_name: &'a str,
    global_cfg: &'a GlobalConfig,
    /// Base (merged global+project) LLM config; `llm.model` may itself name a
    /// profile — per-phase resolution happens in [`Ctx::resolve_phase_llm`].
    llm: LLMConfig,
    profiles: HashMap<String, LLMConfig>,
    factory: ProviderFactory<'a>,
    providers: StdMutex<HashMap<String, Arc<dyn Provider>>>,
    callbacks: &'a dyn RunnerCallbacks,
    store: &'a Store,
    flows_dir: Option<&'a Path>,
    logger: flows_core::LlmLogger,
    /// Context-delivery feature flags (search/range/slim-tree/…), from project
    /// config. Threaded into each agent phase.
    context: flows_core::models::ContextConfig,
}

impl Ctx<'_> {
    /// Effective LLM config for a phase (profile/literal resolution + key).
    fn resolve_phase_llm(&self, phase: &PhaseConfig) -> LLMConfig {
        resolve_llm(
            phase.model.as_deref().unwrap_or(""),
            &self.llm,
            &self.profiles,
        )
    }

    /// Provider for a resolved config, built once per distinct
    /// model+endpoint+key and reused across phases.
    fn provider_for(&self, eff: &LLMConfig) -> Arc<dyn Provider> {
        let key = format!("{}\u{0}{}\u{0}{}", eff.model, eff.api_base, eff.api_key);
        let mut cache = crate::lock(&self.providers);
        cache
            .entry(key)
            .or_insert_with(|| (self.factory)(eff))
            .clone()
    }
}

fn idea_to_slug(idea: &str) -> String {
    static RE: std::sync::OnceLock<Regex> = std::sync::OnceLock::new();
    let re = RE.get_or_init(|| Regex::new(r"[^a-z0-9]+").unwrap());
    let lowered = idea.to_lowercase();
    let slug = re.replace_all(lowered.trim(), "-");
    slug.chars()
        .take(60)
        .collect::<String>()
        .trim_matches('-')
        .to_string()
}

pub use flows_core::condition::eval_loop_condition;

fn index_of(phases: &[PhaseConfig], name: &str) -> Option<usize> {
    phases.iter().position(|p| p.name == name)
}

fn write_artifact_file(store: &Store, run_id: &str, phase_name: &str, artifact: &Artifact) {
    let path = store.artifact_path(run_id, phase_name);
    if let Ok(yaml) = serde_yaml_ng::to_string(artifact) {
        let _ = std::fs::write(path, yaml);
    }
}

/// Run one phase, returning (artifact, cost_usd, failure_error, fatal). `fatal`
/// forces the run to stop regardless of `on_failure` (used when `@auto:test` /
/// `@auto:lint` can't resolve a tool — an environment problem, not a test to
/// retry).
async fn execute_phase(
    ctx: &Ctx<'_>,
    phase: &PhaseConfig,
    prior_artifacts: &Map<String, Value>,
    prior_cost: f64,
) -> (Artifact, f64, Option<String>, bool) {
    match phase.phase_type {
        PhaseType::Agent => {
            let pcb = PhaseCb {
                cb: ctx.callbacks,
                phase: phase.name.clone(),
                store: ctx.store,
                run_id: ctx.run_id,
            };
            // Resolve this phase's model/endpoint and route to its provider.
            let eff = ctx.resolve_phase_llm(phase);
            let provider = ctx.provider_for(&eff);
            let result = run_agent_phase(
                phase,
                ctx.idea,
                ctx.repo_root,
                prior_artifacts,
                ctx.template_name,
                &eff,
                provider.as_ref(),
                &pcb,
                ctx.flows_dir,
                &ctx.context,
                &ctx.global_cfg.tools.web_search,
                ctx.global_cfg.max_cost_usd,
                prior_cost,
                Some(&ctx.logger),
            )
            .await;
            let err = result
                .artifact
                .content
                .get("error")
                .and_then(Value::as_str)
                .map(|e| format!("Agent phase {:?} failed: {e}", phase.name));
            (result.artifact, result.cost_usd, err, false)
        }
        PhaseType::Script => {
            // Resolve @auto:test / @auto:lint (pure Rust). An unresolvable tool is
            // fatal — stop rather than commit untested/unchecked code.
            let mut effective = phase.clone();
            if let Some(kind) = crate::script::auto_kind(phase.command.as_deref().unwrap_or("")) {
                match crate::script::resolve_auto(kind, ctx.repo_root) {
                    Ok(cmd) => {
                        ctx.callbacks.on_status(&phase.name, &format!("→ {cmd}"));
                        effective.command = Some(cmd);
                    }
                    Err(msg) => {
                        // Surface it in the output pane — otherwise the run just
                        // stops with only a desktop notification and nothing to see.
                        ctx.callbacks
                            .on_note(&phase.name, &format!("⛔ {}: {msg}", phase.name));
                        let artifact = Artifact {
                            phase: phase.name.clone(),
                            idea: ctx.idea.into(),
                            content: json!({ "error": msg }),
                            ..Artifact::default()
                        };
                        return (artifact, 0.0, Some(msg), true);
                    }
                }
            }
            match crate::script::run_script_cancellable(
                &effective,
                ctx.repo_root,
                ctx.idea,
                &|| ctx.store.is_cancelled(ctx.run_id),
            ) {
                Ok(res) => {
                    ctx.logger.script(
                        &phase.name,
                        res.command.as_str(),
                        res.exit_code,
                        &res.stdout,
                        &res.stderr,
                        0,
                    );
                    ctx.callbacks.on_script_result(
                        &phase.name,
                        res.exit_code,
                        &res.stdout,
                        &res.stderr,
                    );
                    let artifact = Artifact {
                        phase: phase.name.clone(),
                        idea: ctx.idea.into(),
                        content: json!({"stdout": res.stdout, "stderr": res.stderr, "exit_code": res.exit_code}),
                        ..Artifact::default()
                    };
                    let err = (!res.success())
                        .then(|| format!("Script phase {:?} exited {}", phase.name, res.exit_code));
                    (artifact, 0.0, err, false)
                }
                Err(e) => (
                    Artifact {
                        phase: phase.name.clone(),
                        idea: ctx.idea.into(),
                        ..Artifact::default()
                    },
                    0.0,
                    Some(e),
                    false,
                ),
            }
        }
        PhaseType::Human => {
            let instructions = phase
                .instructions
                .clone()
                .unwrap_or_else(|| format!("Review {} and advance when ready.", phase.name));
            // `auto_advance` skips the interactive gate entirely.
            let answer = if phase.auto_advance {
                "auto-advanced".to_string()
            } else {
                ctx.callbacks
                    .on_human_gate(&phase.name, &instructions)
                    .await
            };
            let artifact = Artifact {
                phase: phase.name.clone(),
                idea: ctx.idea.into(),
                content: json!({"answer": answer, "advanced": true}),
                ..Artifact::default()
            };
            (artifact, 0.0, None, false)
        }
    }
}

fn persist(ctx: &Ctx<'_>, prior: &mut Map<String, Value>, artifact: &Artifact) {
    // Stamp the run_id (phases build artifacts without it).
    let mut artifact = artifact.clone();
    artifact.run_id = ctx.run_id.to_string();

    prior.insert(
        artifact.phase.clone(),
        serde_json::to_value(&artifact).unwrap_or(Value::Null),
    );
    write_artifact_file(ctx.store, ctx.run_id, &artifact.phase, &artifact);
    ctx.callbacks.on_phase_end(&artifact.phase, Some(&artifact));
}

/// Evaluate a phase's `when` condition against the artifacts recorded so far.
/// `Ok(None)` → run it; `Ok(Some(reason))` → skip it; `Err` → fail the run.
fn when_skip(
    ctx: &Ctx<'_>,
    phase: &PhaseConfig,
    artifacts: &Map<String, Value>,
) -> Result<Option<String>, String> {
    let Some(cond) = phase.when.as_deref() else {
        return Ok(None);
    };
    let artifacts = flows_core::include::scoped_artifacts(phase, artifacts);
    match flows_core::condition::eval_when_with_vars(cond, &artifacts, ctx.idea, &phase.vars()) {
        Ok(true) => Ok(None),
        Ok(false) => Ok(Some(format!("when condition {cond:?} is false"))),
        Err(e) => Err(format!(
            "Phase {:?}: when condition {cond:?} failed to evaluate: {e}",
            phase.name
        )),
    }
}

/// Record a skipped phase: a `{skipped, reason}` artifact (so prompts and later
/// conditions see it) and the `on_phase_skipped` event.
fn record_skip(
    ctx: &Ctx<'_>,
    prior: &mut Map<String, Value>,
    state: &mut RunState,
    phase: &PhaseConfig,
    reason: &str,
) {
    let artifact = Artifact {
        phase: phase.name.clone(),
        run_id: ctx.run_id.to_string(),
        idea: ctx.idea.into(),
        content: json!({ "skipped": true, "reason": reason }),
        ..Artifact::default()
    };
    prior.insert(
        phase.name.clone(),
        serde_json::to_value(&artifact).unwrap_or(Value::Null),
    );
    write_artifact_file(ctx.store, ctx.run_id, &phase.name, &artifact);
    if !state.skipped_phases.contains(&phase.name) {
        state.skipped_phases.push(phase.name.clone());
    }
    ctx.callbacks.on_phase_skipped(&phase.name, reason);
}

/// Run a pipeline to completion. Returns the list of completed phase names.
#[allow(clippy::too_many_arguments)]
pub async fn run_pipeline(
    run_id: &str,
    idea: &str,
    repo_root: &Path,
    phases: &[PhaseConfig],
    template_name: &str,
    global_cfg: &GlobalConfig,
    project_cfg: &ProjectConfig,
    provider_factory: ProviderFactory<'_>,
    callbacks: &dyn RunnerCallbacks,
    store: &Store,
    start_phase: Option<&str>,
    flows_dir: Option<&Path>,
) -> Result<Vec<String>, RunError> {
    validate_pipeline(phases).map_err(RunError::Validation)?;
    let llm = merge_llm(global_cfg, project_cfg);
    let profiles = merge_profiles(global_cfg, project_cfg);

    store
        .ensure_run_dir(run_id)
        .map_err(|e| RunError::Other(e.to_string()))?;

    let meta = RunMeta {
        run_id: run_id.into(),
        idea: idea.into(),
        slug: idea_to_slug(idea),
        repo_path: repo_root.display().to_string(),
        template: template_name.into(),
        phases: phases
            .iter()
            .filter_map(|p| serde_json::to_value(p).ok())
            .collect(),
        created_at: String::new(),
        pid: Some(std::process::id() as i32),
    };
    let _ = store.write_meta(&meta);

    let mut state = RunState {
        run_id: run_id.into(),
        status: RunStatus::Running,
        current_phase: String::new(),
        current_phase_index: 0,
        round: 0,
        questions: vec![],
        error: None,
        completed_phases: vec![],
        skipped_phases: vec![],
        fan_outs: Default::default(),
    };
    let _ = store.write_state(&state);

    let start_idx = match start_phase {
        Some(name) => index_of(phases, name)
            .ok_or_else(|| RunError::Other(format!("Phase {name:?} not found in pipeline")))?,
        None => 0,
    };

    let logger = flows_core::LlmLogger::new(
        run_id,
        flows_core::llm_log::log_path(&store.run_dir(run_id)),
    );

    let ctx = Ctx {
        run_id,
        idea,
        repo_root,
        phases,
        template_name,
        global_cfg,
        llm,
        profiles,
        factory: provider_factory,
        providers: StdMutex::new(HashMap::new()),
        callbacks,
        store,
        flows_dir,
        logger,
        context: flows_core::config::merge_context(global_cfg, project_cfg),
    };

    if is_dag(phases) {
        run_dag(&ctx, &mut state, start_idx).await
    } else {
        run_linear(&ctx, &mut state, start_idx).await
    }
}

// ---------------------------------------------------------------------------
// Linear
// ---------------------------------------------------------------------------

async fn run_linear(
    ctx: &Ctx<'_>,
    state: &mut RunState,
    start_idx: usize,
) -> Result<Vec<String>, RunError> {
    // Owned: fan-out groups splice their expanded phases in at run time.
    let mut phases: Vec<PhaseConfig> = ctx.phases.to_vec();
    let mut prior_artifacts = Map::new();
    let mut completed: Vec<String> = Vec::new();
    let mut loop_counts: HashMap<String, u32> = HashMap::new();
    // Consecutive `retry:` failures per phase (reset when the phase passes).
    let mut retry_counts: HashMap<String, u32> = HashMap::new();
    let mut total_cost = 0.0f64;
    let mut idx = start_idx;
    let mut groups: HashMap<String, GroupRun> = HashMap::new();
    // The fan-out item the previous phase belonged to.
    let mut cur_item: Option<FanOutItem> = None;

    while idx < phases.len() {
        let here = phases[idx].fan_out().cloned();
        if here != cur_item {
            if let Some(prev) = cur_item.take() {
                finish_item(ctx, &mut groups, &mut prior_artifacts, &prev);
            }
            if let Some(f) = &here {
                if let Some(g) = groups.get(&f.group) {
                    if let Some(reason) = g.blocked_reason(&f.id) {
                        let p = phases[idx].clone();
                        record_skip(ctx, &mut prior_artifacts, state, &p, &reason);
                        if !completed.contains(&p.name) {
                            completed.push(p.name.clone());
                        }
                        state.completed_phases = completed.clone();
                        let _ = ctx.store.write_state(state);
                        idx += 1;
                        continue;
                    }
                    if let Some((i, it)) = g.items.iter().enumerate().find(|(_, it)| it.id == f.id)
                    {
                        ctx.callbacks.on_fan_out_item(
                            &f.group,
                            &it.id,
                            &it.title,
                            i + 1,
                            g.items.len(),
                        );
                    }
                }
            }
            cur_item = here;
        }
        let total = phases.len();
        let phase = &with_done_vars(&phases[idx], &groups);
        match when_skip(ctx, phase, &prior_artifacts) {
            Err(error) => {
                state.current_phase = phase.name.clone();
                state.current_phase_index = idx as i64;
                return fail_run(ctx, state, &phase.name, error);
            }
            Ok(Some(reason)) => {
                record_skip(ctx, &mut prior_artifacts, state, phase, &reason);
                if phase.is_group() {
                    // A skipped group must not run an expansion from an earlier pass.
                    phases.retain(|p| p.fan_out().map_or(true, |f| f.group != phase.name));
                }
                if !completed.contains(&phase.name) {
                    completed.push(phase.name.clone());
                }
                state.completed_phases = completed.clone();
                let _ = ctx.store.write_state(state);
                idx += 1;
                continue;
            }
            Ok(None) => state.skipped_phases.retain(|n| n != &phase.name),
        }
        ctx.callbacks.on_phase_start(&phase.name, idx, total);
        state.current_phase = phase.name.clone();
        state.current_phase_index = idx as i64;
        state.status = RunStatus::Running;
        let _ = ctx.store.write_state(state);

        if ctx.store.poll_cancel(ctx.run_id) {
            state.status = RunStatus::Cancelled;
            let _ = ctx.store.write_state(state);
            return Ok(completed);
        }

        if matches!(phase.phase_type, PhaseType::Agent)
            && ctx.global_cfg.max_cost_usd > 0.0
            && total_cost >= ctx.global_cfg.max_cost_usd
        {
            let error = format!(
                "Cost limit ${:.2} reached (${:.4} spent so far)",
                ctx.global_cfg.max_cost_usd, total_cost
            );
            return fail_run(ctx, state, &phase.name, error);
        }

        if phase.is_group() {
            let run = match start_group(ctx, phase, &prior_artifacts, groups.get(&phase.name)) {
                Ok(run) => run,
                Err(error) => return fail_run(ctx, state, &phase.name, error),
            };
            // Re-entry: drop the previous expansion and its bookkeeping.
            let prefix = format!("{}.", phase.name);
            phases.retain(|p| p.fan_out().map_or(true, |f| f.group != phase.name));
            completed.retain(|n| !n.starts_with(&prefix));
            state.skipped_phases.retain(|n| !n.starts_with(&prefix));
            loop_counts.retain(|n, _| !n.starts_with(&prefix));
            retry_counts.retain(|n, _| !n.starts_with(&prefix));
            state.fan_outs.insert(
                phase.name.clone(),
                run.items.iter().map(|i| i.id.clone()).collect(),
            );
            let listed: Vec<(String, String)> = run
                .items
                .iter()
                .map(|i| (i.id.clone(), i.title.clone()))
                .collect();
            ctx.callbacks.on_fan_out(&phase.name, &listed);
            if run.items.is_empty() {
                record_skip(
                    ctx,
                    &mut prior_artifacts,
                    state,
                    phase,
                    "for_each yielded no items",
                );
            } else {
                persist(ctx, &mut prior_artifacts, &run.artifact(ctx, &phase.name));
                let expanded = flows_core::fan_out::expand(phase, &run.items, ctx.template_name);
                phases.splice(idx + 1..idx + 1, expanded);
            }
            groups.insert(phase.name.clone(), run);
            if !completed.contains(&phase.name) {
                completed.push(phase.name.clone());
            }
            state.completed_phases = completed.clone();
            let _ = ctx.store.write_state(state);
            idx += 1;
            continue;
        }

        let (artifact, cost, failed_error, fatal) =
            execute_phase(ctx, phase, &prior_artifacts, total_cost).await;
        total_cost += cost;
        if cost > 0.0 {
            ctx.callbacks.on_cost(total_cost);
        }

        // A fatal failure (e.g. @auto:* couldn't resolve a tool) stops the run
        // regardless of on_failure.
        if fatal {
            if let Some(error) = failed_error {
                return fail_run(ctx, state, &phase.name, error);
            }
        }

        // Mid-phase cancellation: the phase bailed early; stop without persisting
        // the partial artifact as a failure.
        if ctx.store.poll_cancel(ctx.run_id) {
            state.status = RunStatus::Cancelled;
            let _ = ctx.store.write_state(state);
            return Ok(completed);
        }

        persist(ctx, &mut prior_artifacts, &artifact);
        if !completed.contains(&phase.name) {
            completed.push(phase.name.clone());
        }
        state.completed_phases = completed.clone();
        let _ = ctx.store.write_state(state);

        if let Some(mut error) = failed_error {
            let mut action = phase.on_failure.clone();
            if let OnFailure::Retry(target) = &action {
                // `retry:` is bounded by `max_loop_iterations`: once that many
                // retries in a row have failed, the phase stops like `stop`.
                let count = retry_counts.entry(phase.name.clone()).or_insert(0);
                *count += 1;
                if *count > phase.max_loop_iterations {
                    error = format!(
                        "{error} (still failing after {} retries of {target:?})",
                        phase.max_loop_iterations
                    );
                    action = OnFailure::Stop;
                }
            }
            match &action {
                OnFailure::Stop => {
                    // In a group with `on_failure: continue`, a stopping failure
                    // fails only the item (and skips the items depending on it).
                    let item = phase.fan_out().cloned();
                    let run = item.as_ref().and_then(|f| groups.get_mut(&f.group));
                    match (item, run) {
                        (Some(f), Some(run)) if run.continue_on_failure => {
                            run.fail(&f.id);
                            write_group_artifact(ctx, &mut prior_artifacts, &f.group, run);
                            ctx.callbacks
                                .on_note(&phase.name, &format!("✗ item {} failed: {error}", f.id));
                            while idx < phases.len() && phases[idx].fan_out() == Some(&f) {
                                idx += 1;
                            }
                            continue;
                        }
                        _ => return fail_run(ctx, state, &phase.name, error),
                    }
                }
                OnFailure::Continue => {
                    idx += 1;
                    continue;
                }
                OnFailure::Retry(target) => match index_of(&phases, target) {
                    Some(t) => {
                        idx = t;
                        continue;
                    }
                    None => {
                        return Err(RunError::Other(format!(
                            "retry target {target:?} not found"
                        )))
                    }
                },
            }
        }

        retry_counts.remove(&phase.name);

        if let Some(target) = &phase.loop_back_to {
            if eval_loop_condition(&phase.loop_condition, &artifact.content) {
                let count = loop_counts.entry(phase.name.clone()).or_insert(0);
                *count += 1;
                if *count <= phase.max_loop_iterations {
                    ctx.callbacks.on_loop_back(
                        &phase.name,
                        target,
                        *count,
                        phase.max_loop_iterations,
                    );
                    if let Some(t) = index_of(&phases, target) {
                        // The phases in [target..=current] are about to re-run — drop
                        // them from completed_phases so the UI doesn't show a stale ✓
                        // (e.g. a rejected design must un-check as it loops back).
                        let redo: std::collections::HashSet<&str> =
                            phases[t..=idx].iter().map(|p| p.name.as_str()).collect();
                        completed.retain(|n| !redo.contains(n.as_str()));
                        state.completed_phases = completed.clone();
                        state.skipped_phases.retain(|n| !redo.contains(n.as_str()));
                        let _ = ctx.store.write_state(state);
                        idx = t;
                        continue;
                    }
                }
            }
        }

        idx += 1;
    }
    if let Some(prev) = cur_item.take() {
        finish_item(ctx, &mut groups, &mut prior_artifacts, &prev);
    }

    state.status = RunStatus::Completed;
    let _ = ctx.store.write_state(state);
    ctx.callbacks.on_run_complete(ctx.run_id, &completed);
    Ok(completed)
}

// ---------------------------------------------------------------------------
// Fan-out groups
// ---------------------------------------------------------------------------

/// Run-time bookkeeping for one `for_each` group.
struct GroupRun {
    items: Vec<Item>,
    /// The group's item variable (`as:`).
    var: String,
    /// Nested phase names, in order (to find an item's agent summaries).
    nested: Vec<String>,
    continue_on_failure: bool,
    completed: Vec<String>,
    failed: Vec<String>,
    /// Items skipped because an item they depend on failed → that item.
    skipped: Vec<(String, String)>,
    /// Finished item → its last agent `content` (`summary`, `files_written`).
    results: HashMap<String, Value>,
}

impl GroupRun {
    /// Why item `id` must not run, if it mustn't.
    fn blocked_reason(&self, id: &str) -> Option<String> {
        if self.failed.iter().any(|f| f == id) {
            return Some(format!("item {id} failed"));
        }
        self.skipped
            .iter()
            .find(|(s, _)| s == id)
            .map(|(_, cause)| format!("depends on failed item {cause}"))
    }

    fn fail(&mut self, id: &str) {
        if !self.failed.iter().any(|f| f == id) {
            self.failed.push(id.to_string());
        }
        for d in flows_core::fan_out::dependents(&self.items, id) {
            if !self.skipped.iter().any(|(s, _)| *s == d) {
                self.skipped.push((d, id.to_string()));
            }
        }
    }

    fn artifact(&self, ctx: &Ctx<'_>, group: &str) -> Artifact {
        let ids = |v: &[String]| -> Vec<String> {
            // Run order, not completion order.
            self.items
                .iter()
                .filter(|i| v.contains(&i.id))
                .map(|i| i.id.clone())
                .collect()
        };
        let skipped: Vec<String> = self.skipped.iter().map(|(s, _)| s.clone()).collect();
        // Roll the items' results up, so prompts reading `<group>.summary` /
        // `.files_written` (as for a single implement phase) still work.
        let mut summary = Vec::new();
        let mut files: Vec<Value> = Vec::new();
        for it in &self.items {
            let Some(r) = self.results.get(&it.id) else {
                continue;
            };
            if let Some(s) = r.get("summary").and_then(Value::as_str) {
                summary.push(format!("[{} {}] {s}", it.id, it.title));
            }
            for f in r
                .get("files_written")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
            {
                if !files.contains(f) {
                    files.push(f.clone());
                }
            }
        }
        Artifact {
            phase: group.to_string(),
            idea: ctx.idea.into(),
            content: json!({
                "items": self.items.iter().map(|i| i.value.clone()).collect::<Vec<_>>(),
                "completed": ids(&self.completed),
                "failed": ids(&self.failed),
                "skipped": ids(&skipped),
                "summary": summary.join("\n"),
                "files_written": files,
            }),
            ..Artifact::default()
        }
    }

    /// The content of item `id`'s last agent phase that produced a `summary`.
    fn item_result(&self, group: &str, id: &str, prior: &Map<String, Value>) -> Option<Value> {
        let prefix = flows_core::fan_out::item_prefix(group, id);
        self.nested.iter().rev().find_map(|n| {
            let content = prior.get(&format!("{prefix}{n}"))?.get("content")?;
            content.get("summary")?;
            Some(content.clone())
        })
    }
}

/// Evaluate (or, once any item completed, reuse) a group's items.
fn start_group(
    ctx: &Ctx<'_>,
    group: &PhaseConfig,
    artifacts: &Map<String, Value>,
    previous: Option<&GroupRun>,
) -> Result<GroupRun, String> {
    let items = match previous {
        Some(prev) if !prev.completed.is_empty() => prev.items.clone(),
        _ => {
            let expr = group.for_each.as_deref().unwrap_or_default();
            let scoped = flows_core::include::scoped_artifacts(group, artifacts);
            let raw = flows_core::condition::eval_list(expr, &scoped, ctx.idea, &group.vars())
                .map_err(|e| {
                    let hint = flows_core::fan_out::missing_list_hint(expr, &scoped)
                        .map(|h| format!(" ({h})"))
                        .unwrap_or_default();
                    format!("Group {:?}: for_each {expr:?}: {e}{hint}", group.name)
                })?;
            flows_core::fan_out::items(raw, flows_core::fan_out::max_items(group))
                .map_err(|e| format!("Group {:?}: for_each {expr:?}: {e}", group.name))?
        }
    };
    Ok(GroupRun {
        items,
        var: group.item_var().to_string(),
        nested: group.phases.iter().map(|p| p.name.clone()).collect(),
        continue_on_failure: matches!(group.on_failure, OnFailure::Continue),
        completed: Vec::new(),
        failed: Vec::new(),
        skipped: Vec::new(),
        results: HashMap::new(),
    })
}

/// Record an updated group artifact (no phase-end event: the group already
/// ended when it expanded).
fn write_group_artifact(
    ctx: &Ctx<'_>,
    prior: &mut Map<String, Value>,
    group: &str,
    run: &GroupRun,
) {
    let mut artifact = run.artifact(ctx, group);
    artifact.run_id = ctx.run_id.to_string();
    prior.insert(
        group.to_string(),
        serde_json::to_value(&artifact).unwrap_or(Value::Null),
    );
    write_artifact_file(ctx.store, ctx.run_id, group, &artifact);
}

/// The run left item `item`: unless it failed, it completed.
fn finish_item(
    ctx: &Ctx<'_>,
    groups: &mut HashMap<String, GroupRun>,
    prior: &mut Map<String, Value>,
    item: &FanOutItem,
) {
    let Some(run) = groups.get_mut(&item.group) else {
        return;
    };
    if run.blocked_reason(&item.id).is_some() || run.completed.contains(&item.id) {
        return;
    }
    run.completed.push(item.id.clone());
    if let Some(r) = run.item_result(&item.group, &item.id, prior) {
        run.results.insert(item.id.clone(), r);
    }
    write_group_artifact(ctx, prior, &item.group, run);
}

/// `phase` with `vars.<as>_done` filled in when it belongs to a fan-out item:
/// `{id, title, summary}` for each item of its group finished so far, where
/// `summary` is that item's last `content.summary`.
fn with_done_vars(phase: &PhaseConfig, groups: &HashMap<String, GroupRun>) -> PhaseConfig {
    let mut phase = phase.clone();
    let Some(run) = phase.fan_out().and_then(|f| groups.get(&f.group)) else {
        return phase;
    };
    let done: Vec<Value> = run
        .items
        .iter()
        .filter(|i| run.completed.contains(&i.id))
        .map(|i| {
            let summary = run
                .results
                .get(&i.id)
                .and_then(|r| r.get("summary"))
                .cloned()
                .unwrap_or_else(|| Value::String(String::new()));
            json!({ "id": i.id, "title": i.title, "summary": summary })
        })
        .collect();
    if let Some(o) = &mut phase.origin {
        o.vars
            .insert(format!("{}_done", run.var), Value::Array(done));
    }
    phase
}

fn fail_run(
    ctx: &Ctx<'_>,
    state: &mut RunState,
    phase: &str,
    error: String,
) -> Result<Vec<String>, RunError> {
    state.status = RunStatus::Failed;
    state.error = Some(error.clone());
    let _ = ctx.store.write_state(state);
    ctx.callbacks.on_run_failed(ctx.run_id, phase, &error);
    Err(RunError::Failed {
        phase: phase.to_string(),
        error,
    })
}

// ---------------------------------------------------------------------------
// DAG
// ---------------------------------------------------------------------------

async fn run_dag(
    ctx: &Ctx<'_>,
    state: &mut RunState,
    start_idx: usize,
) -> Result<Vec<String>, RunError> {
    let phases = ctx.phases;
    let total = phases.len();
    let deps: HashMap<&str, Vec<&str>> = phases
        .iter()
        .map(|p| {
            (
                p.name.as_str(),
                p.depends_on.iter().map(String::as_str).collect(),
            )
        })
        .collect();

    let mut completed: HashSet<String> = HashSet::new();
    let mut completed_order: Vec<String> = Vec::new();
    let failed: HashSet<String> = HashSet::new();
    let mut artifacts = Map::new();
    let mut total_cost = 0.0f64;

    // phases before start_idx are considered already done
    for p in &phases[..start_idx] {
        completed.insert(p.name.clone());
    }

    let mut running: FuturesUnordered<_> = FuturesUnordered::new();
    let mut in_flight: HashSet<String> = HashSet::new();

    loop {
        // Schedule everything whose deps are satisfied.
        let ready: Vec<&PhaseConfig> = phases
            .iter()
            .filter(|p| {
                !completed.contains(&p.name)
                    && !failed.contains(&p.name)
                    && !in_flight.contains(&p.name)
                    && deps[p.name.as_str()].iter().all(|d| completed.contains(*d))
                    && !deps[p.name.as_str()].iter().any(|d| failed.contains(*d))
            })
            .collect();

        if ready.is_empty() && in_flight.is_empty() {
            break;
        }

        // A skip changes readiness without a task completing; re-scan before
        // awaiting in-flight work.
        let mut skipped_any = false;
        for phase in ready {
            match when_skip(ctx, phase, &artifacts) {
                Err(error) => {
                    state.current_phase = phase.name.clone();
                    return fail_run(ctx, state, &phase.name, error);
                }
                Ok(Some(reason)) => {
                    record_skip(ctx, &mut artifacts, state, phase, &reason);
                    completed.insert(phase.name.clone());
                    completed_order.push(phase.name.clone());
                    state.completed_phases = completed_order.clone();
                    let _ = ctx.store.write_state(state);
                    skipped_any = true;
                    continue;
                }
                Ok(None) => {}
            }
            // Human phases serialize: don't start one while others are in flight.
            if matches!(phase.phase_type, PhaseType::Human) && !in_flight.is_empty() {
                break;
            }
            if ctx.global_cfg.max_cost_usd > 0.0
                && matches!(phase.phase_type, PhaseType::Agent)
                && total_cost >= ctx.global_cfg.max_cost_usd
            {
                let error = format!(
                    "Cost limit ${:.2} reached (${:.4} spent so far)",
                    ctx.global_cfg.max_cost_usd, total_cost
                );
                return fail_run(ctx, state, &phase.name, error);
            }

            let idx = index_of(phases, &phase.name).unwrap_or(0);
            ctx.callbacks.on_phase_start(&phase.name, idx, total);
            in_flight.insert(phase.name.clone());
            let snapshot = artifacts.clone();
            let prior_cost = total_cost;
            running.push(async move {
                let (artifact, cost, err, fatal) =
                    execute_phase(ctx, phase, &snapshot, prior_cost).await;
                (phase.name.clone(), artifact, cost, err, fatal)
            });
        }
        if skipped_any {
            continue;
        }

        let Some((name, artifact, cost, failed_error, fatal)) = running.next().await else {
            break;
        };
        in_flight.remove(&name);
        total_cost += cost;
        if cost > 0.0 {
            ctx.callbacks.on_cost(total_cost);
        }

        // A fatal failure (e.g. @auto:* couldn't resolve a tool) stops the run.
        if fatal {
            if let Some(error) = failed_error {
                return fail_run(ctx, state, &name, error);
            }
        }

        // Mid-phase cancellation: stop, drop in-flight tasks, don't persist.
        if ctx.store.poll_cancel(ctx.run_id) {
            state.status = RunStatus::Cancelled;
            let _ = ctx.store.write_state(state);
            return Ok(completed_order);
        }

        persist(ctx, &mut artifacts, &artifact);

        if let Some(error) = failed_error {
            if phases[index_of(phases, &name).unwrap()].on_failure == OnFailure::Continue {
                completed.insert(name.clone());
                completed_order.push(name.clone());
            } else {
                // stop: let in-flight tasks drop (cancelled), fail the run.
                return fail_run(ctx, state, &name, error);
            }
        } else {
            completed.insert(name.clone());
            completed_order.push(name.clone());
        }
        state.completed_phases = completed_order.clone();
        let _ = ctx.store.write_state(state);

        if ctx.store.poll_cancel(ctx.run_id) {
            state.status = RunStatus::Cancelled;
            let _ = ctx.store.write_state(state);
            return Ok(completed_order);
        }
    }

    state.status = RunStatus::Completed;
    let _ = ctx.store.write_state(state);
    ctx.callbacks.on_run_complete(ctx.run_id, &completed_order);
    Ok(completed_order)
}
