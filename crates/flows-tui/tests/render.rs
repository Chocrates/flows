//! Render tests over a plain ratatui Buffer (no terminal). Asserts the
//! selection highlight actually paints — the Python output-pane bug, which is
//! trivially correct in immediate mode since we set cell styles directly.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;

use flows_tui::output::Category;
use flows_tui::phases::PhaseStatus;
use flows_tui::render::{draw_output, draw_phases, SELECTION_BG};
use flows_tui::{OutputPane, PhasePanel};

fn buffer(w: u16, h: u16) -> (Buffer, Rect) {
    let area = Rect::new(0, 0, w, h);
    (Buffer::empty(area), area)
}

fn row_text(buf: &Buffer, y: u16, area: Rect) -> String {
    (0..area.width)
        .map(|x| buf[(x, y)].symbol().chars().next().unwrap_or(' '))
        .collect()
}

#[test]
fn output_draws_visible_lines() {
    let (mut buf, area) = buffer(20, 4);
    let mut pane = OutputPane::new();
    pane.set_viewport(4, 1000);
    for i in 0..3 {
        pane.push_line(Category::Normal, format!("line{i}"));
    }
    draw_output(&mut buf, area, &pane);
    assert_eq!(row_text(&buf, 0, area).trim_end(), "line0");
    assert_eq!(row_text(&buf, 2, area).trim_end(), "line2");
}

#[test]
fn selection_highlight_paints() {
    let (mut buf, area) = buffer(20, 2);
    let mut pane = OutputPane::new();
    pane.set_viewport(2, 1000);
    pane.push_line(Category::Normal, "0123456789");
    pane.start_selection(2, 0);
    pane.update_selection(6, 0);
    draw_output(&mut buf, area, &pane);

    // cols 2..6 highlighted, others not
    assert_ne!(buf[(1, 0)].bg, SELECTION_BG);
    for x in 2..6u16 {
        assert_eq!(
            buf[(x, 0)].bg,
            SELECTION_BG,
            "col {x} should be highlighted"
        );
    }
    assert_ne!(buf[(6, 0)].bg, SELECTION_BG); // exclusive end
}

#[test]
fn output_respects_scroll() {
    let (mut buf, area) = buffer(20, 3);
    let mut pane = OutputPane::new();
    pane.set_viewport(3, 1000);
    for i in 0..20 {
        pane.push_line(Category::Normal, format!("L{i:02}"));
    }
    // following → shows the last 3 lines
    draw_output(&mut buf, area, &pane);
    assert_eq!(row_text(&buf, 0, area).trim_end(), "L17");
    assert_eq!(row_text(&buf, 2, area).trim_end(), "L19");
}

#[test]
fn phases_draw_with_glyphs() {
    let (mut buf, area) = buffer(30, 4);
    let mut panel = PhasePanel::new();
    panel.setup(&["plan".into(), "implement".into(), "lint".into()]);
    panel.set_status("plan", PhaseStatus::Done);
    panel.set_status("implement", PhaseStatus::Running);
    draw_phases(&mut buf, area, &panel);
    assert!(row_text(&buf, 0, area).contains('✓'));
    assert!(row_text(&buf, 0, area).contains("plan"));
    assert!(row_text(&buf, 1, area).contains('▶'));
    assert!(row_text(&buf, 2, area).contains('○')); // pending lint
}

#[test]
fn phases_show_loop_iteration() {
    use flows_tui::phases::LoopEdge;
    let (mut buf, area) = buffer(30, 2);
    let mut panel = PhasePanel::new();
    panel.setup(&["implement".into(), "review".into()]);
    panel.set_loops(vec![LoopEdge {
        from: "review".into(),
        to: "implement".into(),
        max: 3,
    }]);
    panel.set_loop_iteration("review", 2);
    draw_phases(&mut buf, area, &panel);
    // loop source shows glyph + persistent iteration budget
    let row = row_text(&buf, 1, area);
    assert!(row.contains('↻'));
    assert!(row.contains("↺2/3"));
    // gutter bracket connects the target (top) back up from the source
    let top = row_text(&buf, 0, area);
    assert!(top.contains('╭'));
    let bottom = row_text(&buf, 1, area);
    assert!(bottom.contains('╰'));
}
