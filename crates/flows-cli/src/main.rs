//! `flows` CLI — the command-line entry point and the launcher. Subcommands:
//! doctor, config, init, templates, plan, run, runs, usage, attach, completions.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use async_trait::async_trait;
use clap::{CommandFactory, Parser, Subcommand};
use std::collections::HashMap;

use flows_core::config::{
    apply_context_override, apply_user_project_override, load_global_config, load_project_config,
    load_user_project_override, merge_llm, merge_profiles, resolve_llm,
};
use flows_core::detect::{detect_stack, generate_config};
use flows_core::dryrun::dry_run;
use flows_core::models::{
    Artifact, GlobalConfig, LLMConfig, PhaseConfig, PhaseType, ProjectConfig, RunStatus,
};
use flows_core::pricing::format_cost;
use flows_core::state::Store;
use flows_core::templates::{builtin_files, list_templates, load_template};
use flows_runner::runner::{default_provider_factory, run_pipeline, RunnerCallbacks};

mod eval;
mod eval_store;
mod eval_tui;
mod searxng;

#[derive(Parser)]
#[command(
    name = "flows",
    version,
    about = "Local LLM workflow runner (Rust port)"
)]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// Check environment: config, sandbox, available templates.
    Doctor,
    /// Generate a `.flows.yml` for the current project (or sync global templates).
    Init {
        /// Sync shipped templates into ~/.flows/templates instead.
        #[arg(long)]
        global: bool,
        /// Template to target in the generated config.
        #[arg(long, default_value = "feature")]
        template: String,
        /// Overwrite an existing file.
        #[arg(long)]
        force: bool,
    },
    /// Template management.
    Templates {
        #[command(subcommand)]
        command: TemplatesCmd,
    },
    /// Show or set global config (~/.flows/config.yml).
    Config {
        #[command(subcommand)]
        command: ConfigCmd,
    },
    /// Validate config and show the execution plan + cost estimate.
    Plan {
        /// The idea / task for the run.
        idea: Vec<String>,
        /// Template to use (defaults to project config or `feature`).
        #[arg(long)]
        template: Option<String>,
        /// Change to this directory first (default: current directory).
        #[arg(long = "directory", short = 'C', value_name = "DIR")]
        directory: Option<String>,
    },
    /// Run a pipeline against the configured model.
    Run {
        /// The idea / task for the run.
        idea: Vec<String>,
        /// Template to use (defaults to project config or `feature`).
        #[arg(long)]
        template: Option<String>,
        /// Change to this directory first, like `git -C` (default: current
        /// directory) — launch from anywhere but operate on another project.
        #[arg(long = "directory", short = 'C', value_name = "DIR")]
        directory: Option<String>,
        /// Run in the full-screen TUI instead of plain foreground output.
        #[arg(long)]
        tui: bool,
        /// Detach and run in the background; returns immediately.
        #[arg(long)]
        background: bool,
        /// Context-delivery features for this run only, overriding the config's
        /// flags: `baseline`, `all`, or tokens joined by `+` (`search`, `range`,
        /// `slim`, `map`, `budget`, `rag`), e.g. `--context map+rag`.
        #[arg(long, value_name = "CONFIG")]
        context: Option<String>,
    },
    /// List recent runs and their status.
    Runs,
    /// Stop a running flow: cancel it and kill its worker (keeps the run + output).
    Stop {
        /// Run id to stop (from `flows runs`; a unique prefix works).
        run_id: String,
    },
    /// Manage an optional self-hosted SearXNG container for the `searxng` search
    /// backend (auto-detects docker / podman / nerdctl / finch).
    Searxng {
        /// up (start) | down (stop) | status. Defaults to status.
        #[arg(default_value = "status")]
        action: String,
    },
    /// Delete a single run by id (use --force to stop + delete a running one).
    Delete {
        /// Run id to delete (from `flows runs`; a unique prefix works).
        run_id: String,
        /// Cancel + kill the worker first if the run is still active.
        #[arg(long)]
        force: bool,
    },
    /// Delete finished runs from ~/.flows/runs (use --all to delete everything).
    Clean {
        /// Also delete runs that are still running / waiting for input.
        #[arg(long)]
        all: bool,
    },
    /// Benchmark models on task fixtures: a model × context-config × fixture ×
    /// trial matrix, stored for offline grading and comparison.
    Eval {
        #[command(subcommand)]
        command: EvalCmd,
    },
    /// Show token usage + cost for a run.
    Usage {
        /// Run id to report on (from `flows runs`; a unique prefix works).
        run_id: String,
    },
    /// Attach a TUI view to a run (shows phase progress from its state).
    Attach {
        /// Run id to attach to (from `flows runs`; a unique prefix works).
        run_id: String,
    },
    /// Print a shell completion script (bash/zsh/fish/…).
    Completions { shell: clap_complete::Shell },
    /// Internal: run a backgrounded pipeline (spawned by `run --background`).
    #[command(hide = true)]
    Worker { run_id: String },
}

#[derive(Subcommand)]
enum EvalCmd {
    /// Run the eval matrix and store a record per trial.
    Run {
        /// Models to evaluate (CSV of profile names or literal models). Append
        /// `:coder` or `:reasoner` to a model to restrict it to that suite
        /// (e.g. `coder:coder,reasoner:reasoner`). Omit → configured default.
        #[arg(long)]
        models: Option<String>,
        /// Context configs (CSV, e.g. `baseline,search+map`). Omit → each fixture's
        /// own `configs:` axis (or `baseline`). When given, also FILTERS a fixture's
        /// declared axis — e.g. `--configs oracle` runs only the oracle config.
        #[arg(long)]
        configs: Option<String>,
        /// Fixture suite filter: coder | reasoner | all.
        #[arg(long, default_value = "all")]
        suite: String,
        /// Fixture size filter: small | medium | large | all.
        #[arg(long, default_value = "all")]
        size: String,
        /// Granularity filter: flow | phase | turn | all.
        #[arg(long, default_value = "all")]
        granularity: String,
        /// Fixture-name filter (CSV, substring match, e.g. `--fixture core` or
        /// `--fixture medium-core-reasoner`). Omit → all matching the other filters.
        #[arg(long)]
        fixture: Option<String>,
        /// Directory of fixture folders (each with fixture.yml + repo/). Omit for built-ins.
        #[arg(long)]
        fixtures: Option<String>,
        /// Trials per cell (>1 to expose model non-determinism).
        #[arg(long, default_value_t = 1)]
        trials: usize,
    },
    /// List discovered fixtures.
    List {
        /// Directory of fixture folders (each with fixture.yml + repo/). Omit for built-ins.
        #[arg(long)]
        fixtures: Option<String>,
        /// Filter to this suite: coder | reasoner | all.
        #[arg(long, default_value = "all")]
        suite: String,
        /// Filter to this size: small | medium | large | all.
        #[arg(long, default_value = "all")]
        size: String,
        /// Filter to this granularity: flow | phase | turn | all.
        #[arg(long, default_value = "all")]
        granularity: String,
        /// Fixture-name filter (CSV, substring match).
        #[arg(long)]
        fixture: Option<String>,
    },
    /// Manually grade ungraded trials (the pass signal for reasoner tasks).
    Review {
        /// Only grade this suite: coder | reasoner | all.
        #[arg(long, default_value = "all")]
        suite: String,
        /// Only grade trials from this model (profile name or literal).
        #[arg(long)]
        model: Option<String>,
    },
    /// Print a comparison table from the stored results (no model calls).
    Report {
        /// Filter to this model (profile name or literal).
        #[arg(long)]
        model: Option<String>,
        /// Filter to this context config (e.g. `baseline`, `oracle`, `rag`).
        #[arg(long)]
        config: Option<String>,
        /// Filter to this suite: coder | reasoner | all.
        #[arg(long, default_value = "all")]
        suite: String,
        /// Filter to this granularity: flow | phase | turn | all.
        #[arg(long, default_value = "all")]
        granularity: String,
    },
    /// Delete stored results (no filters = wipe the whole store).
    Clean {
        /// Only delete trials from this model.
        #[arg(long)]
        model: Option<String>,
        /// Only delete trials from fixtures whose name matches (CSV substring).
        #[arg(long)]
        fixture: Option<String>,
        /// Only delete this suite: coder | reasoner | all.
        #[arg(long, default_value = "all")]
        suite: String,
        /// Only delete this granularity: flow | phase | turn | all.
        #[arg(long, default_value = "all")]
        granularity: String,
        /// Skip the confirmation prompt.
        #[arg(long)]
        yes: bool,
    },
    /// Deterministic retrieval-quality eval: recall@k / MRR of the repo map and
    /// embedding retrieval against each fixture's labeled `relevant` files. No LLM.
    Retrieval {
        /// Directory of fixture folders (each with fixture.yml + repo/). Omit for built-ins.
        #[arg(long)]
        fixtures: Option<String>,
        /// Local embedding model for the retrieval ranker (empty → all-MiniLM-L6-v2).
        #[arg(long)]
        model: Option<String>,
        /// Fixture-name filter (CSV, substring match).
        #[arg(long)]
        fixture: Option<String>,
    },
}

