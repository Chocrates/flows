//! Attach view — reattach to a (backgrounded) run and drive the **same `App`**
//! used by a live in-process run (`run.rs`). Instead of an in-process channel,
//! the App is fed by polling the run's files: phase status from state.json, the
//! token stream from output.log (categorized JSONL, so thinking/normal toggles
//! work), and cost from llm.jsonl. Questions / human gates surface as the App's
//! chat bar; the typed answer is written back as an answer signal.

use std::io;
use std::path::Path;
use std::time::Duration;

use crossterm::event::{self, DisableMouseCapture, EnableMouseCapture, Event, KeyEventKind};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;
use tokio::sync::oneshot;

use flows_core::state::Store;

use crate::app::{App, UiEvent};
use crate::output::Category;
use crate::PhaseStatus;

#[cfg(test)]
fn total_cost(store: &Store, run_id: &str) -> f64 {
    let path = flows_core::llm_log::log_path(&store.run_dir(run_id));
    flows_core::usage::parse_usage(&path, run_id, "").total_cost()
}

fn loop_edges(store: &Store, run_id: &str) -> Vec<crate::phases::LoopEdge> {
    store
        .read_meta(run_id)
        .map(|m| {
            m.phases
                .iter()
                .filter_map(|p| {
                    let from = p.get("name")?.as_str()?.to_string();
                    let to = p.get("loop_back_to")?.as_str()?.to_string();
                    let max = p
                        .get("max_loop_iterations")
                        .and_then(|v| v.as_u64())
                        .unwrap_or(3) as u32;
                    Some(crate::phases::LoopEdge { from, to, max })
                })
                .collect()
        })
        .unwrap_or_default()
}

