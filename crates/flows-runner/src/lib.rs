//! flows-runner — phases (agent / script / human) + pipeline execution
//! (linear and DAG, loops, retries, cost guard, mid-phase cancel).

pub mod agent;
pub mod prompts;
pub mod repo_context;
pub mod runner;
pub mod script;

pub use agent::{run_agent_phase, AgentCallbacks, AgentResult, NoopCallbacks};
pub use prompts::{builtin_prompt_files, load_prompt, render_prompt};
pub use runner::{
    default_provider_factory, eval_loop_condition, run_pipeline, ProviderFactory, RunError,
    RunnerCallbacks,
};
pub use script::run_script;
