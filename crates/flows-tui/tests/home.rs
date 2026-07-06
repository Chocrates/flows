//! Home-screen handler tests (no terminal) + a TestBackend render smoke test.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::backend::TestBackend;
use ratatui::Terminal;

use flows_tui::home::{FormFocus, HomeAction, HomeApp, Mode};

fn render(app: &mut HomeApp) {
    let mut term = Terminal::new(TestBackend::new(80, 24)).unwrap();
    term.draw(|f| app.render(f)).unwrap();
}

fn click(row: u16) -> MouseEvent {
    MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: 5,
        row,
        modifiers: KeyModifiers::NONE,
    }
}

fn app() -> HomeApp {
    HomeApp::new(
        vec!["blank".into(), "feature".into(), "software".into()],
        vec![
            ("abcd1234ef".into(), "completed".into(), "an old run".into()),
            ("beef5678aa".into(), "running".into(), "another run".into()),
        ],
    )
}

fn key(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}

fn typ(app: &mut HomeApp, s: &str) {
    for c in s.chars() {
        app.handle_key(key(KeyCode::Char(c)));
    }
}

// -- list mode ---------------------------------------------------------------

#[test]
fn starts_in_list_mode_on_new_button() {
    let a = app();
    assert_eq!(a.mode, Mode::List);
    assert_eq!(a.list_idx, 0); // the "New run" button
    assert!(a.selected_run().is_none());
}

#[test]
fn list_navigates_button_then_runs() {
    let mut a = app();
    a.handle_key(key(KeyCode::Down));
    assert_eq!(a.selected_run().map(|r| r.0.as_str()), Some("abcd1234ef"));
    a.handle_key(key(KeyCode::Down));
    assert_eq!(a.selected_run().map(|r| r.0.as_str()), Some("beef5678aa"));
    a.handle_key(key(KeyCode::Down)); // clamp at last run
    assert_eq!(a.selected_run().map(|r| r.0.as_str()), Some("beef5678aa"));
    a.handle_key(key(KeyCode::Up));
    a.handle_key(key(KeyCode::Up));
    assert_eq!(a.list_idx, 0);
}

#[test]
fn enter_on_run_attaches() {
    let mut a = app();
    a.handle_key(key(KeyCode::Down)); // first run
    a.handle_key(key(KeyCode::Enter));
    assert_eq!(
        a.action,
        Some(HomeAction::Attach {
            run_id: "abcd1234ef".into()
        })
    );
}

#[test]
fn q_and_esc_quit_from_list() {
    let mut a = app();
    a.handle_key(key(KeyCode::Char('q')));
    assert_eq!(a.action, Some(HomeAction::Quit));
    let mut b = app();
    b.handle_key(key(KeyCode::Esc));
    assert_eq!(b.action, Some(HomeAction::Quit));
}

// -- opening the form (the "button") -----------------------------------------

#[test]
fn enter_on_new_button_opens_form() {
    let mut a = app();
    a.handle_key(key(KeyCode::Enter)); // list_idx 0 = New run
    assert_eq!(a.mode, Mode::NewForm);
    assert_eq!(a.form_focus, FormFocus::Idea);
}

#[test]
fn n_key_opens_form() {
    let mut a = app();
    a.handle_key(key(KeyCode::Char('n')));
    assert_eq!(a.mode, Mode::NewForm);
}

// -- form mode ---------------------------------------------------------------

fn open_form() -> HomeApp {
    let mut a = app();
    a.handle_key(key(KeyCode::Enter));
    a
}

#[test]
fn form_typing_and_start() {
    let mut a = open_form();
    typ(&mut a, "add dark mode");
    assert_eq!(a.idea, "add dark mode");
    a.handle_key(key(KeyCode::Enter));
    assert_eq!(
        a.action,
        Some(HomeAction::StartRun {
            template: "feature".into(),
            idea: "add dark mode".into()
        })
    );
}

#[test]
fn form_empty_idea_does_not_start() {
    let mut a = open_form();
    a.handle_key(key(KeyCode::Enter));
    assert!(a.action.is_none());
}

#[test]
fn form_tab_and_template_selection() {
    let mut a = open_form();
    typ(&mut a, "x");
    a.handle_key(key(KeyCode::Tab)); // → Template
    assert_eq!(a.form_focus, FormFocus::Template);
    a.handle_key(key(KeyCode::Up)); // feature → blank
    assert_eq!(a.selected_template(), Some("blank"));
    a.handle_key(key(KeyCode::Enter));
    assert_eq!(
        a.action,
        Some(HomeAction::StartRun {
            template: "blank".into(),
            idea: "x".into()
        })
    );
}

