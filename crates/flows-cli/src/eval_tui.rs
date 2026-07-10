//! Full-screen review TUI for `flows eval review`: the trial transcript scrolls
//! in the top pane while the 1–5 ratings are entered in the bottom pane, so the
//! output stays visible the whole time you rate it.
//!
//! Falls back to the plain stdin flow (in `eval.rs`) when there is no terminal.

use std::io;
use std::path::Path;

use crossterm::event::{
    self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEventKind, KeyModifiers,
    MouseEventKind,
};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Direction, Layout};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};
use ratatui::{Frame, Terminal};

use crate::eval_store::{self, Ratings, TrialRecord};

/// One trial to review: its record plus the display text.
pub struct ReviewItem {
    pub record: TrialRecord,
    pub transcript: String,
    pub task: String,
    pub is_turn: bool,
}

const DIMS: [&str; 5] = [
    "Correctness",
    "Efficiency",
    "Tool use",
    "Grounding",
    "Instruction",
];
const TOOL_USE: usize = 2;

struct App {
    items: Vec<ReviewItem>,
    idx: usize,
    scroll: u16,
    dim: usize,
    values: [Option<u8>; 5],
    note: String,
    editing_note: bool,
    status: String,
    // Set during render for scroll clamping.
    view_h: u16,
    total_lines: u16,
    done: bool,
    graded: usize,
}

impl App {
    fn new(items: Vec<ReviewItem>) -> Self {
        let mut a = App {
            items,
            idx: 0,
            scroll: 0,
            dim: 0,
            values: [None; 5],
            note: String::new(),
            editing_note: false,
            status: String::new(),
            view_h: 0,
            total_lines: 0,
            done: false,
            graded: 0,
        };
        a.reset_for_current();
        a
    }

    fn cur(&self) -> &ReviewItem {
        &self.items[self.idx]
    }

    fn reset_for_current(&mut self) {
        self.scroll = 0;
        self.values = [None; 5];
        self.note.clear();
        self.editing_note = false;
        // Tool use is n/a for a turn — start on the first ratable dim, mark n/a.
        self.dim = 0;
        if self.cur().is_turn {
            self.values[TOOL_USE] = None; // stays n/a
        }
    }

    fn is_na(&self, dim: usize) -> bool {
        dim == TOOL_USE && self.cur().is_turn
    }

    fn required_rated(&self) -> bool {
        (0..5).all(|d| self.is_na(d) || self.values[d].is_some())
    }

    fn next_dim(&mut self) {
        let mut d = self.dim;
        for _ in 0..5 {
            d = (d + 1) % 5;
            if !self.is_na(d) {
                break;
            }
        }
        self.dim = d;
    }

    fn prev_dim(&mut self) {
        let mut d = self.dim;
        for _ in 0..5 {
            d = (d + 4) % 5;
            if !self.is_na(d) {
                break;
            }
        }
        self.dim = d;
    }

    fn set_rating(&mut self, n: u8) {
        if self.is_na(self.dim) {
            return;
        }
        self.values[self.dim] = Some(n);
        // Advance to the next still-unrated dimension, if any.
        for _ in 0..5 {
            self.next_dim();
            if self.values[self.dim].is_none() && !self.is_na(self.dim) {
                break;
            }
        }
    }

    fn max_scroll(&self) -> u16 {
        self.total_lines.saturating_sub(self.view_h)
    }

    fn scroll_by(&mut self, delta: i32) {
        let s = (self.scroll as i32 + delta).clamp(0, self.max_scroll() as i32);
        self.scroll = s as u16;
    }

    /// Save the current trial's ratings and advance. Returns false if not all
    /// required dims are rated yet.
    fn save_current(&mut self, flows_dir: &Path) -> bool {
        if !self.required_rated() {
            self.status = "rate every dimension first (1-5)".into();
            return false;
        }
        let is_turn = self.cur().is_turn;
        let ratings = Ratings {
            correctness: self.values[0].unwrap(),
            efficiency: self.values[1].unwrap(),
            tool_use: if is_turn { None } else { self.values[TOOL_USE] },
            grounding: self.values[3].unwrap(),
            instruction: self.values[4].unwrap(),
        };
        let rec = &mut self.items[self.idx].record;
        rec.manual_pass = Some(ratings.correctness >= 3);
        rec.manual_score = Some(ratings.mean().round() as u8);
        rec.ratings = Some(ratings);
        if !self.note.trim().is_empty() {
            rec.note = Some(self.note.trim().to_string());
        }
        rec.ts = eval_store::now_secs();
        let _ = eval_store::append(flows_dir, rec);
        self.graded += 1;

        if self.idx + 1 >= self.items.len() {
            self.done = true;
        } else {
            self.idx += 1;
            self.reset_for_current();
        }
        true
    }

