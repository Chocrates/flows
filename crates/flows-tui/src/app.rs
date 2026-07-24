//! TUI application state + event handling. The pipeline runs on a background
//! task and sends `UiEvent`s to the App; the App also handles key/mouse input.
//! Handlers are plain methods (testable without a terminal); the terminal loop
//! lives in `run.rs`.

use async_trait::async_trait;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::Rect;
use tokio::sync::{mpsc, oneshot};

use flows_core::models::Artifact;
use flows_runner::runner::RunnerCallbacks;

use crate::output::Category;
use crate::{OutputPane, PhasePanel, PhaseStatus};

/// Events from the running pipeline to the UI.
pub enum UiEvent {
    PhaseStart {
        name: String,
        index: usize,
        total: usize,
    },
    Token {
        text: String,
    },
    Json {
        text: String,
    },
    Thinking {
        text: String,
    },
    System {
        text: String,
    },
    User {
        text: String,
    },
    Note {
        text: String,
    },
    ToolCall {
        summary: String,
    },
    ToolResult {
        summary: String,
        is_error: bool,
    },
    Status {
        message: String,
    },
    ScriptResult {
        exit_code: i32,
    },
    Cost {
        total_usd: f64,
    },
    /// Input tokens of the latest request (current context-window usage).
    Context {
        input_tokens: u64,
    },
    /// Current tool-loop round / cap, pre-formatted as `N/T`.
    Round {
        text: String,
    },
    PhaseEnd {
        name: String,
    },
    LoopBack {
        from: String,
        to: String,
        iteration: u32,
    },
    Complete {
        phases: Vec<String>,
    },
    Failed {
        phase: String,
        error: String,
    },
    /// The pipeline needs an answer (question or human gate); reply via the sender.
    NeedInput {
        prompt: String,
        reply: oneshot::Sender<String>,
    },
}

/// Appends categorized output to a run's `output.log` (JSONL `{"c":cat,"x":text}`)
/// so a later `flows attach` can replay it. Same format the background worker
/// writes, so both live-TUI and detached runs are replayable. `c`: n=normal,
/// t=thinking, j=json, o=tool, s=system, u=user, m=meta; ctx/cost/cw are metrics.
pub struct OutputLog {
    path: std::path::PathBuf,
}

impl OutputLog {
    pub fn new(path: std::path::PathBuf) -> Self {
        Self { path }
    }
    pub fn emit(&self, cat: &str, text: &str) {
        use std::io::Write;
        let line = serde_json::json!({ "c": cat, "x": text }).to_string();
        if let Ok(mut f) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)
        {
            let _ = writeln!(f, "{line}");
        }
    }
}

/// `RunnerCallbacks` that forwards everything to the UI over a channel, and
/// optionally tees it to `output.log` so the run is replayable on re-attach.
pub struct ChannelCallbacks {
    pub tx: mpsc::UnboundedSender<UiEvent>,
    pub log: Option<OutputLog>,
}

impl ChannelCallbacks {
    fn send(&self, ev: UiEvent) {
        let _ = self.tx.send(ev);
    }

    /// Tee a categorized line to `output.log` if persistence is enabled.
    fn log(&self, cat: &str, text: &str) {
        if let Some(l) = &self.log {
            l.emit(cat, text);
        }
    }

    async fn request_input(&self, prompt: String) -> String {
        let (reply, rx) = oneshot::channel();
        self.send(UiEvent::NeedInput { prompt, reply });
        rx.await.unwrap_or_default()
    }
}

