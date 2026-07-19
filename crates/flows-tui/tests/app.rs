//! App handler tests — UI events + key/mouse handling, no terminal.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::Rect;

use flows_tui::app::{App, UiEvent};
use flows_tui::PhaseStatus;

fn app() -> App {
    let mut a = App::new(&["plan".into(), "implement".into()]);
    a.set_output_area(Rect::new(0, 0, 40, 10));
    a
}

fn key(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}

fn mouse(kind: MouseEventKind, column: u16, row: u16) -> MouseEvent {
    MouseEvent {
        kind,
        column,
        row,
        modifiers: KeyModifiers::NONE,
    }
}

// -- pipeline events ---------------------------------------------------------

#[test]
fn phase_start_marks_running() {
    let mut a = app();
    a.handle_ui_event(UiEvent::PhaseStart {
        name: "plan".into(),
        index: 0,
        total: 2,
    });
    assert_eq!(a.phases.status_of("plan"), Some(PhaseStatus::Running));
    assert!(a.status.contains("plan"));
}

#[test]
fn tokens_append_to_output() {
    let mut a = app();
    a.handle_ui_event(UiEvent::Token {
        text: "hello ".into(),
    });
    a.handle_ui_event(UiEvent::Token {
        text: "world\n".into(),
    });
    assert!(a.output.visible_lines().iter().any(|l| l == "hello world"));
}

#[test]
fn phase_end_marks_done_but_not_over_failed() {
    let mut a = app();
    a.handle_ui_event(UiEvent::Failed {
        phase: "plan".into(),
        error: "boom".into(),
    });
    a.handle_ui_event(UiEvent::PhaseEnd {
        name: "plan".into(),
    });
    assert_eq!(a.phases.status_of("plan"), Some(PhaseStatus::Failed));
    assert!(a.finished);
}

#[test]
fn loop_back_sets_iteration() {
    let mut a = app();
    a.handle_ui_event(UiEvent::LoopBack {
        from: "implement".into(),
        iteration: 2,
    });
    assert_eq!(a.phases.status_of("implement"), Some(PhaseStatus::Looping));
}

#[test]
fn cost_event_updates_total_and_shows_in_status() {
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;
    let mut a = app();
    a.handle_ui_event(UiEvent::Cost { total_usd: 0.1853 });
    assert!((a.total_cost - 0.1853).abs() < 1e-9);

    let mut term = Terminal::new(TestBackend::new(80, 12)).unwrap();
    term.draw(|f| a.render(f)).unwrap();
    let text: String = term
        .backend()
        .buffer()
        .content
        .iter()
        .map(|c| c.symbol())
        .collect();
    assert!(text.contains("$0.1853"));
}

#[test]
fn complete_sets_finished() {
    let mut a = app();
    a.handle_ui_event(UiEvent::Complete {
        phases: vec!["plan".into()],
    });
    assert!(a.finished);
    assert!(a.status.contains("complete"));
}

// -- keys --------------------------------------------------------------------

#[test]
fn q_quits_and_c_cancels() {
    let mut a = app();
    a.handle_key(key(KeyCode::Char('c')));
    assert!(a.cancel_requested);
    a.handle_key(key(KeyCode::Char('q')));
    assert!(a.quit);
}

#[test]
fn toggles_thinking_and_json() {
    let mut a = app();
    assert!(a.output.show_thinking && a.output.show_json);
    a.handle_key(key(KeyCode::Char('t')));
    a.handle_key(key(KeyCode::Char('j')));
    assert!(!a.output.show_thinking);
    assert!(!a.output.show_json);
}

#[test]
fn scroll_keys_move_view() {
    let mut a = app();
    for i in 0..40 {
        a.handle_ui_event(UiEvent::Token {
            text: format!("line {i}\n"),
        });
    }
    let bottom = a.output.scroll_y();
    a.handle_key(key(KeyCode::Up));
    assert!(a.output.scroll_y() < bottom);
    a.handle_key(key(KeyCode::End));
    assert!(a.output.following());
}

// -- input mode --------------------------------------------------------------

#[test]
fn input_mode_collects_answer_and_replies() {
    let mut a = app();
    let (reply, mut rx) = tokio::sync::oneshot::channel();
    a.handle_ui_event(UiEvent::NeedInput {
        prompt: "?".into(),
        reply,
    });
    assert!(a.awaiting_input());

    for c in "blue".chars() {
        a.handle_key(key(KeyCode::Char(c)));
    }
    a.handle_key(key(KeyCode::Backspace)); // "blu"
    a.handle_key(key(KeyCode::Enter));

    assert!(!a.awaiting_input());
    assert_eq!(rx.try_recv().unwrap(), "blu");
}

