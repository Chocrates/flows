//! Terminal setup + the interactive event loop. The only part that needs a real
//! terminal; the App logic it drives is tested separately.

use std::io;
use std::path::PathBuf;
use std::time::Duration;

use crossterm::event::{self, DisableMouseCapture, EnableMouseCapture, Event, KeyEventKind};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;
use tokio::sync::mpsc;

use std::sync::Arc;

use flows_core::models::{GlobalConfig, LLMConfig, PhaseConfig, ProjectConfig};
use flows_core::state::Store;
use flows_llm::Provider;
use flows_runner::runner::run_pipeline;

use crate::app::{App, ChannelCallbacks};

/// Owned provider factory (builds a [`Provider`] per resolved [`LLMConfig`]).
/// Owned (not borrowed) because the pipeline runs on a spawned `'static` task.
pub type ProviderFactory = Box<dyn Fn(&LLMConfig) -> Arc<dyn Provider> + Send + Sync>;

/// Run a pipeline inside the full-screen TUI. Blocks until the run finishes and
/// the user quits.
#[allow(clippy::too_many_arguments)]
pub fn run_tui(
    run_id: String,
    idea: String,
    repo_root: PathBuf,
    phases: Vec<PhaseConfig>,
    template_name: String,
    global_cfg: GlobalConfig,
    project_cfg: ProjectConfig,
    provider_factory: ProviderFactory,
    store: Store,
    flows_dir: Option<PathBuf>,
) -> io::Result<()> {
    let rt = tokio::runtime::Runtime::new()?;
    let (tx, mut rx) = mpsc::unbounded_channel();

    let names: Vec<String> = phases.iter().map(|p| p.name.clone()).collect();
    let mut app = App::new(&names);
    // Echo the user's task at the top (always visible) so it's clear what the run
    // was asked to do — independent of the system-prompt toggle.
    app.output
        .push_line(crate::output::Category::Meta, format!("▸ task: {idea}"));
    let model = flows_core::config::merge_llm(&global_cfg, &project_cfg).model;
    app.context_window = flows_core::pricing::context_window(&model);

    // Persist output to the run's `output.log` so a later `flows attach` replays
    // this session instead of showing a blank pane. Seed it with the task + window
    // (the live app already shows those in-memory). Ensure the run dir exists first
    // so the seed writes land before the pipeline creates it.
    let _ = store.ensure_run_dir(&run_id);
    let output_log = crate::app::OutputLog::new(store.run_dir(&run_id).join("output.log"));
    output_log.emit("m", &format!("▸ task: {idea}"));
    if let Some(w) = app.context_window {
        output_log.emit("cw", &w.to_string());
    }
    let loops: Vec<crate::phases::LoopEdge> = phases
        .iter()
        .filter_map(|p| {
            p.loop_back_to.clone().map(|to| crate::phases::LoopEdge {
                from: p.name.clone(),
                to,
                max: p.max_loop_iterations,
            })
        })
        .collect();
    app.phases.set_loops(loops);

    // Spawn the pipeline; it owns its inputs so the future is 'static.
    let task_store = store.clone();
    let task_run_id = run_id.clone();
    rt.spawn(async move {
        let cb = ChannelCallbacks {
            tx,
            log: Some(output_log),
        };
        let _ = run_pipeline(
            &task_run_id,
            &idea,
            &repo_root,
            &phases,
            &template_name,
            &global_cfg,
            &project_cfg,
            &*provider_factory,
            &cb,
            &task_store,
            None,
            flows_dir.as_deref(),
        )
        .await;
    });

    // Terminal setup.
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    let mut terminal = Terminal::new(CrosstermBackend::new(stdout))?;

    let result = event_loop(&mut terminal, &mut app, &mut rx, &store, &run_id);

    // Teardown (best-effort, always run).
    let _ = disable_raw_mode();
    let _ = execute!(
        terminal.backend_mut(),
        LeaveAlternateScreen,
        DisableMouseCapture
    );
    let _ = terminal.show_cursor();

    result
}

fn event_loop<B: ratatui::backend::Backend>(
    terminal: &mut Terminal<B>,
    app: &mut App,
    rx: &mut mpsc::UnboundedReceiver<crate::app::UiEvent>,
    store: &Store,
    run_id: &str,
) -> io::Result<()> {
    let mut cancel_sent = false;
    loop {
        terminal.draw(|f| app.render(f))?;

        // Drain pipeline events.
        while let Ok(ev) = rx.try_recv() {
            app.handle_ui_event(ev);
        }

        // Forward a cancel request to the running pipeline (once).
        if app.cancel_requested && !cancel_sent {
            let _ = store.send_cancel(run_id);
            cancel_sent = true;
        }

        // Input (short poll so pipeline events stay responsive).
        if event::poll(Duration::from_millis(50))? {
            match event::read()? {
                Event::Key(key) if key.kind == KeyEventKind::Press => app.handle_key(key),
                Event::Mouse(m) => app.handle_mouse(m),
                _ => {}
            }
        }

        if app.quit {
            break;
        }
    }
    Ok(())
}