#[async_trait]
impl RunnerCallbacks for ChannelCallbacks {
    async fn on_questions(&self, _phase: &str, questions: &[String]) -> String {
        // One question per line (numbered when there are several) so a multi-part
        // clarification is readable instead of run together on one line.
        let prompt = match questions {
            [only] => format!("? {only}"),
            many => {
                let list = many
                    .iter()
                    .enumerate()
                    .map(|(i, q)| format!("{}. {q}", i + 1))
                    .collect::<Vec<_>>()
                    .join("\n");
                format!("?\n{list}")
            }
        };
        self.request_input(prompt).await
    }
    async fn on_human_gate(&self, _phase: &str, instructions: &str) -> String {
        self.request_input(format!("⏸ {instructions}")).await
    }
    fn on_status(&self, _phase: &str, message: &str) {
        self.send(UiEvent::Status {
            message: message.into(),
        });
    }
    fn on_token(&self, _phase: &str, token: &str) {
        self.log("n", token);
        self.send(UiEvent::Token { text: token.into() });
    }
    fn on_json(&self, _phase: &str, token: &str) {
        self.log("j", token);
        self.send(UiEvent::Json { text: token.into() });
    }
    fn on_thinking(&self, _phase: &str, text: &str) {
        self.log("t", text);
        self.send(UiEvent::Thinking { text: text.into() });
    }
    fn on_system(&self, _phase: &str, prompt: &str) {
        self.log("s", prompt);
        self.send(UiEvent::System {
            text: prompt.into(),
        });
    }
    fn on_user(&self, _phase: &str, message: &str) {
        self.log("u", message);
        self.send(UiEvent::User {
            text: message.into(),
        });
    }
    fn on_note(&self, _phase: &str, text: &str) {
        self.log("m", text);
        self.send(UiEvent::Note { text: text.into() });
    }
    fn on_tool_call(&self, _phase: &str, name: &str, summary: &str) {
        self.log("o", &format!("⚙ {name} {summary}"));
        self.send(UiEvent::ToolCall {
            summary: summary.into(),
        });
    }
    fn on_tool_result(&self, _phase: &str, name: &str, summary: &str, is_error: bool) {
        let glyph = if is_error { '✗' } else { '✓' };
        self.log("o", &format!("{glyph} {name} {summary}"));
        self.send(UiEvent::ToolResult {
            summary: summary.into(),
            is_error,
        });
    }
    fn on_phase_start(&self, phase: &str, index: usize, total: usize) {
        self.log(
            "m",
            &format!("━━ phase {}/{}: {phase} ━━", index + 1, total),
        );
        self.send(UiEvent::PhaseStart {
            name: phase.into(),
            index,
            total,
        });
    }
    fn on_phase_end(&self, phase: &str, _artifact: Option<&Artifact>) {
        self.send(UiEvent::PhaseEnd { name: phase.into() });
    }
    fn on_loop_back(&self, from: &str, to: &str, iteration: u32, _max: u32) {
        self.send(UiEvent::LoopBack {
            from: from.into(),
            to: to.into(),
            iteration,
        });
    }
    fn on_script_result(&self, _phase: &str, exit_code: i32, stdout: &str, _stderr: &str) {
        self.log("m", &format!("script → exit {exit_code}\n{stdout}"));
        self.send(UiEvent::ScriptResult { exit_code });
    }
    fn on_context(&self, input_tokens: u64) {
        self.log("ctx", &input_tokens.to_string());
        self.send(UiEvent::Context { input_tokens });
    }
    fn on_round(&self, round: u32, total: u32) {
        let text = format!("{round}/{total}");
        self.log("round", &text);
        self.send(UiEvent::Round { text });
    }
    fn on_cost(&self, total_usd: f64) {
        self.log("cost", &total_usd.to_string());
        self.send(UiEvent::Cost { total_usd });
    }
    fn on_run_complete(&self, _run_id: &str, phases: &[String]) {
        self.send(UiEvent::Complete {
            phases: phases.to_vec(),
        });
    }
    fn on_run_failed(&self, _run_id: &str, phase: &str, error: &str) {
        self.send(UiEvent::Failed {
            phase: phase.into(),
            error: error.into(),
        });
    }
}

struct InputState {
    prompt: String,
    buffer: String,
    reply: Option<oneshot::Sender<String>>,
}

pub struct App {
    pub output: OutputPane,
    pub phases: PhasePanel,
    pub status: String,
    pub quit: bool,
    pub cancel_requested: bool,
    pub finished: bool,
    pub total_cost: f64,
    /// Input tokens of the latest request — current context-window usage.
    pub context_tokens: u64,
    /// Model context window, if known (for a `12k/200k` display).
    pub context_window: Option<u64>,
    /// Current tool-loop round / cap (`N/T`), empty until the first round.
    pub round: String,
    output_area: Rect,
    dragging: bool,
    input: Option<InputState>,
}

impl App {
    pub fn new(phase_names: &[String]) -> Self {
        let mut phases = PhasePanel::new();
        phases.setup(phase_names);
        Self {
            output: OutputPane::new(),
            phases,
            status: "starting…".into(),
            quit: false,
            cancel_requested: false,
            finished: false,
            total_cost: 0.0,
            context_tokens: 0,
            context_window: None,
            round: String::new(),
            output_area: Rect::default(),
            dragging: false,
            input: None,
        }
    }

