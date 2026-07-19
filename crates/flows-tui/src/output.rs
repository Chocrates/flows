//! Output pane model — the scrollback buffer with sticky-follow scroll, category
//! toggles (thinking/JSON), and mouse text selection.
//!
//! This is the Rust re-implementation of the Textual `CopyableRichLog`, but as a
//! pure immediate-mode model: the app re-renders from this each frame, so the
//! Textual bugs (retained-render cache, compositor offset resolution,
//! anchor-released-on-resize, the post_style highlight loss) simply don't exist.
//! The two behaviors that DID matter carry over as explicit rules, tested here:
//!   * sticky follow — track the bottom unless the user scrolled up;
//!   * an active selection suspends follow, so streaming output can't scroll the
//!     highlighted text out from under the cursor.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Category {
    Normal,
    Thinking,
    Json,
    Tool,
    System,
}

#[derive(Debug, Clone)]
struct Entry {
    category: Category,
    text: String,
}

/// A content cell: (line index within the *visible* buffer, column).
pub type Cell = (usize, usize);

fn order(a: Cell, b: Cell) -> (Cell, Cell) {
    if a <= b {
        (a, b)
    } else {
        (b, a)
    }
}

fn slice_cols(line: &str, start: usize, end: usize) -> String {
    line.chars()
        .skip(start)
        .take(end.saturating_sub(start))
        .collect()
}

pub struct OutputPane {
    entries: Vec<Entry>,
    /// In-progress line being assembled from streamed tokens (no trailing '\n').
    current: Option<Entry>,
    scroll_y: usize,
    viewport_h: usize,
    /// Content width — long lines wrap to this so nothing is truncated off-screen.
    viewport_w: usize,
    follow: bool,
    pub show_thinking: bool,
    pub show_json: bool,
    /// System prompt is verbose; hidden by default, toggle to inspect it.
    pub show_system: bool,
    /// (anchor, cursor) in visible-buffer coordinates.
    selection: Option<(Cell, Cell)>,
}

impl Default for OutputPane {
    fn default() -> Self {
        Self {
            entries: Vec::new(),
            current: None,
            scroll_y: 0,
            viewport_h: 24,
            viewport_w: 80,
            follow: true,
            show_thinking: true,
            show_json: true,
            show_system: false,
            selection: None,
        }
    }
}

/// Wrap a logical line to `w` columns: greedy word-wrap, hard-breaking any word
/// longer than `w`. Returns ≥1 line (empty text → one empty line) so line counts
/// stay stable. Char-based (terminal columns), not byte-based.
fn wrap_line(text: &str, w: usize) -> Vec<String> {
    if w == 0 || text.chars().count() <= w {
        return vec![text.to_string()];
    }
    let mut lines = Vec::new();
    let mut cur = String::new();
    let mut cur_len = 0usize;
    for word in text.split_inclusive(' ') {
        let wl = word.chars().count();
        if cur_len > 0 && cur_len + wl > w {
            lines.push(std::mem::take(&mut cur));
            cur_len = 0;
        }
        if wl > w {
            for ch in word.chars() {
                if cur_len == w {
                    lines.push(std::mem::take(&mut cur));
                    cur_len = 0;
                }
                cur.push(ch);
                cur_len += 1;
            }
        } else {
            cur.push_str(word);
            cur_len += wl;
        }
    }
    if !cur.is_empty() || lines.is_empty() {
        lines.push(cur);
    }
    lines
}

impl OutputPane {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn set_viewport(&mut self, h: usize, w: usize) {
        self.viewport_h = h.max(1);
        self.viewport_w = w.max(1);
        self.apply_follow();
    }

    fn is_visible(&self, c: Category) -> bool {
        match c {
            Category::Thinking => self.show_thinking,
            Category::Json => self.show_json,
            Category::System => self.show_system,
            _ => true,
        }
    }

    // -- ingest ----------------------------------------------------------

    /// Append a complete line.
    pub fn push_line(&mut self, category: Category, text: impl Into<String>) {
        self.flush_current();
        self.entries.push(Entry {
            category,
            text: text.into(),
        });
        self.apply_follow();
    }

    /// Append a streamed token; splits on newlines into completed lines.
    pub fn push_token(&mut self, category: Category, token: &str) {
        if self.current.as_ref().map(|c| c.category) != Some(category) {
            self.flush_current();
            self.current = Some(Entry {
                category,
                text: String::new(),
            });
        }
        let cur = self.current.get_or_insert(Entry {
            category,
            text: String::new(),
        });
        cur.text.push_str(token);
        while let Some(nl) = cur.text.find('\n') {
            let line: String = cur.text[..nl].to_string();
            let rest = cur.text[nl + 1..].to_string();
            self.entries.push(Entry {
                category,
                text: line,
            });
            cur.text = rest;
        }
        self.apply_follow();
    }

    fn flush_current(&mut self) {
        if let Some(cur) = self.current.take() {
            if !cur.text.is_empty() {
                self.entries.push(cur);
            }
        }
    }

    // -- visible view ----------------------------------------------------