#[derive(Subcommand)]
enum TemplatesCmd {
    /// List available templates (project → global → builtin).
    List,
    /// Write shipped templates into ~/.flows/templates.
    Sync {
        /// Overwrite existing files.
        #[arg(long)]
        force: bool,
    },
    /// Scaffold a new, heavily-commented template in ~/.flows/templates.
    New {
        /// Template name (alphanumeric, dashes/underscores ok).
        name: String,
    },
}

#[derive(Subcommand)]
enum ConfigCmd {
    /// Show the resolved config (model, provider, endpoint, limits).
    Show,
    /// Set a value in ~/.flows/config.yml.
    Set {
        /// Which setting: `model`, `api-base`, `api-key`, or `max-cost`.
        key: String,
        /// The new value (for `max-cost`, USD; 0 = no cap).
        value: String,
    },
}

fn home_dir() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/"))
}

fn flows_dir() -> PathBuf {
    home_dir().join(".flows")
}

fn which(bin: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|d| d.join(bin))
        .find(|p| p.is_file())
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let Some(command) = cli.command else {
        // No subcommand → the standalone launcher/home TUI.
        return match cmd_home() {
            Ok(()) => ExitCode::SUCCESS,
            Err(msg) => {
                eprintln!("error: {msg}");
                ExitCode::FAILURE
            }
        };
    };
    let result = match command {
        Command::Doctor => cmd_doctor(),
        Command::Init {
            global,
            template,
            force,
        } => cmd_init(global, &template, force),
        Command::Templates { command } => match command {
            TemplatesCmd::List => cmd_templates_list(),
            TemplatesCmd::Sync { force } => cmd_templates_sync(force),
            TemplatesCmd::New { name } => cmd_templates_new(&name),
        },
        Command::Config { command } => match command {
            ConfigCmd::Show => cmd_config_show(),
            ConfigCmd::Set { key, value } => cmd_config_set(&key, &value),
        },
        Command::Plan {
            idea,
            template,
            directory,
        } => cmd_plan(&idea.join(" "), template.as_deref(), directory.as_deref()),
        Command::Run {
            idea,
            template,
            directory,
            tui,
            background,
            context,
        } => cmd_run(
            &idea.join(" "),
            template.as_deref(),
            directory.as_deref(),
            tui,
            background,
            context.as_deref(),
        ),
        Command::Runs => cmd_runs(),
        Command::Stop { run_id } => cmd_stop(&run_id),
        Command::Searxng { action } => searxng::cmd(&action),
        Command::Delete { run_id, force } => cmd_delete(&run_id, force),
        Command::Clean { all } => cmd_clean(all),
        Command::Eval { command } => match command {
            EvalCmd::Run {
                models,
                configs,
                suite,
                size,
                granularity,
                fixture,
                fixtures,
                trials,
            } => eval::cmd_run(
                models.as_deref(),
                configs.as_deref(),
                &suite,
                &size,
                &granularity,
                fixture.as_deref(),
                fixtures.as_deref(),
                trials,
            ),
            EvalCmd::List {
                fixtures,
                suite,
                size,
                granularity,
                fixture,
            } => eval::cmd_list(
                fixtures.as_deref(),
                &suite,
                &size,
                &granularity,
                fixture.as_deref(),
            ),
            EvalCmd::Review { suite, model } => eval::cmd_review(&suite, model.as_deref()),
            EvalCmd::Report {
                model,
                config,
                suite,
                granularity,
            } => eval::cmd_report(model.as_deref(), config.as_deref(), &suite, &granularity),
            EvalCmd::Clean {
                model,
                fixture,
                suite,
                granularity,
                yes,
            } => eval::cmd_clean(
                model.as_deref(),
                fixture.as_deref(),
                &suite,
                &granularity,
                yes,
            ),
            EvalCmd::Retrieval {
                fixtures,
                model,
                fixture,
            } => eval::cmd_retrieval(fixtures.as_deref(), model.as_deref(), fixture.as_deref()),
        },
        Command::Completions { shell } => {
            clap_complete::generate(shell, &mut Cli::command(), "flows", &mut std::io::stdout());
            Ok(())
        }
        Command::Usage { run_id } => cmd_usage(&run_id),
        Command::Attach { run_id } => {
            let store = Store::new(flows_dir());
            match resolve_run_id(&store, &run_id) {
                Ok(id) => flows_tui::attach_tui(store, id).map_err(|e| e.to_string()),
                Err(e) => Err(e),
            }
        }
        Command::Worker { run_id } => cmd_worker(&run_id),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(msg) => {
            eprintln!("error: {msg}");
            ExitCode::FAILURE
        }
    }
}

// ---------------------------------------------------------------------------
// doctor
// ---------------------------------------------------------------------------

/// Probe the platform sandbox and, if it's not usable, print the right fix for
/// this OS (bubblewrap/userns on Linux, Seatbelt on macOS).
fn check_sandbox() {
    let name = flows_tools::sandbox::backend_name();
    if flows_tools::sandbox::sandbox_available() {
        println!("  sandbox:   OK ({name})");
        let mode = flows_tools::sandbox::limits_mode();
        let defaults = flows_tools::sandbox::Limits::default();
        println!(
            "  limits:    {} (defaults {} MB / {} procs; override with tools.bash.memory_mb / max_procs)",
            mode.describe(),
            defaults.memory_mb,
            defaults.max_procs
        );
        let hidden =
            flows_tools::sandbox::hidden_paths(&flows_tools::sandbox::Confinement::default());
        println!(
            "  secrets:   {} credential path(s) under ~ hidden from commands \
             (tools.bash.expose_credentials: true re-exposes all but ~/.flows config)",
            hidden.len()
        );
        return;
    }
    println!("  sandbox:   {name} NOT usable — shell commands (bash/plugin) fail closed.");

    #[cfg(target_os = "linux")]
    {
        if which("bwrap").is_none() {
            println!("             bubblewrap is not installed:");
            println!("               sudo apt install bubblewrap        # Debian/Ubuntu");
            println!("               sudo dnf install bubblewrap        # Fedora");
        } else {
            println!("             bwrap is installed but unprivileged user namespaces are");
            println!("             blocked (Ubuntu 23.10+ restricts them via AppArmor). Enable:");
            println!("               # temporary (until reboot):");
            println!(
                "               sudo sysctl -w kernel.apparmor_restrict_unprivileged_userns=0"
            );
            println!("               # persist across reboots:");
            println!("               echo 'kernel.apparmor_restrict_unprivileged_userns=0' | \\");
            println!("                 sudo tee /etc/sysctl.d/60-flows-userns.conf");
            println!("               sudo sysctl --system");
        }
    }
    #[cfg(target_os = "macos")]
    {
        println!("             Seatbelt uses /usr/bin/sandbox-exec, which ships with macOS.");
        if which("sandbox-exec").is_none() {
            println!("             It was not found on PATH — check /usr/bin/sandbox-exec exists");
            println!("             and that SIP has not been modified.");
        } else {
            println!("             sandbox-exec is present but the probe failed — check Console");
            println!(
                "             for a Seatbelt denial, or that a security tool isn't blocking it."
            );
        }
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        println!(
            "             No sandbox backend for this platform; disable the bash/plugin tools."
        );
    }
}

/// `provider  endpoint  [key]` summary for a resolved config (doctor display).
fn route_summary(llm: &LLMConfig) -> String {
    let provider = if llm.model.to_lowercase().contains("claude") {
        "anthropic"
    } else if !llm.api_base.is_empty() {
        "openai-compatible (local/custom)"
    } else {
        "openai"
    };
    let endpoint = if llm.api_base.is_empty() {
        "(default cloud)"
    } else {
        &llm.api_base
    };
    let key = if llm.api_key.is_empty() {
        "no key"
    } else {
        "key set"
    };
    format!("{provider}  {endpoint}  [{key}]")
}