    pub fn awaiting_input(&self) -> bool {
        self.input.is_some()
    }

    /// Drop any pending input prompt without answering (e.g. the run finished
    /// while a stale gate prompt was showing). The reply sender is dropped, so a
    /// waiting caller sees the channel close rather than an answer.
    pub fn dismiss_input(&mut self) {
        self.input = None;
    }

    pub fn input_prompt(&self) -> Option<(&str, &str)> {
        self.input
            .as_ref()
            .map(|i| (i.prompt.as_str(), i.buffer.as_str()))
    }

    pub fn set_output_area(&mut self, area: Rect) {
        self.output_area = area;
        self.output
            .set_viewport(area.height as usize, area.width as usize);
    }

    // -- pipeline events -------------------------------------------------

    pub fn handle_ui_event(&mut self, ev: UiEvent) {
        match ev {
            UiEvent::PhaseStart { name, .. } => {
                self.phases.set_status(&name, PhaseStatus::Running);
                self.output.push_line(Category::Meta, format!("▶ {name}"));
                self.status = format!("running {name}");
                self.round.clear();
            }
            UiEvent::Token { text } => self.output.push_token(Category::Normal, &text),
            UiEvent::Json { text } => self.output.push_token(Category::Json, &text),
            UiEvent::Thinking { text } => self.output.push_token(Category::Thinking, &text),
            UiEvent::System { text } => {
                // Multi-line — push each line (hidden by default; toggle `s`).
                self.output
                    .push_line(Category::System, "── system prompt ──");
                for line in text.lines() {
                    self.output.push_line(Category::System, line);
                }
                self.output
                    .push_line(Category::System, "── end system prompt ──");
            }
            UiEvent::User { text } => {
                // The question — always visible (Meta), framed as a user turn.
                self.output.push_line(Category::Meta, "── user ──");
                for line in text.lines() {
                    self.output.push_line(Category::Meta, line);
                }
            }
            UiEvent::Note { text } => self.output.push_line(Category::Meta, text),
            UiEvent::ToolCall { summary } => self
                .output
                .push_line(Category::Tool, format!("⚙ {summary}")),
            UiEvent::ToolResult { summary, is_error } => {
                let glyph = if is_error { '✗' } else { '✓' };
                self.output
                    .push_line(Category::Tool, format!("{glyph} {summary}"));
            }
            UiEvent::Status { message } => self.status = message,
            UiEvent::Cost { total_usd } => self.total_cost = total_usd,
            UiEvent::Context { input_tokens } => self.context_tokens = input_tokens,
            UiEvent::Round { text } => self.round = text,
            UiEvent::ScriptResult { exit_code } => self
                .output
                .push_line(Category::Meta, format!("script exited {exit_code}")),
            UiEvent::PhaseEnd { name } => {
                if self.phases.status_of(&name) != Some(PhaseStatus::Failed) {
                    self.phases.set_status(&name, PhaseStatus::Done);
                }
            }
            UiEvent::LoopBack { from, to, iteration } => {
                // Clear stale ✓ on the phases about to re-run, then mark the source
                // as looping (↻) — a rejected design shouldn't read as "done".
                self.phases.restart_range(&to, &from);
                self.phases.set_loop_iteration(&from, iteration);
            }
            UiEvent::Complete { phases } => {
                self.finished = true;
                self.status = format!("✓ complete: {}", phases.join(" → "));
            }
            UiEvent::Failed { phase, error } => {
                self.phases.set_status(&phase, PhaseStatus::Failed);
                self.finished = true;
                self.status = format!("✗ failed at {phase}: {error}");
            }
            UiEvent::NeedInput { prompt, reply } => {
                // The run is paused waiting on the human — reflect that, not "running".
                // A `⏸`-prefixed prompt is a human gate (approval); `?` is a question.
                self.status = if prompt.starts_with('⏸') {
                    "⏸ paused — awaiting your approval (Enter to accept, or type feedback)".into()
                } else {
                    "⏸ paused — awaiting your answer".into()
                };
                self.input = Some(InputState {
                    prompt,
                    buffer: String::new(),
                    reply: Some(reply),
                });
            }
        }
    }

    // -- keyboard --------------------------------------------------------