    fn render(&mut self, f: &mut Frame) {
        let area = f.area();
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(1), // title
                Constraint::Min(3),    // transcript
                Constraint::Length(9), // ratings panel
            ])
            .split(area);

        let it = &self.items[self.idx];
        let r = &it.record;

        // Title bar.
        let title = format!(
            " review {}/{}  ·  {} [{} {} {}]  ·  {}/{} ",
            self.idx + 1,
            self.items.len(),
            r.fixture,
            r.suite,
            r.size,
            r.granularity,
            r.model,
            r.config,
        );
        f.render_widget(
            Paragraph::new(title).style(Style::default().fg(Color::Black).bg(Color::Cyan)),
            chunks[0],
        );

        // Transcript pane (scrollable). Count *wrapped* lines (each raw line
        // occupies ceil(len / width) rows) so max_scroll matches what's actually
        // rendered — a raw newline count under-counts wrapped long lines and would
        // wrongly pin scrolling near the top.
        self.view_h = chunks[1].height.saturating_sub(2); // minus borders
        let inner_w = chunks[1].width.saturating_sub(2).max(1) as usize;
        let wrapped: usize = it
            .transcript
            .lines()
            .map(|l| {
                let c = l.chars().count().max(1);
                c.div_ceil(inner_w)
            })
            .sum();
        self.total_lines = wrapped.min(u16::MAX as usize) as u16;
        let transcript = Paragraph::new(it.transcript.as_str())
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .title(" transcript  (↑↓/PgUp/PgDn/g/G · wheel) "),
            )
            .wrap(Wrap { trim: false })
            .scroll((self.scroll, 0));
        f.render_widget(transcript, chunks[1]);

        // Ratings panel.
        let mut lines: Vec<Line> = Vec::new();
        lines.push(Line::from(vec![
            Span::styled("task: ", Style::default().fg(Color::DarkGray)),
            Span::raw(truncate(&it.task, area.width.saturating_sub(8) as usize)),
        ]));
        // Dimension chips.
        let mut spans: Vec<Span> = vec![Span::styled(
            "rate 1-5  ",
            Style::default().fg(Color::DarkGray),
        )];
        for (d, name) in DIMS.iter().enumerate() {
            let val = if self.is_na(d) {
                "n/a".to_string()
            } else {
                self.values[d].map(|v| v.to_string()).unwrap_or("_".into())
            };
            let mut st = Style::default();
            if d == self.dim {
                st = st
                    .fg(Color::Black)
                    .bg(Color::Yellow)
                    .add_modifier(Modifier::BOLD);
            } else if self.values[d].is_some() {
                st = st.fg(Color::Green);
            } else if self.is_na(d) {
                st = st.fg(Color::DarkGray);
            }
            spans.push(Span::styled(format!(" {name}:{val} "), st));
        }
        lines.push(Line::from(spans));
        // Current dimension hint.
        lines.push(Line::from(Span::styled(
            format!("→ {}: {}", DIMS[self.dim], dim_hint(self.dim)),
            Style::default().fg(Color::Yellow),
        )));
        // Note.
        let note_line = if self.editing_note {
            Line::from(vec![
                Span::styled("note: ", Style::default().fg(Color::DarkGray)),
                Span::raw(&self.note),
                Span::styled("▏", Style::default().fg(Color::Yellow)),
            ])
        } else {
            Line::from(vec![
                Span::styled("note: ", Style::default().fg(Color::DarkGray)),
                Span::raw(if self.note.is_empty() {
                    "(press n to add)"
                } else {
                    &self.note
                }),
            ])
        };
        lines.push(note_line);
        lines.push(Line::from(Span::styled(
            "1-5 rate · Tab/←→ move dim · n note · Enter save+next · q quit",
            Style::default().fg(Color::DarkGray),
        )));
        if !self.status.is_empty() {
            lines.push(Line::from(Span::styled(
                self.status.clone(),
                Style::default().fg(Color::Red),
            )));
        }
        f.render_widget(
            Paragraph::new(lines).block(Block::default().borders(Borders::ALL).title(" ratings ")),
            chunks[2],
        );
    }
}