fn cmd_doctor() -> Result<(), String> {
    println!("flows doctor\n");

    let cfg_path = flows_dir().join("config.yml");
    println!(
        "  config:    {} {}",
        cfg_path.display(),
        if cfg_path.exists() {
            "(found)"
        } else {
            "(missing — using defaults)"
        }
    );

    check_sandbox();

    // Show the resolved model + which provider/endpoint it routes to.
    let cwd = std::env::current_dir().map_err(|e| e.to_string())?;
    let mut global = load_global_config(None).map_err(|e| e.to_string())?;
    if let Some(ov) = load_user_project_override(&cwd, None).map_err(|e| e.to_string())? {
        global = apply_user_project_override(&global, &ov);
    }
    let project = load_project_config(&cwd)
        .map_err(|e| e.to_string())?
        .unwrap_or_else(default_project);
    let llm = merge_llm(&global, &project);
    let provider = if llm.model.to_lowercase().contains("claude") {
        "anthropic"
    } else if !llm.api_base.is_empty() {
        "openai-compatible (local/custom)"
    } else {
        "openai"
    };
    let endpoint = if llm.api_base.is_empty() {
        "(default cloud)"
    } else {
        &llm.api_base
    };
    let key = if llm.api_key.is_empty() {
        "no key"
    } else {
        "key set"
    };
    println!(
        "  model:     {}  →  {provider}  {endpoint}  [{key}]",
        llm.model
    );

    // Named model profiles (and where each routes). Pre-flight each so an
    // unusable one is flagged before a run that would use it.
    let profiles = merge_profiles(&global, &project);
    if !profiles.is_empty() {
        let mut names: Vec<&String> = profiles.keys().collect();
        names.sort();
        println!("  profiles:");
        for name in names {
            let eff = resolve_llm(name, &llm, &profiles);
            let route = route_summary(&eff);
            let status = match preflight_llm(&eff) {
                Ok(()) => String::new(),
                Err(e) => format!("  ⚠ {e}"),
            };
            println!("    {name:<12} {}  →  {route}{status}", eff.model);
        }
    }

    check_web_search(&global);

    let info = detect_stack(&cwd);
    println!(
        "  project:   {} ({})",
        info.language,
        info.detected.join(", ")
    );

    let templates = list_templates(Some(&flows_dir().join("templates")), Some(&cwd));
    println!("  templates: {}", templates.join(", "));
    Ok(())
}

/// Doctor check for the web_search backend. Only `searxng` points at a
/// self-hosted endpoint that can be down — `duckduckgo` (default) hits the public
/// web and needs no local service — so we probe connectivity **only** when
/// searxng is configured, using the same code path a run would.
fn check_web_search(global: &GlobalConfig) {
    let ws = &global.tools.web_search;
    if ws.backend != "searxng" {
        println!("  search:    {} (no connectivity check needed)", ws.backend);
        return;
    }
    let base = ws.base_url.trim_end_matches('/');
    if base.is_empty() {
        println!("  search:    searxng  ⚠ no tools.web_search.base_url configured");
        return;
    }
    let rt = match tokio::runtime::Runtime::new() {
        Ok(rt) => rt,
        Err(e) => {
            println!("  search:    searxng {base}  ⚠ {e}");
            return;
        }
    };
    match rt.block_on(flows_tools::web_search("ping", ws)) {
        Ok(_) => println!("  search:    searxng {base}  (reachable)"),
        Err(e) => println!(
            "  search:    searxng {base}  ⚠ {e}\n             \
             start it (`flows searxng up`), or set \
             tools.web_search.backend: duckduckgo in config.yml"
        ),
    }
}

// ---------------------------------------------------------------------------
// init
// ---------------------------------------------------------------------------

const STARTER_CONFIG: &str = r#"# ~/.flows/config.yml — global defaults (edit me, or use `flows config set`)

# --- default model --------------------------------------------------------
llm:
  # Local llama.cpp / vLLM / any OpenAI-compatible server — no API key needed:
  #   model: qwen2.5-coder
  #   api_base: http://localhost:8080/v1
  #   api_key: local
  #
  # …or a cloud model:
  model: claude-sonnet-4-6
  # api_key: ${ANTHROPIC_API_KEY}    # else falls back to $ANTHROPIC_API_KEY / $OPENAI_API_KEY
  thinking: false                    # extended thinking (if the model supports it)
  # thinking_budget_tokens: 4000

# --- named model profiles -------------------------------------------------
# Route phases to different models: define a profile here, then set `model: <name>`
# on a phase (in a template). `suites` tags it for the eval harness; set
# `supports_tools: false` for models whose server can't emit OpenAI tool_calls.
# models:
#   coder:
#     model: qwen2.5-coder
#     api_base: http://localhost:8080/v1
#     api_key: local
#     suites: [coder]
#   reasoner:
#     model: deepseek-r1
#     api_base: http://localhost:8080/v1
#     supports_tools: false
#     suites: [reasoner]

max_cost_usd: 0.0                    # 0 = no cap (local models are free)

notifications:
  enabled: true
  sound: false

# --- tools ----------------------------------------------------------------
tools:
  web_search:
    backend: duckduckgo              # 'duckduckgo' (default, no setup) or 'searxng'
    max_results: 5
    # For a self-hosted SearXNG (run `flows searxng up`), switch to:
    #   backend: searxng
    #   base_url: http://localhost:8888
    #   api_key: ""                  # only if your instance gates the JSON API
    #
    # Besides web_search, agent phases with the research/implement presets also
    # get `lookup` (crates.io, docs.rs, Stack Exchange, Wikipedia, GitHub) and
    # `http_get` — structured, no setup required.

# --- context delivery (advanced) ------------------------------------------
# How much repo context is fed to the model. All default off (baseline). Turn on
# per need; useful for smaller local models that can't pull context via tools.
# context:
#   exclude: []                      # extra glob(s) to keep out of the repo tree
#   slim_tree: false                 # compact repo tree (fewer tokens)
#   repo_map: false                  # inject a symbol map (tree-sitter) up front
#   repo_map_max_tokens: 1500
#   search_tool: false               # offer a `search_code` (ripgrep) tool
#   range_reads: false               # allow reading line ranges of files
#   retrieval: false                 # local embedding retrieval of relevant chunks
#   retrieval_model: ""              # fastembed model (default all-MiniLM-L6-v2)
#   retrieval_max_chunks: 8         # hard cap on injected chunks
#   retrieval_budget_fraction: 0.25  # share of the model's window for retrieved code
#   retrieval_reindex_threshold: 0.4 # repo churn fraction that forces a reindex
#   budgeting: false                 # trim old tool results when near the window
#   budget_fraction: 0.6
#   unknown_window_tokens: 8000      # assumed window when a model's is unknown
"#;

fn cmd_init(global: bool, template: &str, force: bool) -> Result<(), String> {
    if global {
        // Scaffold a starter config.yml (if absent), then sync templates + prompts.
        let cfg = flows_dir().join("config.yml");
        std::fs::create_dir_all(flows_dir()).map_err(|e| e.to_string())?;
        if cfg.exists() && !force {
            println!("config:   {} (exists — left unchanged)", cfg.display());
        } else {
            std::fs::write(&cfg, STARTER_CONFIG).map_err(|e| e.to_string())?;
            println!("config:   wrote {}", cfg.display());
        }
        return cmd_templates_sync(force);
    }
    let cwd = std::env::current_dir().map_err(|e| e.to_string())?;
    let target = cwd.join(".flows.yml");
    if target.exists() && !force {
        return Err(format!(
            "{} already exists (use --force to overwrite)",
            target.display()
        ));
    }
    let info = detect_stack(&cwd);
    let yaml = generate_config(&info, template);
    std::fs::write(&target, yaml).map_err(|e| e.to_string())?;
    println!(
        "Wrote {} (detected: {})",
        target.display(),
        info.detected.join(", ")
    );
    Ok(())
}

// ---------------------------------------------------------------------------
// templates
// ---------------------------------------------------------------------------

