//! Standalone launcher / home screen. Primary view is the list of recent runs
//! with a "＋ New run" button at the top; selecting it opens a small form (idea
//! + template) to start a run. Returns an action the outer loop acts on.

use std::io;
use std::time::Duration;

use crossterm::event::{
    self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEvent, KeyEventKind,
    KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::widgets::{Block, Borders, List, ListItem, ListState, Paragraph};
use ratatui::{Frame, Terminal};

use flows_core::state::Store;

/// Which screen the home app is showing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// Recent runs list (with the "New run" button at index 0).
    List,
    /// The new-run form (idea + template).
    NewForm,
}

/// Focused field on the new-run form.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FormFocus {
    Idea,
    Template,
}

#[derive(Debug, Clone, PartialEq)]
pub enum HomeAction {
    StartRun {
        template: String,
        idea: String,
    },
    /// Preview the plan + cost estimate (dry run) before starting.
    Plan {
        template: String,
        idea: String,
    },
    Attach {
        run_id: String,
    },
    Quit,
}

/// A recent run row: (run_id, status, idea).
pub type RunRow = (String, String, String);

pub struct HomeApp {
    pub mode: Mode,
    // list view
    pub runs: Vec<RunRow>,
    /// 0 = the "New run" button; 1.. = runs.
    pub list_idx: usize,
    // new-run form
    pub idea: String,
    pub templates: Vec<String>,
    pub template_idx: usize,
    pub form_focus: FormFocus,

    pub action: Option<HomeAction>,
    /// A finished run the user asked to delete (consumed by the outer loop).
    pending_delete: Option<String>,
    // last-rendered widget rects, for mouse hit-testing
    list_area: Rect,
    template_area: Rect,
}

impl HomeApp {
    pub fn new(templates: Vec<String>, runs: Vec<RunRow>) -> Self {
        let template_idx = templates.iter().position(|t| t == "feature").unwrap_or(0);
        Self {
            mode: Mode::List,
            runs,
            list_idx: 0,
            idea: String::new(),
            templates,
            template_idx,
            form_focus: FormFocus::Idea,
            action: None,
            pending_delete: None,
            list_area: Rect::default(),
            template_area: Rect::default(),
        }
    }

    /// Take a pending run deletion (the outer loop removes the dir + refreshes).
    pub fn take_delete(&mut self) -> Option<String> {
        self.pending_delete.take()
    }

    fn request_delete(&mut self) {
        // Only finished runs — don't yank the directory out from under a live worker.
        if let Some((id, status, _)) = self.selected_run() {
            if matches!(status.as_str(), "completed" | "failed" | "cancelled") {
                self.pending_delete = Some(id.clone());
            }
        }
    }

    /// Refresh the runs list without disturbing the current selection.
    pub fn set_runs(&mut self, runs: Vec<RunRow>) {
        self.runs = runs;
        let max = self.runs.len(); // list has runs.len()+1 rows (button + runs)
        if self.list_idx > max {
            self.list_idx = max;
        }
    }

    pub fn selected_template(&self) -> Option<&str> {
        self.templates.get(self.template_idx).map(String::as_str)
    }

    /// The run selected in the list, if the selection isn't the New-run button.
    pub fn selected_run(&self) -> Option<&RunRow> {
        if self.list_idx == 0 {
            None
        } else {
            self.runs.get(self.list_idx - 1)
        }
    }

    fn open_form(&mut self) {
        self.mode = Mode::NewForm;
        self.form_focus = FormFocus::Idea;
        self.idea.clear();
    }

    fn start(&mut self) {
        if self.idea.trim().is_empty() {
            return;
        }
        if let Some(t) = self.selected_template() {
            self.action = Some(HomeAction::StartRun {
                template: t.to_string(),
                idea: self.idea.trim().to_string(),
            });
        }
    }

