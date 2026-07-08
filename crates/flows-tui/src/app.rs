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
    PhaseEnd {
        name: String,
    },
    LoopBack {
        from: String,
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

/// `RunnerCallbacks` that forwards everything to the UI over a channel.
pub struct ChannelCallbacks {
    pub tx: mpsc::UnboundedSender<UiEvent>,
}

impl ChannelCallbacks {
    fn send(&self, ev: UiEvent) {
        let _ = self.tx.send(ev);
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
        self.request_input(format!("? {}", questions.join(" / ")))
            .await
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
        self.send(UiEvent::Token { text: token.into() });
    }
    fn on_json(&self, _phase: &str, token: &str) {
        self.send(UiEvent::Json { text: token.into() });
    }
    fn on_thinking(&self, _phase: &str, text: &str) {
        self.send(UiEvent::Thinking { text: text.into() });
    }
    fn on_tool_call(&self, _phase: &str, _name: &str, summary: &str) {
        self.send(UiEvent::ToolCall {
            summary: summary.into(),
        });
    }
    fn on_tool_result(&self, _phase: &str, _name: &str, summary: &str, is_error: bool) {
        self.send(UiEvent::ToolResult {
            summary: summary.into(),
            is_error,
        });
    }
    fn on_phase_start(&self, phase: &str, index: usize, total: usize) {
        self.send(UiEvent::PhaseStart {
            name: phase.into(),
            index,
            total,
        });
    }
    fn on_phase_end(&self, phase: &str, _artifact: Option<&Artifact>) {
        self.send(UiEvent::PhaseEnd { name: phase.into() });
    }
    fn on_loop_back(&self, from: &str, _to: &str, iteration: u32, _max: u32) {
        self.send(UiEvent::LoopBack {
            from: from.into(),
            iteration,
        });
    }
    fn on_script_result(&self, _phase: &str, exit_code: i32, _stdout: &str, _stderr: &str) {
        self.send(UiEvent::ScriptResult { exit_code });
    }
    fn on_context(&self, input_tokens: u64) {
        self.send(UiEvent::Context { input_tokens });
    }
    fn on_cost(&self, total_usd: f64) {
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
        self.output.set_viewport_height(area.height as usize);
    }

    // -- pipeline events -------------------------------------------------

    pub fn handle_ui_event(&mut self, ev: UiEvent) {
        match ev {
            UiEvent::PhaseStart { name, .. } => {
                self.phases.set_status(&name, PhaseStatus::Running);
                self.output.push_line(Category::System, format!("▶ {name}"));
                self.status = format!("running {name}");
            }
            UiEvent::Token { text } => self.output.push_token(Category::Normal, &text),
            UiEvent::Json { text } => self.output.push_token(Category::Json, &text),
            UiEvent::Thinking { text } => self.output.push_token(Category::Thinking, &text),
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
            UiEvent::ScriptResult { exit_code } => self
                .output
                .push_line(Category::System, format!("script exited {exit_code}")),
            UiEvent::PhaseEnd { name } => {
                if self.phases.status_of(&name) != Some(PhaseStatus::Failed) {
                    self.phases.set_status(&name, PhaseStatus::Done);
                }
            }
            UiEvent::LoopBack { from, iteration } => {
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
            }
            KeyCode::Esc => {
                // send empty (accept) and close
                if let Some(reply) = input.reply.take() {
                    let _ = reply.send(String::new());
                }
                self.input = None;
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
        // main │ status │ help  (+ input line when awaiting an answer)
        let mut constraints = vec![
            Constraint::Min(1),
            Constraint::Length(1),
            Constraint::Length(1),
        ];
        if self.input.is_some() {
            constraints.push(Constraint::Length(1));
        }
        let rows = Layout::vertical(constraints).split(size);
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
        if self.context_tokens > 0 {
            status_line.push_str(&format!("   ·   ctx {}", self.context_readout()));
        }
        if self.total_cost > 0.0 {
            status_line.push_str(&format!(
                "   ·   {}",
                flows_core::pricing::format_cost(self.total_cost)
            ));
        }
        f.render_widget(Paragraph::new(status_line), rows[1]);
        f.render_widget(Paragraph::new(self.help_line()), rows[2]);
        if let Some((prompt, buffer)) = self.input_prompt() {
            f.render_widget(Paragraph::new(format!("{prompt}  > {buffer}")), rows[3]);
        }
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

        Line::from(vec![
            key("t"),
            Span::raw(" thinking "),
            state(self.output.show_thinking),
            Span::styled("   ", dim),
            key("j"),
            Span::raw(" JSON "),
            state(self.output.show_json),
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
