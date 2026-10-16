//! Symbol-aware chunking for retrieval. Splits a source file into line ranges
//! aligned to its top-level syntax nodes (tree-sitter, same languages as the repo
//! map), so a chunk holds whole definitions instead of an arbitrary 50-line slice.
//!
//! - **Segments** run from the end of the previous top-level node to the end of the
//!   next, so leading doc comments / attributes stay with their definition and
//!   imports or module-level statements get chunks of their own. Every line is
//!   covered by construction.
//! - **Oversized** segments recurse into the node's body (an `impl` / `class`
//!   splits into its methods), carrying the enclosing header line as context; if
//!   that doesn't help they're split into size-bounded line windows.
//! - **Small** neighbours are merged up to [`TARGET_CHARS`].
//! - Unsupported languages and parse failures fall back to fixed line windows.
//!
//! Sizes are in characters: both the embedder's input limit (512 word pieces in
//! fastembed) and the injection budget are about text size, not line count.

use tree_sitter::{Node, Parser};

use crate::repo_map::Lang;

/// Merge adjacent small pieces until a chunk reaches about this many characters.
pub(crate) const TARGET_CHARS: usize = 1200;
/// No chunk (other than a single over-long line) exceeds this many characters.
pub(crate) const MAX_CHARS: usize = 2400;
/// Line-window size for unsupported languages / parse failures (the pre-symbol
/// chunker's granularity).
pub(crate) const FALLBACK_LINES: usize = 50;

/// A chunk as a 0-based, inclusive line range plus optional enclosing context
/// (e.g. `impl Foo for Bar {`) for pieces carved out of a larger definition.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Piece {
    pub start: usize,
    pub end: usize,
    pub header: Option<String>,
}

/// Chunk `src` into pieces. `ext` selects the grammar; `None` / unsupported →
/// line windows. Pieces with no non-blank lines are dropped.
pub(crate) fn chunk_source(ext: Option<&str>, src: &str) -> Vec<Piece> {
    let lines: Vec<&str> = src.lines().collect();
    if lines.is_empty() {
        return Vec::new();
    }
    let pieces = match ext.and_then(Lang::from_ext).and_then(|l| parse(l, src)) {
        Some(tree) => {
            let raw = segment(tree.root_node(), 0, lines.len() - 1, None, &lines);
            merge_small(raw, &lines)
        }
        None => line_windows(0, lines.len() - 1),
    };
    pieces
        .into_iter()
        .filter(|p| lines[p.start..=p.end].iter().any(|l| !l.trim().is_empty()))
        .collect()
}

fn parse(lang: Lang, src: &str) -> Option<tree_sitter::Tree> {
    let mut parser = Parser::new();
    parser.set_language(&lang.ts_language()).ok()?;
    parser.parse(src, None)
}

fn chars(lines: &[&str], start: usize, end: usize) -> usize {
    lines[start..=end].iter().map(|l| l.len() + 1).sum()
}

/// Last line a node occupies (a node ending at column 0 really ended on the line
/// before).
fn end_row(node: Node) -> usize {
    let end = node.end_position();
    if end.column == 0 && end.row > node.start_position().row {
        end.row - 1
    } else {
        end.row
    }
}

/// Comments and attributes belong to the definition that follows them, so they
/// never close a segment on their own.
fn attaches_forward(node: Node) -> bool {
    let kind = node.kind();
    kind.contains("comment") || kind == "attribute_item" || kind == "decorator"
}

