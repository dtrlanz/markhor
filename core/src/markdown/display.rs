use pulldown_cmark::{Event, Options, Parser, html};

/// Converts Markdown (or plain text) to HTML meant for *display*, e.g. in chat
/// UIs.
///
/// This differs from [`Markdown::to_html`](super::Markdown::to_html) in a few
/// ways, all aimed at untrusted, conversational text:
///
/// - Raw HTML in the input is escaped and shown literally rather than passed
///   through, so text such as `/agents <n>` is not mistaken for a tag.
/// - Single newlines become line breaks (as in most chat apps) instead of
///   being collapsed to spaces.
/// - Task list markers are emitted as plain text (`[ ]` / `[x]`).
/// - Front matter is not interpreted.
///
/// Tables, strikethrough and task lists are enabled.
pub fn to_display_html(text: &str) -> String {
    let options =
        Options::ENABLE_TABLES | Options::ENABLE_STRIKETHROUGH | Options::ENABLE_TASKLISTS;
    let events = Parser::new_ext(text, options).map(|event| match event {
        Event::Html(html) | Event::InlineHtml(html) => Event::Text(html),
        Event::SoftBreak => Event::HardBreak,
        Event::TaskListMarker(done) => Event::Text(if done { "[x] " } else { "[ ] " }.into()),
        other => other,
    });
    let mut out = String::with_capacity(text.len() * 3 / 2);
    html::push_html(&mut out, events);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paragraphs_and_inline_styles() {
        assert_eq!(
            to_display_html("Hello **world** and *you*"),
            "<p>Hello <strong>world</strong> and <em>you</em></p>\n"
        );
    }

    #[test]
    fn raw_html_is_escaped() {
        let html = to_display_html("Usage: /agents <n>\n\n<script>alert(1)</script>");
        assert!(html.contains("&lt;n&gt;"), "{html}");
        assert!(!html.contains("<script>"), "{html}");
        assert!(html.contains("&lt;script&gt;"), "{html}");
    }

    #[test]
    fn newlines_become_line_breaks() {
        assert_eq!(to_display_html("one\ntwo"), "<p>one<br />\ntwo</p>\n");
    }

    #[test]
    fn task_lists_use_plain_markers() {
        let html = to_display_html("- [x] done\n- [ ] todo");
        assert!(html.contains("[x] done"), "{html}");
        assert!(html.contains("[ ] todo"), "{html}");
        assert!(!html.contains("<input"), "{html}");
    }

    #[test]
    fn tables_and_strikethrough() {
        let html = to_display_html("| a | b |\n|---|---|\n| 1 | 2 |\n\n~~gone~~");
        assert!(html.contains("<table>"), "{html}");
        assert!(html.contains("<del>gone</del>"), "{html}");
    }

    #[test]
    fn front_matter_is_plain_content() {
        let html = to_display_html("---\ntitle: x\n---\n");
        assert!(!html.contains("metadata"), "{html}");
    }
}