    pub fn handle_key(&mut self, key: KeyEvent) {
        // Ctrl+C always quits.
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            self.action = Some(HomeAction::Quit);
            return;
        }
        // Ctrl+P in the new-run form → preview the plan (dry run).
        if self.mode == Mode::NewForm
            && key.modifiers.contains(KeyModifiers::CONTROL)
            && key.code == KeyCode::Char('p')
        {
            if let Some(t) = self.selected_template() {
                if !self.idea.trim().is_empty() {
                    self.action = Some(HomeAction::Plan {
                        template: t.to_string(),
                        idea: self.idea.trim().to_string(),
                    });
                }
            }
            return;
        }
        match self.mode {
            Mode::List => self.handle_list_key(key),
            Mode::NewForm => self.handle_form_key(key),
        }
    }

    fn handle_list_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') => self.action = Some(HomeAction::Quit),
            KeyCode::Up => self.list_idx = self.list_idx.saturating_sub(1),
            KeyCode::Down if self.list_idx < self.runs.len() => self.list_idx += 1,
            KeyCode::Char('n') => self.open_form(),
            KeyCode::Char('d') => self.request_delete(),
            KeyCode::Enter => self.activate_list(),
            _ => {}
        }
    }

    /// Act on the current list selection: New-run button → form, run → attach.
    fn activate_list(&mut self) {
        if self.list_idx == 0 {
            self.open_form();
        } else if let Some((id, _, _)) = self.selected_run() {
            self.action = Some(HomeAction::Attach { run_id: id.clone() });
        }
    }

    /// Mouse: click a list row / template to select (and, in the list, activate);
    /// wheel scrolls the list selection.
    pub fn handle_mouse(&mut self, m: MouseEvent) {
        match self.mode {
            Mode::List => match m.kind {
                MouseEventKind::ScrollUp => self.list_idx = self.list_idx.saturating_sub(1),
                MouseEventKind::ScrollDown if self.list_idx < self.runs.len() => self.list_idx += 1,
                MouseEventKind::Down(MouseButton::Left) => {
                    if let Some(idx) = row_in_list(&self.list_area, m.row) {
                        if idx <= self.runs.len() {
                            self.list_idx = idx;
                            self.activate_list();
                        }
                    }
                }
                _ => {}
            },
            Mode::NewForm => {
                if let MouseEventKind::Down(MouseButton::Left) = m.kind {
                    if let Some(idx) = row_in_list(&self.template_area, m.row) {
                        if idx < self.templates.len() {
                            self.template_idx = idx;
                            self.form_focus = FormFocus::Template;
                        }
                    }
                }
            }
        }
    }

    fn handle_form_key(&mut self, key: KeyEvent) {
        // Esc goes back to the list (does not quit).
        if key.code == KeyCode::Esc {
            self.mode = Mode::List;
            return;
        }
        if key.code == KeyCode::Tab {
            self.form_focus = match self.form_focus {
                FormFocus::Idea => FormFocus::Template,
                FormFocus::Template => FormFocus::Idea,
            };
            return;
        }
        match self.form_focus {
            FormFocus::Idea => match key.code {
                KeyCode::Char(c) => self.idea.push(c),
                KeyCode::Backspace => {
                    self.idea.pop();
                }
                KeyCode::Enter => self.start(),
                KeyCode::Down => self.form_focus = FormFocus::Template,
                _ => {}
            },
            FormFocus::Template => match key.code {
                KeyCode::Up => self.template_idx = self.template_idx.saturating_sub(1),
                KeyCode::Down if self.template_idx + 1 < self.templates.len() => {
                    self.template_idx += 1
                }
                KeyCode::Enter => self.start(),
                _ => {}
            },
        }
    }

    // -- rendering -------------------------------------------------------

    pub fn render(&mut self, f: &mut Frame) {
        match self.mode {
            Mode::List => self.render_list(f),
            Mode::NewForm => self.render_form(f),
        }
    }

    fn render_list(&mut self, f: &mut Frame) {
        let rows = Layout::vertical([
            Constraint::Length(1),
            Constraint::Min(3),
            Constraint::Length(1),
        ])
        .split(f.area());

        f.render_widget(
            Paragraph::new("flows").style(Style::default().add_modifier(Modifier::BOLD)),
            rows[0],
        );

        let mut items = vec![ListItem::new("＋ New run").style(
            Style::default()
                .fg(Color::Green)
                .add_modifier(Modifier::BOLD),
        )];
        items.extend(self.runs.iter().map(|(id, status, idea)| {
            let short: String = id.chars().take(8).collect();
            let idea: String = idea.chars().take(48).collect();
            ListItem::new(format!("{short}  {status:<12}  {idea}"))
        }));

        let mut state = ListState::default();
        state.select(Some(self.list_idx));
        let list = List::new(items)
            .block(Block::default().borders(Borders::ALL).title(" runs "))
            .highlight_symbol("▶ ")
            .highlight_style(Style::default().add_modifier(Modifier::REVERSED));
        self.list_area = rows[1];
        f.render_stateful_widget(list, rows[1], &mut state);

        f.render_widget(
            Paragraph::new("↑↓ select · Enter open/attach · n new run · d delete · q quit")
                .style(Style::default().fg(Color::DarkGray)),
            rows[2],
        );
    }

    fn render_form(&mut self, f: &mut Frame) {
        let rows = Layout::vertical([
            Constraint::Length(1),
            Constraint::Length(3),
            Constraint::Min(3),
            Constraint::Length(1),
        ])
        .split(f.area());

        f.render_widget(
            Paragraph::new("flows — new run").style(Style::default().add_modifier(Modifier::BOLD)),
            rows[0],
        );

        let idea_border = if self.form_focus == FormFocus::Idea {
            Style::default().fg(Color::Cyan)
        } else {
            Style::default().fg(Color::DarkGray)
        };
        f.render_widget(
            Paragraph::new(self.idea.as_str()).block(
                Block::default()
                    .borders(Borders::ALL)
                    .title(" idea ")
                    .border_style(idea_border),
            ),
            rows[1],
        );

        let tmpl_border = if self.form_focus == FormFocus::Template {
            Style::default().fg(Color::Cyan)
        } else {
            Style::default().fg(Color::DarkGray)
        };
        let items: Vec<ListItem> = self
            .templates
            .iter()
            .map(|t| ListItem::new(t.clone()))
            .collect();
        let mut state = ListState::default();
        state.select(Some(self.template_idx));
        let list = List::new(items)
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .title(" template ")
                    .border_style(tmpl_border),
            )
            .highlight_symbol("▶ ")
            .highlight_style(
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            );
        self.template_area = rows[2];
        f.render_stateful_widget(list, rows[2], &mut state);

        f.render_widget(
            Paragraph::new("Tab switch · ↑↓ template · Enter start · ^P plan · Esc back")
                .style(Style::default().fg(Color::DarkGray)),
            rows[3],
        );
    }
}