/// Split `[start, end]` at the boundaries of `parent`'s named children, then
/// shrink any oversized segment (recursing into its node, else windowing).
fn segment(
    parent: Node,
    start: usize,
    end: usize,
    header: Option<&str>,
    lines: &[&str],
) -> Vec<Piece> {
    let mut segs: Vec<(usize, usize, Option<Node>)> = Vec::new();
    let mut cur = start;
    let mut cursor = parent.walk();
    for child in parent.named_children(&mut cursor) {
        if attaches_forward(child) {
            continue; // stays in `cur..`, so it joins the next definition
        }
        let child_end = end_row(child).min(end);
        if child_end < cur || child.start_position().row > end {
            continue;
        }
        segs.push((cur, child_end, Some(child)));
        cur = child_end + 1;
    }
    if cur <= end {
        match segs.last_mut() {
            Some(last) => last.1 = end, // trailing lines join the last segment
            None => segs.push((cur, end, None)),
        }
    }

    let mut out = Vec::new();
    for (s, e, node) in segs {
        if chars(lines, s, e) <= MAX_CHARS {
            out.push(Piece {
                start: s,
                end: e,
                header: header.map(str::to_string),
            });
            continue;
        }
        out.extend(shrink(s, e, node, header, lines));
    }
    out
}

/// Break an oversized segment: recurse into the node's body when that splits it
/// into more than one piece, otherwise fall back to size-bounded windows.
fn shrink(
    s: usize,
    e: usize,
    node: Option<Node>,
    header: Option<&str>,
    lines: &[&str],
) -> Vec<Piece> {
    if let Some(node) = node {
        let body = node.child_by_field_name("body").unwrap_or(node);
        if body.named_child_count() > 1 {
            let own = lines[node.start_position().row.min(e)].trim();
            let nested = match header {
                Some(h) => format!("{h}\n{own}"),
                None => own.to_string(),
            };
            let pieces = segment(body, s, e, Some(&nested), lines);
            if pieces.len() > 1 {
                return pieces;
            }
        }
    }
    sized_windows(s, e, header, lines)
}

/// Consecutive windows of at most [`MAX_CHARS`] (a single longer line stands alone).
fn sized_windows(s: usize, e: usize, header: Option<&str>, lines: &[&str]) -> Vec<Piece> {
    let mut out = Vec::new();
    let mut start = s;
    let mut size = 0;
    for (i, line) in lines.iter().enumerate().take(e + 1).skip(s) {
        let len = line.len() + 1;
        if size > 0 && size + len > MAX_CHARS {
            out.push(Piece {
                start,
                end: i - 1,
                header: header.map(str::to_string),
            });
            start = i;
            size = 0;
        }
        size += len;
    }
    out.push(Piece {
        start,
        end: e,
        header: header.map(str::to_string),
    });
    out
}

/// Fixed [`FALLBACK_LINES`]-line windows.
fn line_windows(s: usize, e: usize) -> Vec<Piece> {
    (s..=e)
        .step_by(FALLBACK_LINES)
        .map(|start| Piece {
            start,
            end: (start + FALLBACK_LINES - 1).min(e),
            header: None,
        })
        .collect()
}