    pub fn handle_key(&mut self, key: KeyEvent) {
        if self.input.is_some() {
            self.handle_input_key(key);
            return;
        }
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        match key.code {
            KeyCode::Char('q') => self.quit = true,
            KeyCode::Char('c') if !ctrl => self.request_cancel(),
            KeyCode::Esc => self.request_cancel(),
            KeyCode::Char('c') if ctrl => self.copy_selection(),
            KeyCode::Char('y') => self.copy_all(),
            KeyCode::Char('t') => self.output.toggle_thinking(),
            KeyCode::Char('j') => self.output.toggle_json(),
            KeyCode::Char('s') => self.output.toggle_system(),
            KeyCode::Up => self.output.scroll_up(1),
            KeyCode::Down => self.output.scroll_down(1),
            KeyCode::PageUp => self
                .output
                .scroll_up(self.output_area.height.max(1) as usize),
            KeyCode::PageDown => self
                .output
                .scroll_down(self.output_area.height.max(1) as usize),
            KeyCode::End => self.output.scroll_to_bottom(),
            KeyCode::Home => self.output.scroll_to_top(),
            _ => {}
        }
    }

    fn handle_input_key(&mut self, key: KeyEvent) {
        // View controls stay live while composing an answer — scrolling and the
        // output toggles must keep working when a question/gate is pending. Scroll
        // keys don't insert text; toggles use Alt so plain t/j/s still type.
        let alt = key.modifiers.contains(KeyModifiers::ALT);
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        // Detach WITHOUT answering: quit the TUI and leave the question pending. A
        // backgrounded run keeps waiting on the gate — re-attach to answer later.
        // Distinct from Enter (submit) and Esc (accept), so you can leave to check
        // something without accidentally approving.
        if ctrl && matches!(key.code, KeyCode::Char('c') | KeyCode::Char('q')) {
            self.quit = true;
            self.status = "detached (question still pending) — reattach to answer".into();
            return;
        }
        match key.code {
            KeyCode::Up => return self.output.scroll_up(1),
            KeyCode::Down => return self.output.scroll_down(1),
            KeyCode::PageUp => {
                return self
                    .output
                    .scroll_up(self.output_area.height.max(1) as usize)
            }
            KeyCode::PageDown => {
                return self
                    .output
                    .scroll_down(self.output_area.height.max(1) as usize)
            }
            KeyCode::Char('t') if alt => return self.output.toggle_thinking(),
            KeyCode::Char('j') if alt => return self.output.toggle_json(),
            KeyCode::Char('s') if alt => return self.output.toggle_system(),
            _ => {}
        }
        let Some(input) = self.input.as_mut() else {
            return;
        };
        match key.code {
            KeyCode::Char(c) => input.buffer.push(c),
            KeyCode::Backspace => {
                input.buffer.pop();
            }
            KeyCode::Enter => {
                let answer = input.buffer.clone();
                if let Some(reply) = input.reply.take() {
                    let _ = reply.send(answer);
                }
                self.input = None;
                self.status = "resuming…".into();
            }
            KeyCode::Esc => {
                // send empty (accept) and close
                if let Some(reply) = input.reply.take() {
                    let _ = reply.send(String::new());
                }
                self.input = None;
                self.status = "resuming…".into();
            }
            _ => {}
        }
    }

    fn request_cancel(&mut self) {
        self.cancel_requested = true;
        self.status = "cancelling…".into();
    }

    // -- mouse -----------------------------------------------------------

    pub fn handle_mouse(&mut self, m: MouseEvent) {
        let area = self.output_area;
        let inside = m.column >= area.x
            && m.column < area.x + area.width
            && m.row >= area.y
            && m.row < area.y + area.height;
        let (vx, vy) = (
            m.column.saturating_sub(area.x) as usize,
            m.row.saturating_sub(area.y) as usize,
        );
        match m.kind {
            MouseEventKind::Down(MouseButton::Left) if inside => {
                self.output.clear_selection();
                self.output.start_selection(vx, vy);
                self.dragging = true;
            }
            MouseEventKind::Drag(MouseButton::Left) if self.dragging => {
                self.output.update_selection(vx, vy);
            }
            MouseEventKind::Up(MouseButton::Left) => self.dragging = false,
            MouseEventKind::ScrollUp => self.output.scroll_up(3),
            MouseEventKind::ScrollDown => self.output.scroll_down(3),
            _ => {}
        }
    }

