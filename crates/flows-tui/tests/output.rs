//! Output-pane model tests — port of the CopyableRichLog behavior (sticky
//! scroll, selection-suspends-follow, toggles, extraction).

use flows_tui::output::Category;
use flows_tui::OutputPane;

fn pane(h: usize) -> OutputPane {
    let mut p = OutputPane::new();
    p.set_viewport(h, 1000);
    p
}

fn fill(p: &mut OutputPane, n: usize) {
    for i in 0..n {
        p.push_line(Category::Normal, format!("line {i:02}"));
    }
}

// -- sticky follow -----------------------------------------------------------

#[test]
fn follows_bottom_on_new_output() {
    let mut p = pane(8);
    fill(&mut p, 40);
    assert!(p.following());
    assert_eq!(p.scroll_y(), p.max_scroll());
}

#[test]
fn fast_burst_stays_pinned() {
    let mut p = pane(8);
    for burst in 0..12 {
        for i in 0..4 {
            p.push_line(Category::Normal, format!("b{burst} l{i}"));
        }
        assert_eq!(p.scroll_y(), p.max_scroll(), "fell behind at burst {burst}");
    }
}

#[test]
fn scroll_up_stops_following() {
    let mut p = pane(8);
    fill(&mut p, 40);
    p.scroll_up(3);
    assert!(!p.following());
    let pos = p.scroll_y();
    fill(&mut p, 10); // more output must NOT yank the view down
    assert_eq!(p.scroll_y(), pos);
}

#[test]
fn scroll_back_to_bottom_resumes_following() {
    let mut p = pane(8);
    fill(&mut p, 40);
    p.scroll_up(5);
    assert!(!p.following());
    p.scroll_to_bottom();
    assert!(p.following());
    let before = p.max_scroll();
    fill(&mut p, 10);
    assert!(p.max_scroll() > before);
    assert_eq!(p.scroll_y(), p.max_scroll()); // followed again
}

#[test]
fn active_selection_suspends_follow() {
    // The key streaming-selection fix: while a selection exists, new output must
    // not scroll the view (so the highlight stays put).
    let mut p = pane(8);
    fill(&mut p, 40);
    assert_eq!(p.scroll_y(), p.max_scroll());
    let frozen = p.scroll_y();

    p.start_selection(0, 2);
    p.update_selection(4, 2);
    fill(&mut p, 20); // streams in during the selection

    assert_eq!(p.scroll_y(), frozen, "view must not scroll while selecting");
    // clearing the selection lets follow resume
    p.clear_selection();
    p.push_line(Category::Normal, "more");
    assert_eq!(p.scroll_y(), p.max_scroll());
}

// -- toggles -----------------------------------------------------------------

#[test]
fn toggle_thinking_hides_lines() {
    let mut p = pane(24);
    p.push_line(Category::Normal, "prose");
    p.push_line(Category::Thinking, "considering");
    p.push_line(Category::Json, "{\"x\":1}");
    assert_eq!(p.visible_count(), 3);
    p.toggle_thinking();
    assert!(!p.visible_lines().iter().any(|l| l == "considering"));
    assert_eq!(p.visible_count(), 2);
    p.toggle_json();
    assert_eq!(p.visible_lines(), vec!["prose".to_string()]);
}

// -- streaming tokens --------------------------------------------------------

#[test]
fn tokens_assemble_into_lines() {
    let mut p = pane(24);
    for tok in ["Hel", "lo\n", "wor", "ld\n"] {
        p.push_token(Category::Normal, tok);
    }
    assert_eq!(
        p.visible_lines(),
        vec!["Hello".to_string(), "world".to_string()]
    );
}

#[test]
fn partial_token_line_is_visible() {
    let mut p = pane(24);
    p.push_token(Category::Normal, "in progress");
    assert_eq!(p.visible_lines(), vec!["in progress".to_string()]);
}

// -- selection extraction ----------------------------------------------------

#[test]
fn selection_single_line() {
    let mut p = pane(24);
    p.push_line(Category::Normal, "0123456789");
    p.start_selection(2, 0);
    p.update_selection(6, 0);
    assert_eq!(p.selected_text().as_deref(), Some("2345"));
    assert!(p.has_selection());
}

#[test]
fn selection_multi_line() {
    let mut p = pane(24);
    p.push_line(Category::Normal, "abcde");
    p.push_line(Category::Normal, "fghij");
    p.push_line(Category::Normal, "klmno");
    p.start_selection(2, 0); // from 'c'
    p.update_selection(3, 2); // to 'n' (exclusive at col 3 on line 2)
    assert_eq!(p.selected_text().as_deref(), Some("cde\nfghij\nklm"));
}