// ---------------------------------------------------------------------------
// Terminal loop + orchestrator
// ---------------------------------------------------------------------------

/// The 0-based item index a mouse row falls on inside a bordered List widget
/// (1-row top border), or None if outside the item area.
fn row_in_list(area: &Rect, mouse_row: u16) -> Option<usize> {
    let top = area.y + 1; // skip the border
    let bottom = area.y + area.height.saturating_sub(1);
    if mouse_row >= top && mouse_row < bottom {
        Some((mouse_row - top) as usize)
    } else {
        None
    }
}

fn run_rows(store: &Store) -> Vec<RunRow> {
    store
        .list_runs()
        .into_iter()
        .map(|m| {
            let status = store
                .read_state(&m.run_id)
                .ok()
                .flatten()
                .map(|s| s.status.to_string())
                .unwrap_or_default();
            (m.run_id, status, m.idea)
        })
        .collect()
}

/// Draw the home screen until the user picks an action.
/// Show the launcher home screen until the user picks an action. Sets up and
/// tears down its own terminal, so the caller can act on the returned
/// [`HomeAction`] (spawn a run, attach, quit) then call again to loop.
pub fn home_screen(templates: &[String], store: &Store) -> io::Result<HomeAction> {
    let mut app = HomeApp::new(templates.to_vec(), run_rows(store));

    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    let mut terminal = Terminal::new(CrosstermBackend::new(stdout))?;

    let action = loop {
        terminal.draw(|f| app.render(f))?;
        if event::poll(Duration::from_millis(500))? {
            match event::read()? {
                Event::Key(k) if k.kind == KeyEventKind::Press => app.handle_key(k),
                Event::Mouse(m) => app.handle_mouse(m),
                _ => {}
            }
        } else if app.mode == Mode::List {
            // idle tick — refresh the runs list so background progress shows
            app.set_runs(run_rows(store));
        }
        // Apply a requested deletion, then refresh the list.
        if let Some(id) = app.take_delete() {
            let _ = store.delete_run(&id);
            app.set_runs(run_rows(store));
        }
        if let Some(a) = app.action.take() {
            break a;
        }
    };

    let _ = disable_raw_mode();
    let _ = execute!(
        terminal.backend_mut(),
        LeaveAlternateScreen,
        DisableMouseCapture
    );
    let _ = terminal.show_cursor();
    Ok(action)
}