    // -- clipboard -------------------------------------------------------

    fn copy(&mut self, text: Option<String>) {
        let Some(text) = text else { return };
        if text.is_empty() {
            return;
        }
        let n = text.chars().count();
        match arboard::Clipboard::new().and_then(|mut c| c.set_text(text)) {
            Ok(()) => self.status = format!("copied {n} chars"),
            Err(e) => self.status = format!("copy failed: {e}"),
        }
    }

    pub fn copy_selection(&mut self) {
        let sel = self.output.selected_text();
        self.copy(sel);
    }

    pub fn copy_all(&mut self) {
        let all = self.output.all_text();
        self.copy(Some(all));
    }

    // -- rendering -------------------------------------------------------

    pub fn render(&mut self, f: &mut ratatui::Frame) {
        use ratatui::layout::{Constraint, Layout};
        use ratatui::widgets::Paragraph;

        let size = f.area();
        // main │ [question panel] │ status │ help. The question panel only
        // appears while awaiting an answer; it wraps a long question + the typed
        // answer instead of letting them run off a single line.
        let awaiting = self.input.is_some();
        let max_panel = (size.height / 2).max(3);
        let panel_h = if awaiting {
            self.question_height(size.width, max_panel)
        } else {
            0
        };
        let mut constraints = vec![Constraint::Min(1)];
        if awaiting {
            constraints.push(Constraint::Length(panel_h));
        }
        constraints.push(Constraint::Length(1)); // status
        constraints.push(Constraint::Length(1)); // help
        let rows = Layout::vertical(constraints).split(size);
        let (panel_idx, status_idx, help_idx) = if awaiting {
            (Some(1usize), 2usize, 3usize)
        } else {
            (None, 1usize, 2usize)
        };
        let cols = Layout::horizontal([Constraint::Length(28), Constraint::Min(10)]).split(rows[0]);

        // Each pane gets a titled border so the two are visually distinct.
        use ratatui::style::{Color, Style};
        use ratatui::widgets::{Block, Widget};
        let phases_block = Block::bordered()
            .title(" phases ")
            .border_style(Style::default().fg(Color::DarkGray));
        let output_block = Block::bordered()
            .title(" output ")
            .border_style(Style::default().fg(Color::DarkGray));
        let phases_inner = phases_block.inner(cols[0]);
        let output_inner = output_block.inner(cols[1]);
        phases_block.render(cols[0], f.buffer_mut());
        output_block.render(cols[1], f.buffer_mut());

        crate::render::draw_phases(f.buffer_mut(), phases_inner, &self.phases);
        self.set_output_area(output_inner);
        crate::render::draw_output(f.buffer_mut(), output_inner, &self.output);

        let mut status_line = self.status.clone();
        if !self.round.is_empty() {
            status_line.push_str(&format!("   ·   round {}", self.round));
        }
        if self.context_tokens > 0 {
            status_line.push_str(&format!("   ·   ctx {}", self.context_readout()));
        }
        if self.total_cost > 0.0 {
            status_line.push_str(&format!(
                "   ·   {}",
                flows_core::pricing::format_cost(self.total_cost)
            ));
        }
        f.render_widget(Paragraph::new(status_line), rows[status_idx]);
        f.render_widget(Paragraph::new(self.help_line()), rows[help_idx]);
        if let Some(i) = panel_idx {
            let title = if self.status.contains("approval") {
                " approve (Enter=ok · type feedback · ^C detach) "
            } else {
                " answer (Enter=submit · ^C detach) "
            };
            let block = Block::bordered()
                .title(title)
                .border_style(Style::default().fg(Color::Cyan));
            let inner = block.inner(rows[i]);
            block.render(rows[i], f.buffer_mut());
            let lines = self.question_lines(inner.width as usize, inner.height as usize);
            f.render_widget(Paragraph::new(lines), inner);
        }
    }

    /// Total height (incl. borders) for the question panel: question + a blank
    /// separator + answer, each wrapped to the inner width, capped at `max_h`.
    fn question_height(&self, width: u16, max_h: u16) -> u16 {
        let Some(inp) = self.input.as_ref() else {
            return 0;
        };
        let inner_w = width.saturating_sub(2).max(1) as usize;
        let mut q = 0usize;
        for l in inp.prompt.lines() {
            q += crate::output::wrap_line(l, inner_w).len();
        }
        let a = crate::output::wrap_line(&format!("> {}\u{2588}", inp.buffer), inner_w).len();
        let inner = ((q + 1 + a) as u16).min(max_h).max(3);
        inner + 2 // top/bottom borders
    }

