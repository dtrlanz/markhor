use console::{measure_text_width, truncate_str};

/// Removes characters that could manipulate the terminal (escape sequences,
/// other C0/C1 control characters), normalizes line endings and expands tabs.
///
/// All text that originates outside this crate (model output, tool results,
/// member names, ...) must pass through here before reaching the terminal.
pub fn sanitize(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\r' => {
                if chars.peek() != Some(&'\n') {
                    out.push('\n');
                }
            }
            '\t' => out.push_str("    "),
            '\n' => out.push('\n'),
            c if c.is_control() => {}
            c => out.push(c),
        }
    }
    out
}

/// Truncates `text` to at most `width` display columns, appending `tail` if
/// anything was cut.
pub fn truncate(text: &str, width: usize, tail: &str) -> String {
    truncate_str(text, width, tail).into_owned()
}

/// Greedy word wrap of plain text to `width` columns.
///
/// Existing newlines are preserved. Words longer than a line are split.
/// Display width is measured with Unicode rules, so wide characters are
/// handled. Always returns at least one line.
pub fn wrap_plain(text: &str, width: usize) -> Vec<String> {
    let width = width.max(1);
    let mut lines = Vec::new();
    for raw in text.split('\n') {
        let mut line = String::new();
        let mut line_width = 0;
        for word in raw.split_whitespace() {
            let word_width = measure_text_width(word);
            if line_width > 0 && line_width + 1 + word_width <= width {
                line.push(' ');
                line.push_str(word);
                line_width += 1 + word_width;
                continue;
            }
            if line_width > 0 {
                lines.push(std::mem::take(&mut line));
                line_width = 0;
            }
            if word_width <= width {
                line.push_str(word);
                line_width = word_width;
                continue;
            }
            for c in word.chars() {
                let w = measure_text_width(c.encode_utf8(&mut [0; 4]));
                if line_width + w > width && line_width > 0 {
                    lines.push(std::mem::take(&mut line));
                    line_width = 0;
                }
                line.push(c);
                line_width += w;
            }
        }
        lines.push(line);
    }
    lines
}

/// Incremental word wrapper for streamed plain text.
///
/// Feed it chunks as they arrive; it returns the text that can be printed
/// right away, with newlines inserted where a word would overflow the line.
/// A trailing partial word is held back until it is complete, so words are
/// never split across lines unless longer than the whole line.
#[derive(Debug, Default)]
pub struct StreamWrap {
    width: usize,
    column: usize,
    space_pending: bool,
    pending: String,
}

impl StreamWrap {
    pub fn new(width: usize) -> Self {
        Self {
            width: width.max(1),
            ..Self::default()
        }
    }

    pub fn push(&mut self, chunk: &str) -> String {
        self.pending.push_str(&sanitize(chunk));
        let mut out = String::new();
        while let Some(end) = self.pending.find(char::is_whitespace) {
            let word: String = self.pending.drain(..end).collect();
            let sep = self.pending.remove(0);
            self.emit_word(&word, &mut out);
            if sep == '\n' {
                out.push('\n');
                self.column = 0;
                self.space_pending = false;
            } else if self.column > 0 {
                self.space_pending = true;
            }
        }
        out
    }

    /// Flushes any held-back text. Call when the stream ends.
    pub fn finish(&mut self) -> String {
        let word = std::mem::take(&mut self.pending);
        let mut out = String::new();
        self.emit_word(&word, &mut out);
        self.column = 0;
        self.space_pending = false;
        out
    }

    // Separator spaces are emitted lazily, so that a space is never left
    // dangling at the end of a wrapped line.
    fn emit_word(&mut self, word: &str, out: &mut String) {
        if word.is_empty() {
            return;
        }
        let w = measure_text_width(word);
        if self.column > 0 {
            let needed = w + usize::from(self.space_pending);
            if self.column + needed > self.width {
                out.push('\n');
                self.column = 0;
            } else if self.space_pending {
                out.push(' ');
                self.column += 1;
            }
        }
        self.space_pending = false;
        if w <= self.width {
            out.push_str(word);
            self.column += w;
            return;
        }
        for c in word.chars() {
            let cw = measure_text_width(c.encode_utf8(&mut [0; 4]));
            if self.column + cw > self.width && self.column > 0 {
                out.push('\n');
                self.column = 0;
            }
            out.push(c);
            self.column += cw;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_strips_escapes_and_controls() {
        assert_eq!(sanitize("a\x1b[31mred\x1b[0m\x07"), "a[31mred[0m");
        assert_eq!(sanitize("a\r\nb\rc"), "a\nb\nc");
        assert_eq!(sanitize("a\tb"), "a    b");
        assert_eq!(sanitize("ok \u{85}"), "ok ");
    }

    #[test]
    fn wrap_plain_basic() {
        assert_eq!(
            wrap_plain("the quick brown fox", 9),
            vec!["the quick", "brown fox"]
        );
        assert_eq!(wrap_plain("", 10), vec![""]);
        assert_eq!(wrap_plain("a\n\nb", 10), vec!["a", "", "b"]);
    }

    #[test]
    fn wrap_plain_splits_long_words_and_handles_wide_chars() {
        assert_eq!(wrap_plain("abcdefgh", 3), vec!["abc", "def", "gh"]);
        // Each CJK character is two columns wide.
        assert_eq!(wrap_plain("日本語日本語", 4), vec!["日本", "語日", "本語"]);
    }

    #[test]
    fn stream_wrap_matches_batch_wrap() {
        let text = "the quick brown fox jumps over the lazy dog";
        let mut w = StreamWrap::new(12);
        let mut out = String::new();
        for chunk in text.split_inclusive(' ') {
            out.push_str(&w.push(chunk));
        }
        out.push_str(&w.finish());
        let expected = wrap_plain(text, 12).join("\n");
        assert_eq!(out, expected);
    }

    #[test]
    fn stream_wrap_handles_split_words_and_newlines() {
        let mut w = StreamWrap::new(20);
        let mut out = w.push("hel");
        assert_eq!(out, "");
        out.push_str(&w.push("lo wor"));
        out.push_str(&w.push("ld\nnext"));
        out.push_str(&w.finish());
        assert_eq!(out, "hello world\nnext");
    }

    #[test]
    fn stream_wrap_splits_overlong_words() {
        let mut w = StreamWrap::new(4);
        let mut out = w.push("abcdefghij ");
        out.push_str(&w.finish());
        assert_eq!(out.trim_end(), "abcd\nefgh\nij");
    }
}