#[test]
fn empty_selection_is_none() {
    let mut p = pane(24);
    p.push_line(Category::Normal, "abc");
    p.start_selection(1, 0); // no drag
    assert!(p.selected_text().is_none());
    assert!(!p.has_selection());
}

#[test]
fn is_selected_marks_span() {
    let mut p = pane(24);
    p.push_line(Category::Normal, "0123456789");
    p.start_selection(2, 0);
    p.update_selection(6, 0);
    assert!(!p.is_selected(0, 1));
    assert!(p.is_selected(0, 2));
    assert!(p.is_selected(0, 5));
    assert!(!p.is_selected(0, 6)); // exclusive end
}

#[test]
fn all_text_joins_visible() {
    let mut p = pane(24);
    p.push_line(Category::Normal, "a");
    p.push_line(Category::Thinking, "t");
    p.push_line(Category::Normal, "b");
    assert_eq!(p.all_text(), "a\nt\nb");
    p.toggle_thinking();
    assert_eq!(p.all_text(), "a\nb");
}

// -- Python-parity scroll: pin / unpin / re-pin / Home-End ------------------

#[test]
fn scroll_up_unpins_then_new_output_does_not_jump() {
    let mut p = pane(8);
    fill(&mut p, 40);
    p.scroll_up(5); // user scrolls up → unpin
    assert!(!p.following());
    let y = p.scroll_y();
    // streaming more output must NOT yank the view to the bottom
    fill(&mut p, 5);
    assert_eq!(p.scroll_y(), y);
    assert!(!p.following());
}

#[test]
fn scroll_down_to_bottom_repins() {
    let mut p = pane(8);
    fill(&mut p, 40);
    p.scroll_up(20);
    assert!(!p.following());
    p.scroll_down(20); // back to bottom
    assert!(p.following());
    assert_eq!(p.scroll_y(), p.max_scroll());
}

#[test]
fn home_goes_to_top_and_unpins_end_repins() {
    let mut p = pane(8);
    fill(&mut p, 40);
    p.scroll_to_top();
    assert_eq!(p.scroll_y(), 0);
    assert!(!p.following());
    p.scroll_to_bottom();
    assert!(p.following());
    assert_eq!(p.scroll_y(), p.max_scroll());
}

#[test]
fn partial_scroll_down_stays_unpinned() {
    let mut p = pane(8);
    fill(&mut p, 40);
    p.scroll_to_top();
    p.scroll_down(3); // still above bottom
    assert!(!p.following());
    assert_eq!(p.scroll_y(), 3);
}

// -- long-line wrapping + system category ------------------------------------

#[test]
fn long_line_wraps_to_width_not_truncated() {
    let mut p = OutputPane::new();
    p.set_viewport(10, 10); // 10-col content width
    p.push_line(Category::Normal, "aaaa bbbb cccc dddd eeee ffff"); // 29 chars

    let lines = p.visible_lines();
    // Wrapped into multiple display lines, each within the width.
    assert!(lines.len() > 1, "should wrap: {lines:?}");
    assert!(lines.iter().all(|l| l.chars().count() <= 10));
    // No text lost — the words all survive across the wrap.
    let joined: String = lines.join("");
    for w in ["aaaa", "bbbb", "cccc", "dddd", "eeee", "ffff"] {
        assert!(joined.contains(w), "lost `{w}`");
    }
    // Scroll math counts wrapped rows, so the bottom is reachable.
    assert_eq!(p.visible_count(), lines.len());
}

#[test]
fn system_category_hidden_until_toggled() {
    let mut p = OutputPane::new();
    p.set_viewport(24, 80);
    p.push_line(Category::System, "secret system prompt");
    assert!(!p.visible_lines().iter().any(|l| l.contains("secret")));
    p.toggle_system();
    assert!(p.visible_lines().iter().any(|l| l.contains("secret")));
}

#[test]
fn meta_always_visible_regardless_of_system_toggle() {
    let mut p = OutputPane::new();
    p.set_viewport(24, 80);
    p.push_line(Category::Meta, "▶ plan");
    p.push_line(Category::Meta, "▸ task: add a widget");
    p.push_line(Category::System, "you are an assistant"); // system prompt
                                                           // System prompt hidden by default, but Meta (markers/task) always shows.
    let vis = p.visible_lines().join("\n");
    assert!(vis.contains("▶ plan") && vis.contains("task: add a widget"));
    assert!(!vis.contains("you are an assistant"));
    // Toggling system doesn't affect Meta visibility.
    p.toggle_system();
    let vis = p.visible_lines().join("\n");
    assert!(vis.contains("▶ plan") && vis.contains("you are an assistant"));
}
