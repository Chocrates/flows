//! Shared HTML→text helpers. Used by `search` and `lookup` (snippet cleanup) and
//! by `http` (full-page conversion), so the tag-strip + entity-decode logic lives
//! in exactly one place.

/// Strip `<...>` tags, keeping the text between them. Does not touch tag *bodies*
/// (a `<script>` body survives) — use [`html_to_text`] for full pages.
fn strip_tags(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut in_tag = false;
    for c in s.chars() {
        match c {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if !in_tag => out.push(c),
            _ => {}
        }
    }
    out
}

/// Decode the handful of HTML entities that show up in search snippets / docs.
fn decode_entities(s: &str) -> String {
    s.replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#x27;", "'")
        .replace("&#39;", "'")
        .replace("&nbsp;", " ")
}

/// Strip tags + decode entities + trim. For short snippets (search results, docs
/// descriptions) where tag *bodies* aren't a concern.
pub fn strip_html(s: &str) -> String {
    decode_entities(&strip_tags(s)).trim().to_string()
}

/// Heuristic: a body that opens with a doctype/`<html` is HTML even if the server
/// mislabels the content-type.
pub fn looks_like_html(s: &str) -> bool {
    let head = s.trim_start().get(..512).unwrap_or(s).to_ascii_lowercase();
    head.contains("<!doctype html") || head.contains("<html")
}

/// Remove every `<tag>…</tag>` block (case-insensitive) including its contents —
/// used to drop `<script>`/`<style>`, whose bodies would otherwise survive a
/// naive tag strip.
fn drop_element(mut s: String, tag: &str) -> String {
    let open = format!("<{tag}");
    let close = format!("</{tag}>");
    loop {
        let lower = s.to_ascii_lowercase();
        let Some(start) = lower.find(&open) else {
            break;
        };
        let Some(open_end) = lower[start..].find('>').map(|g| start + g + 1) else {
            break;
        };
        let end = lower[open_end..]
            .find(&close)
            .map(|e| open_end + e + close.len())
            .unwrap_or(s.len());
        s.replace_range(start..end, " ");
    }
    s
}

/// Collapse runs of whitespace: each run → one space, or one newline if the run
/// contained one.
fn collapse_whitespace(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let (mut in_ws, mut had_nl) = (false, false);
    for c in text.chars() {
        if c.is_whitespace() {
            in_ws = true;
            had_nl |= c == '\n';
        } else {
            if in_ws && !out.is_empty() {
                out.push(if had_nl { '\n' } else { ' ' });
            }
            in_ws = false;
            had_nl = false;
            out.push(c);
        }
    }
    out.trim().to_string()
}

/// Convert a full HTML document to readable text: drop script/style blocks, strip
/// the remaining tags, decode entities, and collapse whitespace. Keeps doc pages
/// from dumping markup + inline JS into the agent's context.
pub fn html_to_text(html: &str) -> String {
    let mut s = drop_element(html.to_string(), "script");
    s = drop_element(s, "style");
    collapse_whitespace(&decode_entities(&strip_tags(&s)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strip_html_decodes_entities() {
        assert_eq!(strip_html("<b>Bevy</b> &amp; ECS"), "Bevy & ECS");
    }

    #[test]
    fn html_to_text_strips_script_style_and_tags() {
        let html = "<html><head><style>.x{color:red}</style>\
            <script>var secret = 'leak';</script></head>\
            <body><h1>Bevy</h1><p>App &amp; ECS</p></body></html>";
        let text = html_to_text(html);
        assert!(!text.contains("secret"), "script body leaked: {text}");
        assert!(!text.contains("color:red"), "style body leaked: {text}");
        assert!(!text.contains('<'), "tags leaked: {text}");
        assert!(text.contains("Bevy"), "{text}");
        assert!(text.contains("App & ECS"), "entity not decoded: {text}");
    }

    #[test]
    fn html_to_text_collapses_whitespace() {
        assert_eq!(html_to_text("<p>a</p>\n\n\n   <p>b</p>"), "a\nb");
    }

    #[test]
    fn looks_like_html_detects_doctype() {
        assert!(looks_like_html("  <!DOCTYPE html><html>"));
        assert!(looks_like_html("<HTML>"));
        assert!(!looks_like_html(r#"{"json": true}"#));
    }
}
