//! Policy for showing message text while it is still being streamed.

use console::measure_text_width;

use crate::text::sanitize;

/// Decides how much of a streamed message can be shown permanently before the
/// message is complete.
///
/// The UI accumulates streamed text and asks the policy after every chunk
/// how many leading bytes of the not-yet-shown text are *final*, meaning they
/// can be converted and printed now without ever having to change. The rest
/// is held back (and summarized in a live status line) until more text
/// arrives or the message completes.
///
/// The default, [`AtEnd`], commits nothing, so a message is rendered as a
/// whole when complete. A block-wise policy (commit each finished paragraph,
/// list or code block) can be added by implementing this trait; no other part
/// of the UI needs to change.
pub trait FlushPolicy: Send + Sync {
    /// Returns the number of leading bytes of `pending` to commit. The value
    /// must lie on a `char` boundary; `0` commits nothing.
    fn ready_prefix(&self, pending: &str) -> usize;
}

/// Commits nothing until the message is complete.
#[derive(Debug, Clone, Copy, Default)]
pub struct AtEnd;

impl FlushPolicy for AtEnd {
    fn ready_prefix(&self, _pending: &str) -> usize {
        0
    }
}

/// Removes the already committed text from the start of message content.
///
/// `committed` is the text previously shown. If the final content does not
/// start with it (the model rewrote its message), everything is shown again.
pub(crate) fn skip_committed(
    parts: &[markhor_core::chat::common::ContentPart],
    committed: &str,
) -> Vec<markhor_core::chat::common::ContentPart> {
    use markhor_core::chat::common::ContentPart;

    let full: String = parts
        .iter()
        .filter_map(|p| match p {
            ContentPart::Text(t) => Some(t.as_str()),
            _ => None,
        })
        .collect();
    if committed.is_empty() || !full.starts_with(committed) {
        return parts.to_vec();
    }

    let mut to_skip = committed.len();
    let mut out = Vec::new();
    for part in parts {
        match part {
            ContentPart::Text(text) if to_skip > 0 => {
                let cut = to_skip.min(text.len());
                to_skip -= cut;
                if cut < text.len() {
                    out.push(ContentPart::Text(text[cut..].to_string()));
                }
            }
            other => out.push(other.clone()),
        }
    }
    out
}

/// A one-line preview of the end of `text`, at most `width` columns wide,
/// for the live status of a streaming message.
pub(crate) fn tail_preview(text: &str, width: usize) -> String {
    let flat: String = sanitize(text)
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    if measure_text_width(&flat) <= width {
        return flat;
    }
    let mut taken = 0;
    let mut chars: Vec<char> = Vec::new();
    for c in flat.chars().rev() {
        let w = measure_text_width(c.encode_utf8(&mut [0; 4]));
        if taken + w + 1 > width {
            break;
        }
        taken += w;
        chars.push(c);
    }
    chars.reverse();
    format!("…{}", chars.into_iter().collect::<String>())
}

/// Counts words in streamed text.
pub(crate) fn word_count(text: &str) -> usize {
    text.split_whitespace().count()
}

#[cfg(test)]
mod tests {
    use markhor_core::chat::common::ContentPart;

    use super::*;

    fn text(s: &str) -> ContentPart {
        ContentPart::Text(s.to_string())
    }

    #[test]
    fn at_end_never_commits() {
        assert_eq!(AtEnd.ready_prefix("a\n\nb\n\n"), 0);
    }

    #[test]
    fn skip_committed_trims_across_parts() {
        let parts = vec![text("Hello "), text("world"), text("!")];
        assert_eq!(
            skip_committed(&parts, "Hello wor"),
            vec![text("ld"), text("!")]
        );
        assert_eq!(
            skip_committed(&parts, "Hello "),
            vec![text("world"), text("!")]
        );
        assert_eq!(skip_committed(&parts, "Hello world!"), vec![]);
    }

    #[test]
    fn skip_committed_keeps_everything_on_mismatch_or_empty() {
        let parts = vec![text("abc")];
        assert_eq!(skip_committed(&parts, "xyz"), parts);
        assert_eq!(skip_committed(&parts, ""), parts);
    }

    #[test]
    fn skip_committed_keeps_images() {
        let image = ContentPart::Image {
            mime_type: "image/png".into(),
            data: vec![1],
        };
        let parts = vec![text("ab"), image.clone()];
        assert_eq!(skip_committed(&parts, "ab"), vec![image]);
    }

    #[test]
    fn tail_preview_flattens_and_truncates() {
        assert_eq!(tail_preview("one\ntwo  three", 40), "one two three");
        let tail = tail_preview("the quick brown fox jumps", 10);
        assert!(tail.starts_with('…'));
        assert!(tail.ends_with("jumps"));
        assert!(console::measure_text_width(&tail) <= 10);
    }

    #[test]
    fn counts_words() {
        assert_eq!(word_count("a b\n c"), 3);
        assert_eq!(word_count(""), 0);
    }
}