fn cmd_templates_list() -> Result<(), String> {
    let cwd = std::env::current_dir().ok();
    let names = list_templates(Some(&flows_dir().join("templates")), cwd.as_deref());
    for n in names {
        println!("{n}");
    }
    Ok(())
}

fn cmd_templates_sync(force: bool) -> Result<(), String> {
    let dir = flows_dir().join("templates");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let (mut written, mut skipped) = (0, 0);
    for (name, contents) in builtin_files() {
        let path = dir.join(&name);
        if path.exists() && !force {
            skipped += 1;
            continue;
        }
        std::fs::write(&path, contents).map_err(|e| e.to_string())?;
        written += 1;
    }
    println!(
        "Synced templates to {} ({written} written, {skipped} skipped)",
        dir.display()
    );

    // Also sync the agent prompts so users can customize them.
    let pdir = flows_dir().join("prompts");
    let (mut pw, mut ps) = (0, 0);
    for (rel, contents) in flows_runner::builtin_prompt_files() {
        let path = pdir.join(&rel);
        if path.exists() && !force {
            ps += 1;
            continue;
        }
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        std::fs::write(&path, contents).map_err(|e| e.to_string())?;
        pw += 1;
    }
    println!(
        "Synced prompts to {} ({pw} written, {ps} skipped)",
        pdir.display()
    );

    if (skipped > 0 || ps > 0) && !force {
        println!("  (some already present — use --force to overwrite)");
    }
    Ok(())
}

/// A heavily-commented starter template — one of each phase type, every common
/// field annotated, so a new author can learn the format by editing it.
const SKELETON_TEMPLATE: &str = r#"# ~/.flows/templates/__NAME__.yml — a flows template.
#
# A template is a pipeline of phases, run top to bottom. Run this one with:
#   flows run "your idea here" --template __NAME__
# Preview it first (no LLM calls):
#   flows plan "your idea here" --template __NAME__
#
# Full reference: docs/templates.md. Delete the phases you don't want.
template: __NAME__

# Optional: pin a model for this whole template. Omit to use ~/.flows/config.yml.
# llm:
#   model: qwen3-coder

phases:
  # --- agent: an LLM tool loop over your repo --------------------------------
  - name: plan
    type: agent
    tools: research_ro            # read-only: read_file/list_files + web + lookup, no writes
    max_clarification_rounds: 3   # questions it may ask YOU (0 = never ask; hands-off)
    # model: reasoner             # per-phase model override (a profile from `models:`)
    # prompt: __NAME__/plan.md    # custom prompt file (else a sensible builtin is used)

  # --- human: a review gate; Enter approves, or type feedback ----------------
  - name: approve_plan
    type: human
    instructions: "Review the plan. Press Enter to approve, or type feedback to revise."
    loop_back_to: plan            # feedback jumps back to `plan`
    loop_condition: "content.get('answer', '').strip() not in ('', 'ok', 'yes', 'approve', 'approved')"
    max_loop_iterations: 5

  # --- agent that writes code -------------------------------------------------
  - name: implement
    type: agent
    tools: research_and_code      # write files + build/test bash + web_search + lookup
    max_clarification_rounds: 0   # hands-off: don't ask, just build
    max_tool_rounds: 200          # room for a long build/edit/fix loop
    on_failure: stop              # stop | continue | retry:<phase>

  # --- script: a trusted shell command (no LLM, output captured) -------------
  - name: test
    type: script
    command: "@auto:test"         # auto-detects: make test / cargo test / pytest / npm / go
    on_failure: retry:implement   # a real failure loops back to implement

  # --- final human gate -------------------------------------------------------
  - name: approve
    type: human
    instructions: "Review the result. Press Enter to accept, or type feedback."
    loop_back_to: implement
    loop_condition: "content.get('answer', '').strip() not in ('', 'ok', 'yes', 'approve', 'approved')"
    max_loop_iterations: 5

  # --- commit: {{ idea }} is the run's idea text -----------------------------
  - name: commit
    type: script
    command: "git add -A && git commit -m 'feat: {{ idea[:60] }}'"
    on_failure: continue
"#;

fn cmd_templates_new(name: &str) -> Result<(), String> {
    let name = name.trim();
    if name.is_empty()
        || !name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        return Err("template name must be alphanumeric (dashes/underscores allowed)".into());
    }
    let dir = flows_dir().join("templates");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let path = dir.join(format!("{name}.yml"));
    if path.exists() {
        return Err(format!(
            "{} already exists — pick another name or edit it directly",
            path.display()
        ));
    }
    std::fs::write(&path, SKELETON_TEMPLATE.replace("__NAME__", name))
        .map_err(|e| e.to_string())?;
    println!("Wrote {}", path.display());
    println!("Edit it, then:  flows run \"your idea\" --template {name}");
    Ok(())
}

// ---------------------------------------------------------------------------
// plan (dry-run)
// ---------------------------------------------------------------------------

/// Pre-flight: a run needs a model and a way to reach it. Returns a clear,
/// actionable error rather than letting the LLM call fail with a cryptic 401.
fn preflight_llm(llm: &flows_core::models::LLMConfig) -> Result<(), String> {
    if llm.model.is_empty() {
        return Err("no model configured — run `flows config set model <name>` \
(e.g. claude-sonnet-4-6, or your llama.cpp model)"
            .into());
    }
    // A custom api_base (e.g. a local llama.cpp server) needs no API key.
    if llm.api_key.is_empty() && llm.api_base.is_empty() {
        return Err(
            "no model endpoint — set a cloud key (ANTHROPIC_API_KEY / OPENAI_API_KEY, \
or `flows config set api-key`) or a local server (`flows config set api-base \
http://localhost:8080/v1`)"
                .into(),
        );
    }
    Ok(())
}

/// Pre-flight every distinct profile a run will actually use: the run default
/// plus each agent phase's resolved selection. Errors name the offending phase
/// so a bad profile is easy to find.
fn preflight_run(
    llm: &LLMConfig,
    profiles: &HashMap<String, LLMConfig>,
    phases: &[PhaseConfig],
) -> Result<(), String> {
    // The run default (a phase with no `model:` falls back to this).
    preflight_llm(&resolve_llm("", llm, profiles))?;
    for phase in phases {
        if !matches!(phase.phase_type, PhaseType::Agent) {
            continue;
        }
        let sel = phase.model.as_deref().unwrap_or("");
        if sel.is_empty() {
            continue; // uses the already-validated default
        }
        let eff = resolve_llm(sel, llm, profiles);
        preflight_llm(&eff).map_err(|e| format!("phase {:?} (model {sel:?}): {e}", phase.name))?;
    }
    Ok(())
}

fn resolve(
    repo: &Path,
    template_flag: Option<&str>,
) -> Result<(Vec<PhaseConfig>, GlobalConfig, ProjectConfig, String), String> {
    let mut global = load_global_config(None).map_err(|e| e.to_string())?;
    if let Some(ov) = load_user_project_override(repo, None).map_err(|e| e.to_string())? {
        global = apply_user_project_override(&global, &ov);
    }
    let project = load_project_config(repo)
        .map_err(|e| e.to_string())?
        .unwrap_or_else(default_project);

    let template_name = template_flag
        .map(str::to_string)
        .filter(|s| !s.is_empty())
        .or_else(|| (!project.template.is_empty()).then(|| project.template.clone()))
        .unwrap_or_else(|| "feature".to_string());

    let phases = if !project.phases.is_empty() {
        project.phases.clone()
    } else {
        load_template(Some(&template_name), repo, None)
            .map_err(|e| e.to_string())?
            .0
    };

    Ok((phases, global, project, template_name))
}

fn default_project() -> ProjectConfig {
    // No .flows.yml → an *empty* project llm (model = "") so merge_llm falls
    // through to the global config. LLMConfig::default() has model =
    // "claude-sonnet-4-6", which would wrongly override the global model.
    ProjectConfig {
        llm: LLMConfig {
            model: String::new(),
            ..Default::default()
        },
        models: Default::default(),
        phases: vec![],
        template: String::new(),
        context: Default::default(),
        context_keys: Vec::new(),
    }
}

/// Resolve the target directory: `-C/--directory` if given (canonicalized, must be
/// an existing directory), else the current working directory.
fn resolve_repo_dir(dir: Option<&str>) -> Result<PathBuf, String> {
    match dir {
        None => std::env::current_dir().map_err(|e| e.to_string()),
        Some(p) => {
            let path = std::fs::canonicalize(p).map_err(|e| format!("-C {p:?}: {e}"))?;
            if !path.is_dir() {
                return Err(format!("-C {p:?} is not a directory"));
            }
            Ok(path)
        }
    }
}