/// Greedily merge adjacent pieces that share a header while the merged chunk
/// stays within [`TARGET_CHARS`].
fn merge_small(pieces: Vec<Piece>, lines: &[&str]) -> Vec<Piece> {
    let mut out: Vec<Piece> = Vec::new();
    for p in pieces {
        if let Some(last) = out.last_mut() {
            if last.header == p.header
                && last.end + 1 == p.start
                && chars(lines, last.start, p.end) <= TARGET_CHARS
            {
                last.end = p.end;
                continue;
            }
        }
        out.push(p);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(src: &str, p: &Piece) -> String {
        src.lines().collect::<Vec<_>>()[p.start..=p.end].join("\n")
    }

    /// Every non-blank line lands in exactly one piece.
    fn assert_full_coverage(src: &str, pieces: &[Piece]) {
        for (i, line) in src.lines().enumerate() {
            let hits = pieces.iter().filter(|p| p.start <= i && i <= p.end).count();
            if line.trim().is_empty() {
                assert!(hits <= 1, "line {i} in {hits} pieces");
            } else {
                assert_eq!(hits, 1, "line {i} `{line}` in {hits} pieces");
            }
        }
    }

    fn rust_fn(name: &str, body_lines: usize) -> String {
        let mut s = format!("/// Docs for {name}.\npub fn {name}(x: i32) -> i32 {{\n");
        for i in 0..body_lines {
            s.push_str(&format!(
                "    let v{i} = x + {i}; // some filler text here\n"
            ));
        }
        s.push_str("    x\n}\n\n");
        s
    }

    #[test]
    fn function_kept_whole_with_its_doc_comment() {
        let src = format!(
            "use std::fmt;\n\n{}{}",
            rust_fn("alpha", 30),
            rust_fn("beta", 30)
        );
        let pieces = chunk_source(Some("rs"), &src);
        assert_full_coverage(&src, &pieces);
        let beta = pieces
            .iter()
            .find(|p| text(&src, p).contains("pub fn beta"))
            .unwrap();
        let t = text(&src, beta);
        assert!(
            t.contains("/// Docs for beta."),
            "doc comment attached: {t}"
        );
        assert!(t.trim_end().ends_with('}'), "function not cut: {t}");
        assert!(!t.contains("pub fn alpha"), "too big to merge with alpha");
    }

    #[test]
    fn small_definitions_merge() {
        let src: String = (0..10)
            .map(|i| format!("const C{i}: u32 = {i};\n"))
            .collect();
        let pieces = chunk_source(Some("rs"), &src);
        assert_eq!(pieces.len(), 1, "{pieces:?}");
        assert_full_coverage(&src, &pieces);
    }

    #[test]
    fn oversized_impl_splits_into_methods_with_header() {
        let mut src = String::from("impl Widget for Thing {\n");
        for name in ["one", "two", "three", "four"] {
            for line in rust_fn(name, 25).lines() {
                src.push_str(&format!("    {line}\n"));
            }
        }
        src.push_str("}\n");
        assert!(src.len() > MAX_CHARS);
        let pieces = chunk_source(Some("rs"), &src);
        assert_full_coverage(&src, &pieces);
        assert!(pieces.len() > 1);
        for p in &pieces {
            assert!(chars(&src.lines().collect::<Vec<_>>(), p.start, p.end) <= MAX_CHARS);
            assert_eq!(p.header.as_deref(), Some("impl Widget for Thing {"));
        }
        let three = pieces
            .iter()
            .find(|p| text(&src, p).contains("pub fn three"))
            .unwrap();
        assert!(text(&src, three).contains("/// Docs for three."));
    }

    #[test]
    fn oversized_function_splits_into_bounded_windows() {
        let src = rust_fn("huge", 200);
        let lines: Vec<&str> = src.lines().collect();
        let pieces = chunk_source(Some("rs"), &src);
        assert!(pieces.len() > 1);
        assert_full_coverage(&src, &pieces);
        for p in &pieces {
            assert!(chars(&lines, p.start, p.end) <= MAX_CHARS);
        }
    }

    #[test]
    fn python_class_and_module_code() {
        let src = "import os\n\nclass A:\n    def f(self):\n        return 1\n\nprint(A().f())\n";
        let pieces = chunk_source(Some("py"), src);
        assert_full_coverage(src, &pieces);
        assert!(pieces.iter().any(|p| text(src, p).contains("class A:")));
    }

    #[test]
    fn unsupported_language_uses_line_windows() {
        let src: String = (0..120).map(|i| format!("line {i}\n")).collect();
        let pieces = chunk_source(Some("md"), &src);
        let spans: Vec<(usize, usize)> = pieces.iter().map(|p| (p.start, p.end)).collect();
        assert_eq!(spans, vec![(0, 49), (50, 99), (100, 119)]);
        assert!(chunk_source(None, &src).len() == 3);
    }

    #[test]
    fn blank_only_pieces_dropped_and_empty_source_is_empty() {
        assert!(chunk_source(Some("rs"), "").is_empty());
        assert!(chunk_source(Some("md"), "\n\n   \n").is_empty());
    }

    #[test]
    fn syntax_errors_still_cover_every_line() {
        let src = "fn ok() {}\n\nfn broken( {\n  let x = ;\n}\n\nstruct S;\n";
        let pieces = chunk_source(Some("rs"), src);
        assert_full_coverage(src, &pieces);
    }
}