#[test]
fn form_esc_returns_to_list_not_quit() {
    let mut a = open_form();
    a.handle_key(key(KeyCode::Esc));
    assert_eq!(a.mode, Mode::List);
    assert!(a.action.is_none());
}

#[test]
fn ctrl_c_quits_from_form() {
    let mut a = open_form();
    a.handle_key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL));
    assert_eq!(a.action, Some(HomeAction::Quit));
}

// -- render ------------------------------------------------------------------

#[test]
fn list_renders_new_button_and_runs() {
    let mut a = app();
    let mut term = Terminal::new(TestBackend::new(80, 24)).unwrap();
    term.draw(|f| a.render(f)).unwrap();
    let text: String = term
        .backend()
        .buffer()
        .content
        .iter()
        .map(|c| c.symbol())
        .collect();
    assert!(text.contains("New run"));
    assert!(text.contains("an old run"));
    assert!(text.contains("d delete"));
}

#[test]
fn form_renders_idea_and_templates() {
    let mut a = open_form();
    typ(&mut a, "hello");
    let mut term = Terminal::new(TestBackend::new(80, 24)).unwrap();
    term.draw(|f| a.render(f)).unwrap();
    let text: String = term
        .backend()
        .buffer()
        .content
        .iter()
        .map(|c| c.symbol())
        .collect();
    assert!(text.contains("new run"));
    assert!(text.contains("hello"));
    assert!(text.contains("feature"));
}

// -- mouse -------------------------------------------------------------------

#[test]
fn click_new_run_button_opens_form() {
    // layout: title y0, list y1 (border top y1, items from y2) → button at row 2
    let mut a = app();
    render(&mut a);
    a.handle_mouse(click(2));
    assert_eq!(a.mode, Mode::NewForm);
}

#[test]
fn click_run_row_attaches() {
    let mut a = app();
    render(&mut a);
    a.handle_mouse(click(3)); // button=2, first run=3
    assert_eq!(
        a.action,
        Some(HomeAction::Attach {
            run_id: "abcd1234ef".into()
        })
    );
}

#[test]
fn click_outside_list_does_nothing() {
    let mut a = app();
    render(&mut a);
    a.handle_mouse(click(0)); // title row
    assert!(a.action.is_none());
    assert_eq!(a.mode, Mode::List);
}

#[test]
fn wheel_scrolls_selection() {
    let mut a = app();
    render(&mut a);
    a.handle_mouse(MouseEvent {
        kind: MouseEventKind::ScrollDown,
        column: 5,
        row: 5,
        modifiers: KeyModifiers::NONE,
    });
    assert_eq!(a.list_idx, 1);
}

#[test]
fn click_template_in_form_selects_it() {
    // form: title y0, idea y1-3, template y4 (items from y5) → blank at y5
    let mut a = open_form();
    render(&mut a);
    a.handle_mouse(click(5));
    assert_eq!(a.selected_template(), Some("blank"));
    assert_eq!(a.form_focus, FormFocus::Template);
}

#[test]
fn ctrl_p_in_form_triggers_plan() {
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    let mut a = open_form();
    typ(&mut a, "add a widget");
    a.handle_key(KeyEvent::new(KeyCode::Char('p'), KeyModifiers::CONTROL));
    assert_eq!(
        a.action,
        Some(HomeAction::Plan {
            template: "feature".into(),
            idea: "add a widget".into()
        })
    );
}

#[test]
fn ctrl_p_with_empty_idea_does_nothing() {
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    let mut a = open_form();
    a.handle_key(KeyEvent::new(KeyCode::Char('p'), KeyModifiers::CONTROL));
    assert!(a.action.is_none());
}

#[test]
fn plain_p_types_into_idea_not_plan() {
    let mut a = open_form();
    typ(&mut a, "map");
    assert_eq!(a.idea, "map");
    assert!(a.action.is_none()); // plain 'p' is just a character
}

#[test]
fn d_deletes_finished_run_not_running_or_button() {
    use crossterm::event::KeyCode;
    let mut a = HomeApp::new(
        vec!["feature".into()],
        vec![
            ("done1".into(), "completed".into(), "x".into()),
            ("live1".into(), "running".into(), "y".into()),
        ],
    );
    // on the New-run button (idx 0): d does nothing
    a.handle_key(key(KeyCode::Char('d')));
    assert!(a.take_delete().is_none());
    // select the completed run → d marks it
    a.handle_key(key(KeyCode::Down));
    a.handle_key(key(KeyCode::Char('d')));
    assert_eq!(a.take_delete().as_deref(), Some("done1"));
    // select the running run → d refuses
    a.handle_key(key(KeyCode::Down));
    a.handle_key(key(KeyCode::Char('d')));
    assert!(a.take_delete().is_none());
}