/// Show a dry-run plan + cost estimate. Returns true to start the run, false to
/// go back to the launcher. Sets up + tears down its own terminal.
pub fn plan_screen(
    report: &flows_core::dryrun::DryRunReport,
    template: &str,
    idea: &str,
) -> io::Result<bool> {
    use flows_core::pricing::format_cost;

    // Build the scrollable body once (dry run is already computed).
    let mut lines: Vec<String> = Vec::new();
    lines.push(format!("idea:     {idea}"));
    lines.push(format!("template: {template}"));
    lines.push(String::new());
    lines.push("Execution plan:".into());
    lines.extend(report.plan_lines.iter().cloned());
    lines.push(String::new());
    lines.push("Cost estimate:".into());
    for e in &report.estimates {
        if e.model.is_empty() {
            lines.push(format!("  {:<18} [{}]  —", e.phase, e.phase_type.as_str()));
        } else {
            lines.push(format!(
                "  {:<18} {}  {}–{}",
                e.phase,
                e.model,
                format_cost(e.cost_low),
                format_cost(e.cost_high)
            ));
        }
    }
    lines.push(String::new());
    lines.push(format!(
        "Total: {}–{}",
        format_cost(report.total_cost_low),
        format_cost(report.total_cost_high)
    ));
    if !report.errors.is_empty() {
        lines.push(String::new());
        lines.push("Validation errors:".into());
        lines.extend(report.errors.iter().map(|e| format!("  - {e}")));
    }
    let has_errors = !report.errors.is_empty();

    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let mut terminal = Terminal::new(CrosstermBackend::new(stdout))?;

    let mut scroll: u16 = 0;
    let run = loop {
        terminal.draw(|f| {
            let rows = Layout::vertical([
                Constraint::Length(1),
                Constraint::Min(1),
                Constraint::Length(1),
            ])
            .split(f.area());
            f.render_widget(
                Paragraph::new(" plan preview (dry run — no LLM calls) ")
                    .style(Style::default().add_modifier(Modifier::BOLD)),
                rows[0],
            );
            let body = lines.join("\n");
            f.render_widget(
                Paragraph::new(body)
                    .scroll((scroll, 0))
                    .block(Block::default().borders(Borders::ALL)),
                rows[1],
            );
            let footer = if has_errors {
                "config has errors — fix them first · ↑↓ scroll · Esc back"
            } else {
                "Enter start this run · ↑↓ scroll · Esc back"
            };
            f.render_widget(
                Paragraph::new(footer).style(Style::default().fg(Color::DarkGray)),
                rows[2],
            );
        })?;

        if event::poll(Duration::from_millis(500))? {
            if let Event::Key(k) = event::read()? {
                if k.kind == KeyEventKind::Press {
                    match k.code {
                        KeyCode::Enter if !has_errors => break true,
                        KeyCode::Esc | KeyCode::Char('q') => break false,
                        KeyCode::Down => scroll = scroll.saturating_add(1),
                        KeyCode::Up => scroll = scroll.saturating_sub(1),
                        _ => {}
                    }
                }
            }
        }
    };

    let _ = disable_raw_mode();
    let _ = execute!(terminal.backend_mut(), LeaveAlternateScreen);
    let _ = terminal.show_cursor();
    Ok(run)
}