    /// Styled panel body: the (possibly truncated) question, a blank line, then
    /// the wrapped answer tail with a block cursor — so neither runs off-screen.
    fn question_lines(&self, inner_w: usize, inner_h: usize) -> Vec<ratatui::text::Line<'static>> {
        use ratatui::style::{Color, Modifier, Style};
        use ratatui::text::Line;
        let Some(inp) = self.input.as_ref() else {
            return vec![];
        };
        let mut qw: Vec<String> = Vec::new();
        for l in inp.prompt.lines() {
            qw.extend(crate::output::wrap_line(l, inner_w));
        }
        let aw = crate::output::wrap_line(&format!("> {}\u{2588}", inp.buffer), inner_w);
        let sep = 1usize;
        // Always keep the answer tail visible; the question takes what's left.
        let a_show = aw.len().min(inner_h.saturating_sub(sep + 1)).max(1);
        let a_lines = &aw[aw.len() - a_show..];
        let q_budget = inner_h.saturating_sub(sep + a_show);

        let q_style = Style::default()
            .fg(Color::White)
            .add_modifier(Modifier::BOLD);
        let a_style = Style::default().fg(Color::Yellow);
        let mut lines: Vec<Line> = Vec::new();
        if qw.len() > q_budget && q_budget > 0 {
            for l in &qw[..q_budget - 1] {
                lines.push(Line::styled(l.clone(), q_style));
            }
            lines.push(Line::styled("…", q_style));
        } else {
            for l in &qw {
                lines.push(Line::styled(l.clone(), q_style));
            }
        }
        lines.push(Line::from("")); // separator
        for l in a_lines {
            lines.push(Line::styled(l.clone(), a_style));
        }
        lines
    }

    /// The bottom help bar: toggle keys with their live on/off state, plus the
    /// main shortcuts — so the user can see how to hide thinking / JSON.
    /// Compact context readout: `12.3k`, or `12.3k/200k (6%)` if the window is known.
    fn context_readout(&self) -> String {
        let cur = fmt_tokens(self.context_tokens);
        match self.context_window {
            Some(w) if w > 0 => {
                let pct = (self.context_tokens as f64 / w as f64 * 100.0).round() as u64;
                format!("{cur}/{} ({pct}%)", fmt_tokens(w))
            }
            _ => cur,
        }
    }

    fn help_line(&self) -> ratatui::text::Line<'static> {
        use ratatui::style::{Color, Modifier, Style};
        use ratatui::text::{Line, Span};

        let key = |k: &str| {
            Span::styled(
                format!(" {k} "),
                Style::default().add_modifier(Modifier::REVERSED),
            )
        };
        let state = |on: bool| {
            if on {
                Span::styled("on", Style::default().fg(Color::Green))
            } else {
                Span::styled("off", Style::default().fg(Color::DarkGray))
            }
        };
        let dim = Style::default().fg(Color::DarkGray);
        // While composing an answer, toggles move to Alt (plain letters type text).
        let awaiting = self.awaiting_input();
        let tk = |k: &str| {
            if awaiting {
                format!("⌥{k}")
            } else {
                k.to_string()
            }
        };

        Line::from(vec![
            key(&tk("t")),
            Span::raw(" thinking "),
            state(self.output.show_thinking),
            Span::styled("   ", dim),
            key(&tk("j")),
            Span::raw(" JSON "),
            state(self.output.show_json),
            Span::styled("   ", dim),
            key(&tk("s")),
            Span::raw(" system "),
            state(self.output.show_system),
            Span::styled("   ", dim),
            key("y"),
            Span::styled(" copy   ", dim),
            key("c"),
            Span::styled(" cancel   ", dim),
            key("q"),
            Span::styled(" quit", dim),
        ])
    }
}

/// Compact token count: `842`, `12.3k`, `1.0M`.
fn fmt_tokens(n: u64) -> String {
    if n < 1_000 {
        n.to_string()
    } else if n < 1_000_000 {
        format!("{:.1}k", n as f64 / 1_000.0)
    } else {
        format!("{:.1}M", n as f64 / 1_000_000.0)
    }
}
