//! ratatui rendering for the phases + output panes. Pure draw functions over a
//! `Buffer`, so they can be unit-tested with `Buffer::empty` (no terminal).

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};

use crate::output::{Category, OutputPane};
use crate::phases::{PhasePanel, PhaseStatus};

/// Selection highlight background — the same colour Textual composited for
/// `screen--selection` (#094472), so the two implementations look identical.
pub const SELECTION_BG: Color = Color::Rgb(9, 68, 114);

fn status_style(status: PhaseStatus) -> Style {
    let base = Style::default();
    match status {
        PhaseStatus::Pending => base.fg(Color::DarkGray),
        PhaseStatus::Running => base.fg(Color::Cyan).add_modifier(Modifier::BOLD),
        PhaseStatus::Looping => base.fg(Color::Magenta),
        PhaseStatus::Done => base.fg(Color::Green),
        PhaseStatus::Failed => base.fg(Color::Red),
        PhaseStatus::Skipped => base.fg(Color::DarkGray).add_modifier(Modifier::DIM),
    }
}

/// Draw the phases sidebar, with a left-gutter bracket connecting each loop's
/// source back up to its target so loops are visually obvious.
pub fn draw_phases(buf: &mut Buffer, area: Rect, panel: &PhasePanel) {
    // Reserve a 2-col gutter for loop brackets when any loop exists.
    let gutter: u16 = if panel.has_loops() { 2 } else { 0 };

    for (top, bottom, active) in panel.loop_spans() {
        let style = if active {
            Style::default()
                .fg(Color::Magenta)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::DarkGray)
        };
        for row in top..=bottom {
            if row as u16 >= area.height {
                break;
            }
            let y = area.y + row as u16;
            // vertical bar spans the loop; ╭ at the target (top), ╰ at source.
            let ch = if row == top {
                '╭'
            } else if row == bottom {
                '╰'
            } else {
                '│'
            };
            buf.set_string(area.x, y, ch.to_string(), style);
        }
        // arrow into the target row: the loop jumps back up to here.
        if (top as u16) < area.height {
            buf.set_string(area.x + 1, area.y + top as u16, "→", style);
        }
    }

    let text_w = area.width.saturating_sub(gutter) as usize;
    for (row, (glyph, label, status)) in panel.rows().into_iter().enumerate() {
        if row as u16 >= area.height {
            break;
        }
        let y = area.y + row as u16;
        let text = format!("{glyph} {label}");
        let text: String = text.chars().take(text_w).collect();
        buf.set_string(area.x + gutter, y, &text, status_style(status));
    }
}

/// Per-category text style, so thinking / JSON / tool output are visually
/// distinct from normal prose even before toggling.
fn category_style(cat: Category) -> Style {
    match cat {
        Category::Normal => Style::default(),
        Category::Thinking => Style::default()
            .fg(Color::DarkGray)
            .add_modifier(Modifier::ITALIC),
        Category::Json => Style::default().fg(Color::Blue),
        Category::Tool => Style::default().fg(Color::Yellow),
        // System prompt reads as dim scaffolding; Meta (markers/task) stands out.
        Category::System => Style::default().fg(Color::DarkGray),
        Category::Meta => Style::default()
            .fg(Color::Cyan)
            .add_modifier(Modifier::BOLD),
    }
}

/// Draw the output pane, painting per-category text style and the selection
/// highlight. Thinking is dim italic, JSON blue, tool yellow, system cyan — so
/// categories are visually distinct without toggling.
pub fn draw_output(buf: &mut Buffer, area: Rect, pane: &OutputPane) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    let visible = pane.visible_entries();
    let start = pane.scroll_y();
    let sel = Style::default().bg(SELECTION_BG);

    for row in 0..area.height {
        let line_idx = start + row as usize;
        let Some((cat, line)) = visible.get(line_idx) else {
            break;
        };
        let y = area.y + row;
        let style = category_style(*cat);
        for (col, ch) in line.chars().enumerate() {
            if col as u16 >= area.width {
                break;
            }
            let x = area.x + col as u16;
            let cell = &mut buf[(x, y)];
            cell.set_char(ch);
            cell.set_style(style);
            if pane.is_selected(line_idx, col) {
                cell.set_style(sel);
            }
        }
    }
}
