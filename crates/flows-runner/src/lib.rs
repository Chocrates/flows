//! flows-runner — phases (agent / script / human) + pipeline execution
//! (linear and DAG, loops, retries, cost guard, mid-phase cancel).

/// Lock a mutex, recovering from poison instead of panicking. Poison only occurs
/// if a thread panicked while holding the lock; these guarded regions are tiny and
/// never panic, and the data behind the lock stays valid, so we use it as-is. One
/// helper so every lock site behaves the same (no `.unwrap()` vs `.map_err()` mix).
pub(crate) fn lock<T>(m: &std::sync::Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

pub mod agent;
pub mod budget;
mod chunker;
pub mod prompts;
pub mod repo_context;
pub mod repo_map;
pub mod retrieval;
pub mod runner;
pub mod script;

pub use agent::{run_agent_phase, AgentCallbacks, AgentResult, NoopCallbacks};
pub use prompts::{builtin_prompt_files, load_prompt, render_prompt};
pub use runner::{
    default_provider_factory, eval_loop_condition, run_pipeline, ProviderFactory, RunError,
    RunnerCallbacks,
};
pub use script::run_script;
