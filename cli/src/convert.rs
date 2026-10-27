//! The single place where message text is converted to HTML for display.
//!
//! [`ConsoleChatUi`](crate::ConsoleChatUi) only ever sees HTML: every message
//! part goes through a [`TextToHtml`] implementation. The default one,
//! [`MarkdownToHtml`], delegates to `markhor_core::markdown`, so no Markdown
//! parsing happens in this crate. To change how text is interpreted (plain
//! text only, another Markdown flavour, richer formats, ...) provide a
//! different implementation via
//! [`ConsoleChatUi::with_converter`](crate::ConsoleChatUi::with_converter).

use markhor_core::chat::common::ContentPart;

/// Converts the text of a chat message to HTML.
///
/// Implementations must treat input as untrusted: markup in the text should
/// be neutralized unless it is deliberately supported.
pub trait TextToHtml: Send + Sync {
    fn to_html(&self, text: &str) -> String;
}

impl<F> TextToHtml for F
where
    F: Fn(&str) -> String + Send + Sync,
{
    fn to_html(&self, text: &str) -> String {
        self(text)
    }
}

/// Interprets message text as Markdown.
#[derive(Debug, Clone, Copy, Default)]
pub struct MarkdownToHtml;

impl TextToHtml for MarkdownToHtml {
    fn to_html(&self, text: &str) -> String {
        markhor_core::markdown::to_display_html(text)
    }
}

/// Shows message text literally, without interpreting any markup.
#[derive(Debug, Clone, Copy, Default)]
pub struct PlainToHtml;

impl TextToHtml for PlainToHtml {
    fn to_html(&self, text: &str) -> String {
        let mut out = String::with_capacity(text.len() + 16);
        for paragraph in text.split("\n\n").filter(|p| !p.trim().is_empty()) {
            out.push_str("<p>");
            let mut lines = paragraph.trim_matches('\n').split('\n');
            if let Some(first) = lines.next() {
                escape_html_into(first, &mut out);
            }
            for line in lines {
                out.push_str("<br />");
                escape_html_into(line, &mut out);
            }
            out.push_str("</p>\n");
        }
        out
    }
}

/// Converts message content (text and images) to a single HTML document.
pub(crate) fn parts_to_html(converter: &dyn TextToHtml, parts: &[ContentPart]) -> String {
    let mut html = String::new();
    for part in parts {
        match part {
            ContentPart::Text(text) => html.push_str(&converter.to_html(text)),
            ContentPart::Image { mime_type, data } => {
                html.push_str("<p><img src=\"image\" alt=\"");
                escape_html_into(
                    &format!("{mime_type}, {}", format_size(data.len())),
                    &mut html,
                );
                html.push_str("\"></p>\n");
            }
        }
    }
    html
}

fn escape_html_into(text: &str, out: &mut String) {
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            c => out.push(c),
        }
    }
}

fn format_size(bytes: usize) -> String {
    const KIB: usize = 1024;
    if bytes < KIB {
        format!("{bytes} B")
    } else if bytes < KIB * KIB {
        format!("{:.1} KB", bytes as f64 / KIB as f64)
    } else {
        format!("{:.1} MB", bytes as f64 / (KIB * KIB) as f64)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn markdown_converter_escapes_raw_html() {
        let html = MarkdownToHtml.to_html("use <b>x</b>");
        assert!(!html.contains("<b>"), "{html}");
    }

    #[test]
    fn plain_converter_escapes_and_keeps_line_structure() {
        let html = PlainToHtml.to_html("a < b\nsecond line\n\nnew & para");
        assert_eq!(
            html,
            "<p>a &lt; b<br />second line</p>\n<p>new &amp; para</p>\n"
        );
        assert_eq!(PlainToHtml.to_html(""), "");
    }

    #[test]
    fn closures_can_be_converters() {
        let upper = |t: &str| format!("<p>{}</p>", t.to_uppercase());
        assert_eq!(upper.to_html("hi"), "<p>HI</p>");
    }

    #[test]
    fn parts_mix_text_and_images() {
        let parts = vec![
            ContentPart::Text("hello".into()),
            ContentPart::Image {
                mime_type: "image/png".into(),
                data: vec![0; 2048],
            },
        ];
        let html = parts_to_html(&PlainToHtml, &parts);
        assert!(html.starts_with("<p>hello</p>"));
        assert!(html.contains("alt=\"image/png, 2.0 KB\""), "{html}");
    }

    #[test]
    fn sizes_are_human_readable() {
        assert_eq!(format_size(10), "10 B");
        assert_eq!(format_size(1536), "1.5 KB");
        assert_eq!(format_size(3 * 1024 * 1024), "3.0 MB");
    }
}