#[test]
fn input_mode_keys_do_not_trigger_shortcuts() {
    let mut a = app();
    let (reply, _rx) = tokio::sync::oneshot::channel();
    a.handle_ui_event(UiEvent::NeedInput {
        prompt: "?".into(),
        reply,
    });
    // 'q' while typing should NOT quit
    a.handle_key(key(KeyCode::Char('q')));
    assert!(!a.quit);
    assert_eq!(a.input_prompt().unwrap().1, "q");
}

// -- mouse selection ---------------------------------------------------------

#[test]
fn mouse_drag_selects_text() {
    let mut a = app();
    a.handle_ui_event(UiEvent::Token {
        text: "0123456789\n".into(),
    });
    a.handle_mouse(mouse(MouseEventKind::Down(MouseButton::Left), 2, 0));
    a.handle_mouse(mouse(MouseEventKind::Drag(MouseButton::Left), 6, 0));
    a.handle_mouse(mouse(MouseEventKind::Up(MouseButton::Left), 6, 0));
    assert_eq!(a.output.selected_text().as_deref(), Some("2345"));
}

#[test]
fn mouse_wheel_scrolls() {
    let mut a = app();
    for i in 0..40 {
        a.handle_ui_event(UiEvent::Token {
            text: format!("line {i}\n"),
        });
    }
    let bottom = a.output.scroll_y();
    a.handle_mouse(mouse(MouseEventKind::ScrollUp, 20, 5));
    assert!(a.output.scroll_y() < bottom);
    assert!(!a.output.following());
}

#[test]
fn help_bar_shows_toggle_state_and_flips() {
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;
    let mut a = app();
    let mut term = Terminal::new(TestBackend::new(90, 12)).unwrap();

    let text = |t: &mut Terminal<TestBackend>, a: &mut App| -> String {
        t.draw(|f| a.render(f)).unwrap();
        t.backend()
            .buffer()
            .content
            .iter()
            .map(|c| c.symbol())
            .collect()
    };

    let out = text(&mut term, &mut a);
    assert!(out.contains("thinking"));
    assert!(out.contains("JSON"));
    assert!(out.contains("quit"));
    assert!(out.contains("on")); // both start enabled

    a.handle_key(key(KeyCode::Char('t')));
    a.handle_key(key(KeyCode::Char('j')));
    let out = text(&mut term, &mut a);
    assert!(out.contains("off")); // toggles reflect the new state
}

#[test]
fn json_tokens_hidden_by_j_toggle() {
    let mut a = app();
    a.handle_ui_event(UiEvent::Token {
        text: "prose\n".into(),
    });
    a.handle_ui_event(UiEvent::Json {
        text: "{\"artifact\":1}\n".into(),
    });
    assert!(a
        .output
        .visible_lines()
        .iter()
        .any(|l| l.contains("artifact")));
    a.handle_key(key(KeyCode::Char('j'))); // hide JSON
    let vis = a.output.visible_lines().join("\n");
    assert!(!vis.contains("artifact"));
    assert!(vis.contains("prose")); // normal survives
}

#[test]
fn system_prompt_event_shows_when_toggled() {
    let mut a = app();
    a.handle_ui_event(UiEvent::System {
        text: "SYSTEM LINE ONE\nSYSTEM LINE TWO".into(),
    });
    // Hidden by default.
    let vis = a.output.visible_lines().join("\n");
    assert!(
        !vis.contains("SYSTEM LINE ONE"),
        "should be hidden by default"
    );
    // Toggle on → present.
    a.handle_key(key(KeyCode::Char('s')));
    let vis = a.output.visible_lines().join("\n");
    assert!(
        vis.contains("SYSTEM LINE ONE"),
        "toggled on but empty:\n{vis}"
    );
    assert!(vis.contains("SYSTEM LINE TWO"));
    assert!(vis.contains("system prompt"));
}

#[test]
fn input_mode_view_controls_still_work() {
    let mut a = app();
    for _ in 0..50 {
        a.handle_ui_event(UiEvent::Token { text: "x\n".into() });
    }
    let (reply, _rx) = tokio::sync::oneshot::channel();
    a.handle_ui_event(UiEvent::NeedInput {
        prompt: "?".into(),
        reply,
    });
    let before = a.output.show_thinking;
    a.handle_key(key(KeyCode::Char('t'))); // types
    assert_eq!(a.output.show_thinking, before);
    assert_eq!(a.input_prompt().unwrap().1, "t");
    a.handle_key(KeyEvent::new(KeyCode::Char('t'), KeyModifiers::ALT)); // toggles
    assert_eq!(a.output.show_thinking, !before);
    assert_eq!(a.input_prompt().unwrap().1, "t");
    a.handle_key(key(KeyCode::Up));
    assert!(!a.output.following());
}