fn cmd_plan(idea: &str, template: Option<&str>, repo: Option<&str>) -> Result<(), String> {
    if idea.trim().is_empty() {
        return Err("provide an idea, e.g. `flows plan \"add a feature\"`".into());
    }
    let cwd = resolve_repo_dir(repo)?;
    let (phases, global, project, template_name) = resolve(&cwd, template)?;

    let report = dry_run(idea, &phases, &template_name, &global, &project, &cwd);

    println!("Template: {template_name}\n");
    println!("Execution plan:");
    for line in &report.plan_lines {
        println!("{line}");
    }

    println!("\nCost estimate:");
    for e in &report.estimates {
        if e.model.is_empty() {
            println!("  {:<20} [{}]  —", e.phase, e.phase_type.as_str());
        } else {
            println!(
                "  {:<20} {}  {}–{}",
                e.phase,
                e.model,
                format_cost(e.cost_low),
                format_cost(e.cost_high)
            );
        }
    }
    println!(
        "\nTotal: {}–{}",
        format_cost(report.total_cost_low),
        format_cost(report.total_cost_high)
    );

    if !report.errors.is_empty() {
        println!("\nValidation errors:");
        for err in &report.errors {
            println!("  - {err}");
        }
        return Err("pipeline has validation errors".into());
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// run
// ---------------------------------------------------------------------------

struct Foreground;

fn read_line(prompt: &str) -> String {
    use std::io::Write;
    print!("{prompt} ");
    let _ = std::io::stdout().flush();
    let mut line = String::new();
    let _ = std::io::stdin().read_line(&mut line);
    line.trim().to_string()
}

#[async_trait]
impl RunnerCallbacks for Foreground {
    async fn on_questions(&self, _phase: &str, questions: &[String]) -> String {
        for q in questions {
            println!("  ? {q}");
        }
        read_line("  your answer >")
    }
    async fn on_human_gate(&self, _phase: &str, instructions: &str) -> String {
        println!("  ⏸ {instructions}");
        read_line("  press Enter to accept, or type feedback >")
    }
    fn on_phase_start(&self, phase: &str, index: usize, total: usize) {
        println!("\n▶ [{}/{}] {phase}", index + 1, total);
    }
    fn on_status(&self, _phase: &str, message: &str) {
        println!("    {message}");
    }
    fn on_token(&self, _phase: &str, token: &str) {
        use std::io::Write;
        print!("{token}");
        let _ = std::io::stdout().flush();
    }
    fn on_tool_call(&self, _phase: &str, _name: &str, summary: &str) {
        println!("    ⚙ {summary}");
    }
    fn on_tool_result(&self, _phase: &str, _name: &str, summary: &str, is_error: bool) {
        println!("    {} {summary}", if is_error { "✗" } else { "✓" });
    }
    fn on_script_result(&self, _phase: &str, exit_code: i32, _stdout: &str, _stderr: &str) {
        println!("    script exited {exit_code}");
    }
    fn on_cost(&self, total_usd: f64) {
        println!("    cost so far: {}", format_cost(total_usd));
    }
    fn on_loop_back(&self, from: &str, to: &str, iteration: u32, max: u32) {
        println!("    ↻ {from} → {to} (iteration {iteration}/{max})");
    }
    fn on_phase_end(&self, phase: &str, _artifact: Option<&Artifact>) {
        println!("  ✓ {phase} done");
    }
    fn on_phase_skipped(&self, phase: &str, reason: &str) {
        println!("  ⊘ {phase} skipped — {reason}");
    }
    fn on_run_complete(&self, _run_id: &str, phases: &[String]) {
        println!("\n✓ complete: {}", phases.join(" → "));
    }
    fn on_run_failed(&self, _run_id: &str, phase: &str, error: &str) {
        eprintln!("\n✗ failed at {phase}: {error}");
    }
}

/// Background-run callbacks: publish questions / human gates to `state.json`
/// and block until `attach` (or `flows`) sends an answer / advance signal, so a
/// detached run pauses for input instead of silently auto-proceeding.
struct Waiting {
    store: Store,
    run_id: String,
    notify: bool,
    sound: bool,
}

impl Waiting {
    /// Publish a pause into the run state so `attach` can display + answer it.
    fn publish(&self, status: RunStatus, prompts: Vec<String>) {
        if let Ok(Some(mut st)) = self.store.read_state(&self.run_id) {
            st.status = status;
            st.questions = prompts;
            let _ = self.store.write_state(&st);
        }
    }
    fn resume_running(&self) {
        if let Ok(Some(mut st)) = self.store.read_state(&self.run_id) {
            st.status = RunStatus::Running;
            st.questions = vec![];
            let _ = self.store.write_state(&st);
        }
    }
    fn ping(&self, title: &str, body: &str) {
        if self.notify {
            flows_core::notify::notify(title, body, self.sound);
        }
    }

    /// Poll the run's signal files until the gate is answered. Peeks the cancel
    /// signal (leaving it for the runner to consume + cancel), returns a typed
    /// answer through `map_answer`, and — when `accept_advance` — treats a bare
    /// advance as "approved". Shared by `on_questions` and `on_human_gate`.
    async fn wait_for_reply(
        &self,
        accept_advance: bool,
        map_answer: impl Fn(String) -> String,
    ) -> String {
        loop {
            if self.store.is_cancelled(&self.run_id) {
                return String::new();
            }
            if let Some(answer) = self.store.read_answer(&self.run_id) {
                let _ = self.store.poll_advance(&self.run_id); // clear paired advance
                self.resume_running();
                return map_answer(answer);
            }
            if accept_advance && self.store.poll_advance(&self.run_id) {
                self.resume_running();
                return "approved".into();
            }
            tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        }
    }

    /// Append streamed output to `<run_dir>/output.log` so `flows attach` can
    /// tail it live (a backgrounded worker has no terminal of its own). Written
    /// as JSONL `{"c": category, "x": text}` so the reader can restore the
    /// thinking/normal/tool categories (and their toggles). `c`: n=normal,
    /// t=thinking, j=json, o=tool, s=system (toggled), u=user turn, m=meta (task
    /// / phase markers / scripts, always visible). Non-display metrics for the
    /// status bar: ctx=input tokens, cost=usd total, cw=context window.
    fn emit(&self, cat: &str, text: &str) {
        use std::io::Write;
        let line = serde_json::json!({ "c": cat, "x": text }).to_string();
        let path = self.store.run_dir(&self.run_id).join("output.log");
        if let Ok(mut f) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
        {
            let _ = writeln!(f, "{line}");
        }
    }
}

#[async_trait]
impl RunnerCallbacks for Waiting {
    async fn on_questions(&self, _p: &str, q: &[String]) -> String {
        self.publish(RunStatus::WaitingInput, q.to_vec());
        self.ping("flows: needs input", &q.join("  •  "));
        // A question needs a typed answer; a bare advance is not an accept here.
        self.wait_for_reply(false, |answer| answer).await
    }

    async fn on_human_gate(&self, _p: &str, instructions: &str) -> String {
        self.publish(RunStatus::WaitingApproval, vec![instructions.to_string()]);
        self.ping("flows: approval needed", instructions);
        // A bare advance approves; typed feedback is captured so the gate's loop
        // condition sees it; empty answer means approve.
        self.wait_for_reply(true, |answer| {
            if answer.is_empty() {
                "approved".into()
            } else {
                answer
            }
        })
        .await
    }

    // -- live output streamed to output.log (tailed by `flows attach`) --------
    // Phase markers use "m" (Meta, always visible); the system prompt uses "s"
    // (System, hidden behind the `s` toggle).
    fn on_system(&self, _phase: &str, prompt: &str) {
        self.emit("s", prompt);
    }
    fn on_user(&self, _phase: &str, message: &str) {
        self.emit("u", message);
    }
    fn on_note(&self, _phase: &str, text: &str) {
        self.emit("m", text);
    }
    fn on_context(&self, input_tokens: u64) {
        self.emit("ctx", &input_tokens.to_string());
    }
    fn on_round(&self, round: u32, total: u32) {
        self.emit("round", &format!("{round}/{total}"));
    }
    fn on_cost(&self, total_usd: f64) {
        self.emit("cost", &total_usd.to_string());
    }
    fn on_phase_start(&self, phase: &str, index: usize, total: usize) {
        self.emit(
            "m",
            &format!("━━ phase {}/{}: {phase} ━━", index + 1, total),
        );
    }
    fn on_phase_skipped(&self, phase: &str, reason: &str) {
        self.emit("m", &format!("⊘ {phase} skipped — {reason}"));
    }
    fn on_token(&self, _phase: &str, token: &str) {
        self.emit("n", token);
    }
    fn on_json(&self, _phase: &str, token: &str) {
        self.emit("j", token);
    }
    fn on_thinking(&self, _phase: &str, text: &str) {
        self.emit("t", text);
    }
    fn on_tool_call(&self, _phase: &str, name: &str, summary: &str) {
        self.emit("o", &format!("⚙ {name} {summary}"));
    }
    fn on_tool_result(&self, _phase: &str, name: &str, summary: &str, is_error: bool) {
        let glyph = if is_error { '✗' } else { '✓' };
        self.emit("o", &format!("{glyph} {name} {summary}"));
    }
    fn on_script_result(&self, _phase: &str, exit_code: i32, stdout: &str, _stderr: &str) {
        self.emit("m", &format!("script → exit {exit_code}\n{stdout}"));
    }
}

fn cmd_home() -> Result<(), String> {
    let cwd = std::env::current_dir().map_err(|e| e.to_string())?;
    let store = Store::new(flows_dir());
    let templates = list_templates(Some(&flows_dir().join("templates")), Some(&cwd));

    // Pre-flight before opening the TUI, so a misconfigured first run gets clear
    // guidance instead of a cryptic failure inside the attach view.
    let (_, global, project, _) = resolve(&cwd, None)?;
    preflight_llm(&merge_llm(&global, &project))?;

    // Launcher loop: a New run spawns a *background worker* and attaches to it, so
    // the running job and a later reattach are the exact same view (and the
    // worker streams to output.log for the live pane).
    loop {
        match flows_tui::home_screen(&templates, &store).map_err(|e| e.to_string())? {
            flows_tui::HomeAction::Quit => break,
            flows_tui::HomeAction::Attach { run_id } => {
                flows_tui::attach_tui(store.clone(), run_id).map_err(|e| e.to_string())?;
            }
            flows_tui::HomeAction::StartRun { template, idea } => {
                let run_id = uuid::Uuid::new_v4().to_string();
                spawn_background(&run_id, &idea, &template, &cwd, &store, false, None)?;
                flows_tui::attach_tui(store.clone(), run_id).map_err(|e| e.to_string())?;
            }
            flows_tui::HomeAction::Plan { template, idea } => {
                // Dry run (no LLM): preview the plan + cost, then optionally start.
                let (phases, global, project, tname) = resolve(&cwd, Some(&template))?;
                let report = dry_run(&idea, &phases, &tname, &global, &project, &cwd);
                let start =
                    flows_tui::plan_screen(&report, &tname, &idea).map_err(|e| e.to_string())?;
                if start {
                    let run_id = uuid::Uuid::new_v4().to_string();
                    spawn_background(&run_id, &idea, &tname, &cwd, &store, false, None)?;
                    flows_tui::attach_tui(store.clone(), run_id).map_err(|e| e.to_string())?;
                }
            }
        }
    }
    Ok(())
}

fn cmd_run(
    idea: &str,
    template: Option<&str>,
    repo: Option<&str>,
    tui: bool,
    background: bool,
    context: Option<&str>,
) -> Result<(), String> {
    if idea.trim().is_empty() {
        return Err("provide an idea, e.g. `flows run \"add a feature\"`".into());
    }
    let cwd = resolve_repo_dir(repo)?;
    let (phases, global, mut project, template_name) = resolve(&cwd, template)?;
    if let Some(spec) = context {
        apply_context_override(&global, &mut project, spec)?;
    }

    let llm = merge_llm(&global, &project);
    let profiles = merge_profiles(&global, &project);
    preflight_run(&llm, &profiles, &phases)?;
    let store = Store::new(flows_dir());
    let run_id = uuid::Uuid::new_v4().to_string();

    if background {
        return spawn_background(&run_id, idea, &template_name, &cwd, &store, true, context);
    }

    if tui {
        // Run as a detached background worker and attach the TUI to it — the same
        // path the home screen uses. This way quitting the view (`q`) only detaches;
        // the run keeps going and can be re-attached (`flows attach <id>`) or stopped
        // (`flows stop <id>`). Quitting no longer kills the run.
        spawn_background(&run_id, idea, &template_name, &cwd, &store, false, context)?;
        flows_tui::attach_tui(store.clone(), run_id.clone()).map_err(|e| e.to_string())?;
        // After detaching, tell the user how to get back / stop it (unless it
        // already finished).
        if let Ok(Some(st)) = store.read_state(&run_id) {
            if !matches!(st.status.as_str(), "completed" | "failed" | "cancelled") {
                println!("run {run_id} still running in the background");
                println!("  reattach:  flows attach {run_id}");
                println!("  stop:      flows stop {run_id}");
            }
        }
        return Ok(());
    }

    println!("Run {run_id}  (template: {template_name})");
    let rt = tokio::runtime::Runtime::new().map_err(|e| e.to_string())?;
    let result = rt.block_on(run_pipeline(
        &run_id,
        idea,
        &cwd,
        &phases,
        &template_name,
        &global,
        &project,
        &default_provider_factory,
        &Foreground,
        &store,
        None,
        None,
    ));
    notify_run_result(&global, &run_id, idea, &result);
    result.map(|_| ()).map_err(|e| e.to_string())
}

// ---------------------------------------------------------------------------
// background worker + runs listing
// ---------------------------------------------------------------------------

fn spawn_background(
    run_id: &str,
    idea: &str,
    template_name: &str,
    repo: &std::path::Path,
    store: &Store,
    announce: bool,
    context: Option<&str>,
) -> Result<(), String> {
    use std::os::unix::process::CommandExt;
    use std::process::{Command, Stdio};

    // Pre-create the run dir so we can point the worker's stdio at a log file.
    store.ensure_run_dir(run_id).map_err(|e| e.to_string())?;

    // Pre-write meta + state so an immediate `attach` finds the run (the worker
    // overwrites both with the full record when it starts).
    let (phases, _g, _p, tname) = resolve(repo, Some(template_name))?;
    let meta = flows_core::models::RunMeta {
        run_id: run_id.into(),
        idea: idea.into(),
        slug: String::new(),
        repo_path: repo.display().to_string(),
        template: tname,
        phases: phases
            .iter()
            .filter_map(|p| serde_json::to_value(p).ok())
            .collect(),
        created_at: String::new(),
        pid: None,
    };
    let _ = store.write_meta(&meta);
    let _ = store.write_state(&flows_core::models::RunState {
        run_id: run_id.into(),
        status: RunStatus::Running,
        current_phase: phases.first().map(|p| p.name.clone()).unwrap_or_default(),
        current_phase_index: 0,
        round: 0,
        questions: vec![],
        error: None,
        completed_phases: vec![],
        skipped_phases: vec![],
    });

    let log = store.run_dir(run_id).join("worker.log");
    let log_file = std::fs::File::create(&log).map_err(|e| e.to_string())?;
    let err_file = log_file.try_clone().map_err(|e| e.to_string())?;

    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let mut cmd = Command::new(exe);
    if let Some(spec) = context {
        cmd.env("FLOWS_CONTEXT", spec);
    }
    let child = cmd
        .arg("worker")
        .arg(run_id)
        .env("FLOWS_IDEA", idea)
        .env("FLOWS_TEMPLATE", template_name)
        .env("FLOWS_REPO", repo)
        .stdin(Stdio::null())
        .stdout(Stdio::from(log_file))
        .stderr(Stdio::from(err_file))
        .process_group(0) // detach from the parent's job control
        .spawn()
        .map_err(|e| e.to_string())?;

    // Record the worker's pid so `flows stop`/`delete --force` can actually kill it
    // (a worker deep in an LLM call may not poll the cancel signal promptly).
    let mut meta = meta;
    meta.pid = Some(child.id() as i32);
    let _ = store.write_meta(&meta);

    if announce {
        println!("Started background run {run_id}");
        println!("  track with: flows runs   (log: {})", log.display());
    }
    Ok(())
}

fn cmd_worker(run_id: &str) -> Result<(), String> {
    let idea = std::env::var("FLOWS_IDEA").unwrap_or_default();
    let template = std::env::var("FLOWS_TEMPLATE").ok();
    let repo = PathBuf::from(std::env::var("FLOWS_REPO").map_err(|_| "FLOWS_REPO not set")?);

    let (phases, global, mut project, template_name) = resolve(&repo, template.as_deref())?;
    if let Ok(spec) = std::env::var("FLOWS_CONTEXT") {
        apply_context_override(&global, &mut project, &spec)?;
    }
    let store = Store::new(flows_dir());

    let waiting = Waiting {
        store: store.clone(),
        run_id: run_id.to_string(),
        notify: global.notifications.enabled,
        sound: global.notifications.sound,
    };
    // Echo the task first so `attach` shows what the run was asked to do, even
    // before the model produces any tokens (matches the live `run_tui` path).
    waiting.emit("m", &format!("▸ task: {idea}"));
    // Publish the context window once so `attach` can show token usage as a
    // percentage (the default model's window; per-phase models may differ).
    let model = flows_core::config::merge_llm(&global, &project).model;
    if let Some(w) = flows_core::pricing::context_window(&model) {
        waiting.emit("cw", &w.to_string());
    }
    let rt = tokio::runtime::Runtime::new().map_err(|e| e.to_string())?;
    let result = rt.block_on(run_pipeline(
        run_id,
        &idea,
        &repo,
        &phases,
        &template_name,
        &global,
        &project,
        &default_provider_factory,
        &waiting,
        &store,
        None,
        None,
    ));

    notify_run_result(&global, run_id, &idea, &result);
    result.map(|_| ()).map_err(|e| e.to_string())
}

/// Fire a desktop notification for a finished run, if enabled.
fn notify_run_result(
    global: &GlobalConfig,
    run_id: &str,
    idea: &str,
    result: &Result<Vec<String>, flows_runner::RunError>,
) {
    if !global.notifications.enabled {
        return;
    }
    let sound = global.notifications.sound;
    let short: String = run_id.chars().take(8).collect();
    match result {
        Ok(_) => flows_core::notify::notify("flows: run complete", idea, sound),
        Err(e) => flows_core::notify::notify(
            &format!("flows: run failed ({short})"),
            &e.to_string(),
            sound,
        ),
    }
}

// ---------------------------------------------------------------------------
// config
// ---------------------------------------------------------------------------

fn cmd_config_show() -> Result<(), String> {
    let cwd = std::env::current_dir().map_err(|e| e.to_string())?;
    let mut global = load_global_config(None).map_err(|e| e.to_string())?;
    if let Some(ov) = load_user_project_override(&cwd, None).map_err(|e| e.to_string())? {
        global = apply_user_project_override(&global, &ov);
    }
    let project = load_project_config(&cwd)
        .map_err(|e| e.to_string())?
        .unwrap_or_else(default_project);
    let llm = merge_llm(&global, &project);

    println!("config file:  {}", flows_dir().join("config.yml").display());
    println!("model:        {}", llm.model);
    println!(
        "api_base:     {}",
        if llm.api_base.is_empty() {
            "(default cloud)"
        } else {
            &llm.api_base
        }
    );
    println!(
        "api_key:      {}",
        if llm.api_key.is_empty() {
            "(not set)"
        } else {
            "(set)"
        }
    );
    println!(
        "thinking:     {}",
        llm.thinking
            .map(|b| b.to_string())
            .unwrap_or_else(|| "(default)".into())
    );
    println!("max_cost_usd: {}", global.max_cost_usd);
    println!("notifications: {}", global.notifications.enabled);
    println!("web_search:   {}", global.tools.web_search.backend);
    Ok(())
}

fn cmd_config_set(key: &str, value: &str) -> Result<(), String> {
    use serde_yaml_ng::Value;

    let path = flows_dir().join("config.yml");
    let mut root: Value = if path.exists() {
        let text = std::fs::read_to_string(&path).map_err(|e| e.to_string())?;
        serde_yaml_ng::from_str(&text).unwrap_or(Value::Mapping(Default::default()))
    } else {
        Value::Mapping(Default::default())
    };
    if !root.is_mapping() {
        root = Value::Mapping(Default::default());
    }
    let map = root.as_mapping_mut().unwrap();

    let set_llm = |map: &mut serde_yaml_ng::Mapping, field: &str, v: Value| {
        let llm_key = Value::String("llm".into());
        if !map.contains_key(&llm_key) {
            map.insert(llm_key.clone(), Value::Mapping(Default::default()));
        }
        map.get_mut(&llm_key)
            .and_then(Value::as_mapping_mut)
            .unwrap()
            .insert(Value::String(field.into()), v);
    };

    match key {
        "model" => set_llm(map, "model", Value::String(value.into())),
        "api-base" | "api_base" => set_llm(map, "api_base", Value::String(value.into())),
        "api-key" | "api_key" => set_llm(map, "api_key", Value::String(value.into())),
        "max-cost" | "max_cost" | "max_cost_usd" => {
            let n: f64 = value
                .parse()
                .map_err(|_| format!("max-cost must be a number, got {value:?}"))?;
            map.insert(
                Value::String("max_cost_usd".into()),
                Value::Number(n.into()),
            );
        }
        other => {
            return Err(format!(
                "unknown config key {other:?} (valid: model, api-base, api-key, max-cost)"
            ))
        }
    }

    std::fs::create_dir_all(flows_dir()).map_err(|e| e.to_string())?;
    let yaml = serde_yaml_ng::to_string(&root).map_err(|e| e.to_string())?;
    std::fs::write(&path, yaml).map_err(|e| e.to_string())?;
    println!("set {key} = {value}  ({})", path.display());
    Ok(())
}

fn cmd_usage(run_id: &str) -> Result<(), String> {
    let store = Store::new(flows_dir());
    let run_id = &resolve_run_id(&store, run_id).unwrap_or_else(|_| run_id.to_string());
    let log = flows_core::llm_log::log_path(&store.run_dir(run_id));
    let report = flows_core::usage::parse_usage(&log, run_id, "");
    if report.phases.is_empty() {
        println!("no usage recorded for {run_id} (run not started, or no LLM calls yet)");
        return Ok(());
    }
    println!(
        "{:<16} {:<22} {:>10} {:>10} {:>12}",
        "phase", "model", "in_tok", "out_tok", "cost"
    );
    for p in &report.phases {
        let model = if p.model.is_empty() { "—" } else { &p.model };
        println!(
            "{:<16} {:<22} {:>10} {:>10} {:>12}",
            p.phase,
            model,
            p.input_tokens,
            p.output_tokens,
            format_cost(p.cost_usd)
        );
    }
    println!(
        "{:<16} {:<22} {:>10} {:>10} {:>12}",
        "TOTAL",
        "",
        report.total_input(),
        report.total_output(),
        format_cost(report.total_cost())
    );
    if report.any_unknown_pricing() {
        println!("(some models have unknown pricing → cost shown as $0)");
    }
    Ok(())
}

/// Resolve a run id or unique **prefix** (the short ids shown by `flows runs`)
/// to a full run id.
fn resolve_run_id(store: &Store, partial: &str) -> Result<String, String> {
    if store.run_dir(partial).exists() {
        return Ok(partial.to_string());
    }
    let runs_dir = match store.runs_dir() {
        Ok(d) => d,
        Err(_) => return Err(format!("no such run: {partial}")),
    };
    let mut matches: Vec<String> = std::fs::read_dir(&runs_dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| e.file_name().to_str().map(String::from))
        .filter(|name| name.starts_with(partial))
        .collect();
    match matches.len() {
        0 => Err(format!("no such run: {partial}")),
        1 => Ok(matches.remove(0)),
        n => Err(format!(
            "ambiguous run id {partial:?} — matches {n} runs; use more characters"
        )),
    }
}

fn cmd_delete(run_id: &str, force: bool) -> Result<(), String> {
    let store = Store::new(flows_dir());
    let run_id = &resolve_run_id(&store, run_id)?;
    // Unreadable state → treat as active (don't skip stopping it).
    let status = store.run_status(run_id);
    let active = status.map_or(true, |s| s.is_active());

    if active && !force {
        let label = status.map_or_else(|| "unknown".to_string(), |s| s.to_string());
        return Err(format!(
            "run {run_id} is still active ({label}) — use `flows delete {run_id} --force` to stop and delete it"
        ));
    }
    if active && force {
        // Stop the worker (signal + kill) before deleting so it isn't writing into
        // a directory we're removing.
        stop_active_run(&store, run_id);
    }
    store.delete_run(run_id).map_err(|e| e.to_string())?;
    println!("deleted run {run_id}");
    Ok(())
}

/// Stop a still-active run: send the cancel signal, then SIGTERM (and SIGKILL as a
/// fallback) the worker pid if we recorded one — a worker mid-LLM-call may not poll
/// the cancel promptly. Returns whether the run was active.
fn stop_active_run(store: &Store, run_id: &str) -> bool {
    // Unreadable state → treat as active (don't skip stopping it).
    let active = store.run_status(run_id).map_or(true, |s| s.is_active());
    if !active {
        return false;
    }
    let _ = store.send_cancel(run_id);
    match store.read_meta(run_id).ok().and_then(|m| m.pid) {
        Some(pid) if pid > 0 => {
            #[cfg(unix)]
            {
                use nix::sys::signal::{kill, Signal};
                use nix::unistd::Pid;
                let pid = Pid::from_raw(pid);
                let _ = kill(pid, Signal::SIGTERM);
                std::thread::sleep(std::time::Duration::from_millis(400));
                // Still alive? (a `None` signal just probes existence) → SIGKILL.
                if kill(pid, None).is_ok() {
                    let _ = kill(pid, Signal::SIGKILL);
                }
            }
            #[cfg(not(unix))]
            std::thread::sleep(std::time::Duration::from_millis(400));
        }
        // No pid recorded (older run): fall back to the cancel signal + grace.
        _ => std::thread::sleep(std::time::Duration::from_millis(300)),
    }
    true
}

fn cmd_stop(run_id: &str) -> Result<(), String> {
    let store = Store::new(flows_dir());
    let run_id = &resolve_run_id(&store, run_id)?;
    if !stop_active_run(&store, run_id) {
        println!("run {run_id} is not active — nothing to stop");
        return Ok(());
    }
    // Reflect the stop in state so listings/attach show it (the worker may have been
    // killed before it could update its own status).
    if let Ok(Some(mut st)) = store.read_state(run_id) {
        st.status = RunStatus::Cancelled;
        st.questions = vec![];
        let _ = store.write_state(&st);
    }
    println!("stopped run {run_id}");
    Ok(())
}

fn cmd_clean(all: bool) -> Result<(), String> {
    let store = Store::new(flows_dir());
    let (mut removed, mut kept) = (0usize, 0usize);
    for m in store.list_runs() {
        let finished = store.run_status(&m.run_id).is_some_and(|s| s.is_finished());
        if all || finished {
            let _ = store.delete_run(&m.run_id);
            removed += 1;
        } else {
            kept += 1;
        }
    }
    if kept > 0 {
        println!(
            "removed {removed} run(s); kept {kept} still running (use --all to remove those too)"
        );
    } else {
        println!("removed {removed} run(s)");
    }
    Ok(())
}

fn cmd_runs() -> Result<(), String> {
    let store = Store::new(flows_dir());
    let runs = store.list_runs();
    if runs.is_empty() {
        println!("(no runs yet)");
        return Ok(());
    }
    println!("{:<8}  {:<16}  {:>9}  idea", "id", "status", "cost");
    for meta in runs {
        let status = store
            .run_status(&meta.run_id)
            .map_or_else(|| "?".to_string(), |s| s.to_string());
        let log = flows_core::llm_log::log_path(&store.run_dir(&meta.run_id));
        let cost = flows_core::usage::parse_usage(&log, &meta.run_id, "").total_cost();
        let cost_s = if cost > 0.0 {
            format_cost(cost)
        } else {
            "—".into()
        };
        let short: String = meta.run_id.chars().take(8).collect();
        let idea: String = meta.idea.chars().take(50).collect();
        println!("{short:<8}  {status:<16}  {cost_s:>9}  {idea}");
    }
    Ok(())
}

#[cfg(test)]
mod preflight_tests {
    use super::preflight_llm;
    use flows_core::models::LLMConfig;

    #[test]
    fn empty_model_errors() {
        let llm = LLMConfig {
            model: String::new(),
            ..Default::default()
        };
        assert!(preflight_llm(&llm).unwrap_err().contains("no model"));
    }

    #[test]
    fn model_without_endpoint_errors() {
        let llm = LLMConfig {
            model: "claude-x".into(),
            api_key: String::new(),
            api_base: String::new(),
            ..Default::default()
        };
        assert!(preflight_llm(&llm).unwrap_err().contains("endpoint"));
    }

    #[test]
    fn api_base_is_enough() {
        let llm = LLMConfig {
            model: "qwen".into(),
            api_base: "http://localhost:8080/v1".into(),
            ..Default::default()
        };
        assert!(preflight_llm(&llm).is_ok());
    }

    #[test]
    fn api_key_is_enough() {
        let llm = LLMConfig {
            model: "claude-x".into(),
            api_key: "sk".into(),
            ..Default::default()
        };
        assert!(preflight_llm(&llm).is_ok());
    }
}

#[cfg(test)]
mod waiting_tests {
    use super::*;
    use flows_core::models::RunState;

    fn seed(store: &Store, run_id: &str) {
        store.ensure_run_dir(run_id).unwrap();
        store
            .write_state(&RunState {
                run_id: run_id.into(),
                status: RunStatus::Running,
                current_phase: "plan".into(),
                current_phase_index: 0,
                round: 0,
                questions: vec![],
                error: None,
                completed_phases: vec![],
                skipped_phases: vec![],
            })
            .unwrap();
    }

    #[tokio::test]
    async fn on_questions_blocks_until_answer() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(dir.path());
        seed(&store, "r1");
        let w = Waiting {
            store: store.clone(),
            run_id: "r1".into(),
            notify: false,
            sound: false,
        };

        let s2 = store.clone();
        let sender = tokio::spawn(async move {
            // wait until the run publishes the question, then answer it
            loop {
                if let Ok(Some(st)) = s2.read_state("r1") {
                    if st.status == RunStatus::WaitingInput
                        && st.questions == vec!["color?".to_string()]
                    {
                        break;
                    }
                }
                tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            }
            s2.send_answer("r1", "blue").unwrap();
        });

        let answer = w.on_questions("plan", &["color?".into()]).await;
        assert_eq!(answer, "blue");
        sender.await.unwrap();
        // status restored to running after answering
        assert_eq!(
            store.read_state("r1").unwrap().unwrap().status,
            RunStatus::Running
        );
    }

    #[tokio::test]
    async fn on_human_gate_unblocks_on_advance() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(dir.path());
        seed(&store, "r1");
        let w = Waiting {
            store: store.clone(),
            run_id: "r1".into(),
            notify: false,
            sound: false,
        };
        store.send_advance("r1").unwrap(); // already advanced
        assert_eq!(w.on_human_gate("approve", "ok?").await, "approved");
    }

    #[tokio::test]
    async fn cancel_unblocks_questions() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(dir.path());
        seed(&store, "r1");
        let w = Waiting {
            store: store.clone(),
            run_id: "r1".into(),
            notify: false,
            sound: false,
        };
        store.send_cancel("r1").unwrap();
        assert_eq!(w.on_questions("plan", &["q?".into()]).await, "");
        // The gate must PEEK, not consume — the runner's own poll_cancel needs the
        // signal to actually cancel the run (else the empty answer reads as proceed).
        assert!(
            store.is_cancelled("r1"),
            "cancel signal must survive the gate"
        );
    }

    #[tokio::test]
    async fn cancel_at_approval_gate_does_not_consume_signal() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(dir.path());
        seed(&store, "r1");
        let w = Waiting {
            store: store.clone(),
            run_id: "r1".into(),
            notify: false,
            sound: false,
        };
        store.send_cancel("r1").unwrap();
        // Unblocks with an empty answer, but leaves the cancel signal for the runner
        // — otherwise the approval gate would read "" as APPROVE and resume the run.
        assert_eq!(w.on_human_gate("approve", "ok?").await, "");
        assert!(
            store.is_cancelled("r1"),
            "cancel signal must survive the gate"
        );
    }
}