fn phase_names(store: &Store, run_id: &str) -> Vec<String> {
    store
        .read_meta(run_id)
        .map(|m| {
            m.phases
                .iter()
                .filter_map(|p| p.get("name").and_then(|n| n.as_str()).map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

/// Read new bytes of a file past `offset`, advancing it. Handles truncation.
fn tail_from(path: &Path, offset: &mut u64) -> String {
    use std::io::{Read, Seek, SeekFrom};
    let Ok(mut f) = std::fs::File::open(path) else {
        return String::new();
    };
    let len = f.metadata().map(|m| m.len()).unwrap_or(0);
    if len < *offset {
        *offset = 0; // file shrank / rotated
    }
    if len == *offset {
        return String::new();
    }
    if f.seek(SeekFrom::Start(*offset)).is_err() {
        return String::new();
    }
    let mut buf = Vec::new();
    let _ = f.read_to_end(&mut buf);
    *offset = len;
    String::from_utf8_lossy(&buf).into_owned()
}

/// Feed one categorized output.log line (`{"c":..,"x":..}`) into the pane.
fn push_output_line(app: &mut App, line: &str) {
    let line = line.trim_end();
    if line.is_empty() {
        return;
    }
    match serde_json::from_str::<serde_json::Value>(line) {
        Ok(v) => {
            let x = v.get("x").and_then(|x| x.as_str()).unwrap_or("");
            match v.get("c").and_then(|c| c.as_str()).unwrap_or("n") {
                "t" => app.output.push_token(Category::Thinking, x),
                "j" => app.output.push_token(Category::Json, x),
                "o" => app.output.push_line(Category::Tool, x.to_string()),
                "s" => app.output.push_line(Category::System, x.to_string()),
                "m" => app.output.push_line(Category::Meta, x.to_string()),
                "u" => {
                    app.output
                        .push_line(Category::Meta, "── user ──".to_string());
                    for line in x.lines() {
                        app.output.push_line(Category::Meta, line.to_string());
                    }
                }
                // Status-bar metrics — update the field, no scrollback line.
                "ctx" => app.context_tokens = x.parse().unwrap_or(0),
                "cost" => app.total_cost = x.parse().unwrap_or(0.0),
                "cw" => app.context_window = x.parse().ok(),
                "round" => app.round = x.to_string(),
                _ => app.output.push_token(Category::Normal, x),
            }
        }
        // Tolerate any non-JSON line (e.g. an older run) as plain output.
        Err(_) => app.output.push_token(Category::Normal, line),
    }
}

/// Open the attach view for `run_id`. Blocks until the user quits.
pub fn attach_tui(store: Store, run_id: String) -> io::Result<()> {
    let names = phase_names(&store, &run_id);
    if names.is_empty() {
        eprintln!("no such run: {run_id}");
        return Ok(());
    }

    let mut app = App::new(&names);
    app.phases.set_loops(loop_edges(&store, &run_id));

    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    let mut terminal = Terminal::new(CrosstermBackend::new(stdout))?;

    let result = attach_loop(&mut terminal, &store, &run_id, &mut app);

    let _ = disable_raw_mode();
    let _ = execute!(
        terminal.backend_mut(),
        LeaveAlternateScreen,
        DisableMouseCapture
    );
    let _ = terminal.show_cursor();
    result
}

fn attach_loop<B: ratatui::backend::Backend>(
    terminal: &mut Terminal<B>,
    store: &Store,
    run_id: &str,
    app: &mut App,
) -> io::Result<()> {
    let log_path = store.run_dir(run_id).join("output.log");
    let mut live_offset: u64 = 0;
    let mut line_buf = String::new();
    let mut prompting = false;
    let mut reply_rx: Option<oneshot::Receiver<String>> = None;
    let mut cancel_sent = false;

    // Clear any stale answer/advance/cancel signals left on an already-finished
    // run, so nothing lingers to be misread.
    if let Ok(Some(st)) = store.read_state(run_id) {
        if matches!(st.status.as_str(), "completed" | "failed" | "cancelled") {
            let _ = store.poll_cancel(run_id);
            let _ = store.poll_advance(run_id);
            let _ = store.read_answer(run_id);
        }
    }

    loop {
        // --- poll run state → phases / status / cost / input prompts ---------
        if let Ok(Some(st)) = store.read_state(run_id) {
            for done in &st.completed_phases {
                app.phases.set_status(done, PhaseStatus::Done);
            }
            match st.status.as_str() {
                "failed" => {
                    app.phases
                        .set_status(&st.current_phase, PhaseStatus::Failed);
                    app.finished = true;
                    app.status = format!("✗ failed at {}", st.current_phase);
                }
                "completed" => {
                    app.finished = true;
                    app.status = "✓ complete".into();
                }
                "cancelled" => {
                    app.finished = true;
                    app.status = "✗ cancelled".into();
                }
                // Only a genuinely-running run (not one already finished) prompts.
                "waiting_input" | "waiting_approval" if !app.finished => {
                    app.phases
                        .set_status(&st.current_phase, PhaseStatus::Running);
                    if !prompting {
                        let prompt = if st.status == "waiting_input" {
                            format!("? {}", st.questions.join("  /  "))
                        } else {
                            format!(
                                "⏸ {}  (Enter to approve, or type feedback)",
                                st.questions.first().cloned().unwrap_or_default()
                            )
                        };
                        let (tx, rx) = oneshot::channel();
                        app.handle_ui_event(UiEvent::NeedInput { prompt, reply: tx });
                        reply_rx = Some(rx);
                        prompting = true;
                    }
                }
                _ => {
                    app.phases
                        .set_status(&st.current_phase, PhaseStatus::Running);
                    if !app.finished {
                        app.status = format!("running {}", st.current_phase);
                    }
                    // Worker resumed past a gate — ready to prompt for the next one.
                    prompting = false;
                }
            }
            // One parse of llm.jsonl → running cost + current context size (the
            // latest request's input tokens) + the model's window for a %.
            let rep = flows_core::usage::parse_usage(
                &flows_core::llm_log::log_path(&store.run_dir(run_id)),
                run_id,
                "",
            );
            app.total_cost = rep.total_cost();
            app.context_tokens = rep.last_input_tokens;
            if app.context_window.is_none() {
                if let Some(model) = rep
                    .phases
                    .iter()
                    .rev()
                    .find(|p| !p.model.is_empty())
                    .map(|p| p.model.clone())
                {
                    app.context_window = flows_core::pricing::context_window(&model);
                }
            }
        }

        // A finished run must never keep a pending gate prompt around.
        if app.finished && prompting {
            app.dismiss_input();
            prompting = false;
            reply_rx = None;
        }

        // --- deliver a typed answer back to the worker as an answer signal ---
        // Keep `prompting` latched until the worker actually resumes (state leaves
        // the waiting status), so we don't re-open the prompt during the ~500ms
        // before the worker picks up the answer.
        if let Some(rx) = &mut reply_rx {
            if let Ok(answer) = rx.try_recv() {
                if !app.finished {
                    let _ = store.send_answer(run_id, answer.trim());
                }
                reply_rx = None;
            }
        }

        // --- tail the token stream (buffer partial JSONL lines) --------------
        line_buf.push_str(&tail_from(&log_path, &mut live_offset));
        while let Some(nl) = line_buf.find('\n') {
            let line = line_buf[..nl].to_string();
            line_buf.drain(..=nl);
            push_output_line(app, &line);
        }

        terminal.draw(|f| app.render(f))?;

        // --- input ----------------------------------------------------------
        if event::poll(Duration::from_millis(200))? {
            match event::read()? {
                Event::Key(k) if k.kind == KeyEventKind::Press => app.handle_key(k),
                Event::Mouse(m) => app.handle_mouse(m),
                _ => {}
            }
        }
        if app.cancel_requested && !cancel_sent {
            let _ = store.send_cancel(run_id);
            cancel_sent = true;
        }
        if app.quit {
            break;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{push_output_line, tail_from, total_cost};
    use crate::app::App;
    use crate::output::Category;
    use flows_core::state::Store;
    use flows_core::LlmLogger;
    use std::io::Write;

    #[test]
    fn finished_run_signal_cleanup_consumes_stale_signals() {
        // Regression: a completed run must not leave answer/advance signals that
        // could be misread as a pending approval. attach_tui's start-of-loop
        // cleanup consumes them.
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(dir.path());
        store.ensure_run_dir("r1").unwrap();
        store.send_answer("r1", "approved").unwrap(); // writes answer.txt + advance
                                                      // mirror attach's cleanup for a finished run
        let _ = store.poll_cancel("r1");
        let _ = store.poll_advance("r1");
        let _ = store.read_answer("r1");
        assert!(
            store.read_answer("r1").is_none(),
            "answer signal should be consumed"
        );
        assert!(
            !store.poll_advance("r1"),
            "advance signal should be consumed"
        );
    }

    #[test]
    fn categorized_lines_route_to_the_right_pane_category() {
        let mut app = App::new(&["plan".into()]);
        push_output_line(&mut app, r#"{"c":"n","x":"hello "}"#);
        push_output_line(&mut app, r#"{"c":"t","x":"thinking..."}"#);
        push_output_line(&mut app, r#"{"c":"o","x":"tool ran"}"#);

        // normal + tool visible by default; thinking hidden once toggled off
        let vis = app.output.visible_lines().join("\n");
        assert!(vis.contains("hello"));
        assert!(vis.contains("thinking..."));
        assert!(vis.contains("tool ran"));
        app.output.toggle_thinking();
        let vis = app.output.visible_lines().join("\n");
        assert!(!vis.contains("thinking..."));
        assert!(vis.contains("hello")); // normal survives the thinking toggle
    }

    #[test]
    fn non_json_line_falls_back_to_normal() {
        let mut app = App::new(&["plan".into()]);
        push_output_line(&mut app, "plain old line");
        assert!(app
            .output
            .visible_lines()
            .iter()
            .any(|l| l == "plain old line"));
    }

    #[test]
    fn attach_replays_existing_output_log_from_start() {
        // Regression: attaching to an already-running (or finished) run must
        // backfill the prior output, not show a blank pane waiting for new output.
        use crate::app::OutputLog;
        let dir = tempfile::tempdir().unwrap();
        let log_path = dir.path().join("output.log");
        let log = OutputLog::new(log_path.clone());
        log.emit("m", "▸ task: build solitaire");
        log.emit("n", "hello ");
        log.emit("n", "world\n");
        log.emit("o", "✓ write_file ok");

        // Replay exactly as attach_loop does: tail from offset 0, split lines, push.
        let mut app = App::new(&["plan".into()]);
        let mut off = 0u64;
        let mut line_buf = super::tail_from(&log_path, &mut off);
        while let Some(nl) = line_buf.find('\n') {
            let line = line_buf[..nl].to_string();
            line_buf.drain(..=nl);
            push_output_line(&mut app, &line);
        }
        let vis = app.output.visible_lines().join("\n");
        assert!(vis.contains("build solitaire"), "task missing:\n{vis}");
        assert!(vis.contains("hello world"), "tokens missing:\n{vis}");
        assert!(vis.contains("write_file ok"), "tool line missing:\n{vis}");
    }

    #[test]
    fn tail_from_reads_incrementally() {
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join("output.log");
        std::fs::write(&log, "hello ").unwrap();
        let mut off = 0u64;
        assert_eq!(tail_from(&log, &mut off), "hello ");
        assert_eq!(tail_from(&log, &mut off), "");
        std::fs::OpenOptions::new()
            .append(true)
            .open(&log)
            .unwrap()
            .write_all(b"world")
            .unwrap();
        assert_eq!(tail_from(&log, &mut off), "world");
    }

    #[test]
    fn total_cost_from_llm_jsonl() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(dir.path());
        store.ensure_run_dir("r1").unwrap();
        let logger = LlmLogger::new("r1", flows_core::llm_log::log_path(&store.run_dir("r1")));
        logger.response("plan", 0, "claude-sonnet-4-6", 1_000_000, 1_000_000, 100);
        assert!((total_cost(&store, "r1") - 18.0).abs() < 1e-6);
        let _ = Category::Normal;
    }
}