fn dim_hint(d: usize) -> &'static str {
    match d {
        0 => "output actually right?",
        1 => "no wasted rounds/tokens?",
        2 => "right tools, valid args, recovers?",
        3 => "used real context, no hallucination?",
        _ => "obeyed constraints + output format?",
    }
}

fn truncate(s: &str, n: usize) -> String {
    let one = s.lines().next().unwrap_or("");
    if one.chars().count() <= n {
        one.to_string()
    } else {
        format!(
            "{}…",
            one.chars().take(n.saturating_sub(1)).collect::<String>()
        )
    }
}

/// RAII guard: restores the terminal (raw mode off, leave alt screen, show
/// cursor) on drop — including during a panic unwind, so a crash in the TUI never
/// leaves the shell in a broken state.
struct TermGuard;

impl TermGuard {
    fn enter() -> io::Result<Self> {
        enable_raw_mode()?;
        execute!(io::stdout(), EnterAlternateScreen, EnableMouseCapture)?;
        Ok(TermGuard)
    }
}

impl Drop for TermGuard {
    fn drop(&mut self) {
        let _ = disable_raw_mode();
        let _ = execute!(
            io::stdout(),
            LeaveAlternateScreen,
            DisableMouseCapture,
            crossterm::cursor::Show
        );
    }
}

/// Run the review TUI over `items`, saving ratings as they are entered. Returns
/// the number of trials graded.
pub fn run_review(flows_dir: &Path, items: Vec<ReviewItem>) -> io::Result<usize> {
    let mut app = App::new(items);

    let _guard = TermGuard::enter()?;
    let mut terminal = Terminal::new(CrosstermBackend::new(io::stdout()))?;

    let res = event_loop(&mut terminal, &mut app, flows_dir);
    // `_guard` restores the terminal here (or on panic).
    res.map(|_| app.graded)
}

fn event_loop<B: ratatui::backend::Backend>(
    terminal: &mut Terminal<B>,
    app: &mut App,
    flows_dir: &Path,
) -> io::Result<()> {
    loop {
        terminal.draw(|f| app.render(f))?;
        match event::read()? {
            Event::Key(k) if k.kind == KeyEventKind::Press => {
                app.status.clear();
                if app.editing_note {
                    match k.code {
                        KeyCode::Esc | KeyCode::Enter => app.editing_note = false,
                        KeyCode::Backspace => {
                            app.note.pop();
                        }
                        KeyCode::Char(c) => app.note.push(c),
                        _ => {}
                    }
                } else if k.code == KeyCode::Char('c')
                    && k.modifiers.contains(KeyModifiers::CONTROL)
                {
                    // Raw mode delivers Ctrl-C as a keypress, not SIGINT — treat it
                    // as a clean quit so the terminal is always restored.
                    break;
                } else {
                    match k.code {
                        KeyCode::Char('q') | KeyCode::Esc => break,
                        KeyCode::Up | KeyCode::Char('k') => app.scroll_by(-1),
                        KeyCode::Down | KeyCode::Char('j') => app.scroll_by(1),
                        KeyCode::PageUp => app.scroll_by(-10),
                        KeyCode::PageDown => app.scroll_by(10),
                        KeyCode::Home | KeyCode::Char('g') => app.scroll = 0,
                        KeyCode::End | KeyCode::Char('G') => app.scroll = app.max_scroll(),
                        KeyCode::Tab | KeyCode::Right | KeyCode::Char('l') => app.next_dim(),
                        KeyCode::BackTab | KeyCode::Left | KeyCode::Char('h') => app.prev_dim(),
                        KeyCode::Char(c @ '1'..='5') => app.set_rating(c as u8 - b'0'),
                        KeyCode::Char('n') => app.editing_note = true,
                        KeyCode::Enter => {
                            app.save_current(flows_dir);
                        }
                        _ => {}
                    }
                }
            }
            Event::Mouse(m) => match m.kind {
                MouseEventKind::ScrollUp => app.scroll_by(-3),
                MouseEventKind::ScrollDown => app.scroll_by(3),
                _ => {}
            },
            _ => {}
        }
        if app.done {
            break;
        }
    }
    Ok(())
}