    /// Visible lines as plain strings (categories filtered out).
    pub fn visible_lines(&self) -> Vec<String> {
        self.visible_entries().into_iter().map(|(_, t)| t).collect()
    }

    /// Visible lines paired with their category, each **wrapped** to the viewport
    /// width — so scroll/selection/render all operate on the same display lines and
    /// long lines are never truncated.
    pub fn visible_entries(&self) -> Vec<(Category, String)> {
        let mut out: Vec<(Category, String)> = Vec::new();
        let push = |cat: Category, text: &str, out: &mut Vec<(Category, String)>| {
            for wl in wrap_line(text, self.viewport_w) {
                out.push((cat, wl));
            }
        };
        for e in &self.entries {
            if self.is_visible(e.category) {
                push(e.category, &e.text, &mut out);
            }
        }
        if let Some(cur) = &self.current {
            if !cur.text.is_empty() && self.is_visible(cur.category) {
                push(cur.category, &cur.text, &mut out);
            }
        }
        out
    }

    pub fn visible_count(&self) -> usize {
        self.visible_entries().len()
    }

    pub fn max_scroll(&self) -> usize {
        self.visible_count().saturating_sub(self.viewport_h)
    }

    pub fn scroll_y(&self) -> usize {
        self.scroll_y
    }

    pub fn following(&self) -> bool {
        self.follow
    }

    // -- scrolling -------------------------------------------------------

    fn apply_follow(&mut self) {
        // Follow the bottom on new content — UNLESS the user has a live
        // selection, which would otherwise scroll the highlight out of view.
        if self.follow && self.selection.is_none() {
            self.scroll_y = self.max_scroll();
        } else {
            self.scroll_y = self.scroll_y.min(self.max_scroll());
        }
    }

    pub fn scroll_up(&mut self, n: usize) {
        self.follow = false;
        self.scroll_y = self.scroll_y.saturating_sub(n);
    }

    pub fn scroll_down(&mut self, n: usize) {
        self.scroll_y = (self.scroll_y + n).min(self.max_scroll());
        // Reaching the bottom resumes following.
        if self.scroll_y == self.max_scroll() {
            self.follow = true;
        }
    }

    pub fn scroll_to_bottom(&mut self) {
        self.follow = true;
        self.scroll_y = self.max_scroll();
    }

    pub fn scroll_to_top(&mut self) {
        self.follow = false;
        self.scroll_y = 0;
    }

    // -- toggles ---------------------------------------------------------

    pub fn toggle_thinking(&mut self) {
        self.show_thinking = !self.show_thinking;
        self.apply_follow();
    }

    pub fn toggle_json(&mut self) {
        self.show_json = !self.show_json;
        self.apply_follow();
    }

    pub fn toggle_system(&mut self) {
        self.show_system = !self.show_system;
        self.apply_follow();
    }

    // -- selection -------------------------------------------------------

    fn to_cell(&self, viewport_x: usize, viewport_y: usize) -> Cell {
        let line = (self.scroll_y + viewport_y).min(self.visible_count().saturating_sub(1));
        (line, viewport_x)
    }

    pub fn start_selection(&mut self, viewport_x: usize, viewport_y: usize) {
        let cell = self.to_cell(viewport_x, viewport_y);
        self.selection = Some((cell, cell));
    }

    pub fn update_selection(&mut self, viewport_x: usize, viewport_y: usize) {
        if let Some((anchor, _)) = self.selection {
            self.selection = Some((anchor, self.to_cell(viewport_x, viewport_y)));
        }
    }

    pub fn clear_selection(&mut self) {
        self.selection = None;
    }

    pub fn has_selection(&self) -> bool {
        self.selection.map(|(a, b)| a != b).unwrap_or(false)
    }

    /// Is column `col` of visible line `line` inside the current selection?
    pub fn is_selected(&self, line: usize, col: usize) -> bool {
        let Some((a, b)) = self.selection else {
            return false;
        };
        let (start, end) = order(a, b);
        if line < start.0 || line > end.0 {
            return false;
        }
        let lo = if line == start.0 { start.1 } else { 0 };
        let hi = if line == end.0 { end.1 } else { usize::MAX };
        col >= lo && col < hi
    }

    /// Extract the selected text (for copy).
    pub fn selected_text(&self) -> Option<String> {
        let (a, b) = self.selection?;
        if a == b {
            return None;
        }
        let (start, end) = order(a, b);
        let lines = self.visible_lines();
        if start.0 == end.0 {
            return lines.get(start.0).map(|l| slice_cols(l, start.1, end.1));
        }
        let mut out = Vec::new();
        out.push(slice_cols(lines.get(start.0)?, start.1, usize::MAX));
        for i in start.0 + 1..end.0 {
            if let Some(l) = lines.get(i) {
                out.push(l.clone());
            }
        }
        out.push(slice_cols(lines.get(end.0)?, 0, end.1));
        Some(out.join("\n"))
    }

    /// Entire visible buffer as text (for copy-all).
    pub fn all_text(&self) -> String {
        self.visible_lines().join("\n")
    }
}
