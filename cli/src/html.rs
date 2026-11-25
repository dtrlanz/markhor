//! Renders (a subset of) HTML to styled terminal lines.
//!
//! The heavy lifting (parsing, block layout, tables, wrapping) is done by
//! `html2text`. This module supplies a decorator that tags text with semantic
//! annotations, and a styling pass that turns those into ANSI styles via the
//! [`Theme`].

use html2text::render::{TaggedLine, TextDecorator};

use crate::theme::Theme;

/// Zero-width markers placed in prefixes by the decorator so that the styling
/// pass can recognize headings, quotes and bullets. `html2text` assumes that
/// prefixes are ASCII (it measures them in bytes), so the real glyphs are
/// substituted only after layout; each marker is replaced by a single column.
/// Input text is stripped of control characters first, so the markers cannot
/// occur naturally.
const HEADING_MARKS: [char; 6] = ['\x01', '\x02', '\x03', '\x04', '\x05', '\x06'];
const QUOTE_MARK: char = '\x0e';
const BULLET_MARK: char = '\x0f';

#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub(crate) enum Ann {
    #[default]
    Plain,
    Link,
    Image,
    Emphasis,
    Strong,
    Strikeout,
    Code,
    Pre,
    Dim,
}

#[derive(Clone, Debug)]
struct TermDecorator;

impl TextDecorator for TermDecorator {
    type Annotation = Ann;

    fn decorate_link_start(&mut self, _url: &str) -> (String, Ann) {
        (String::new(), Ann::Link)
    }

    fn decorate_link_end(&mut self) -> String {
        String::new()
    }

    fn decorate_em_start(&self) -> (String, Ann) {
        (String::new(), Ann::Emphasis)
    }

    fn decorate_em_end(&self) -> String {
        String::new()
    }

    fn decorate_strong_start(&self) -> (String, Ann) {
        (String::new(), Ann::Strong)
    }

    fn decorate_strong_end(&self) -> String {
        String::new()
    }

    fn decorate_strikeout_start(&self) -> (String, Ann) {
        (String::new(), Ann::Strikeout)
    }

    fn decorate_strikeout_end(&self) -> String {
        String::new()
    }

    fn decorate_code_start(&self) -> (String, Ann) {
        (String::new(), Ann::Code)
    }

    fn decorate_code_end(&self) -> String {
        String::new()
    }

    fn decorate_preformat_first(&self) -> Ann {
        Ann::Pre
    }

    fn decorate_preformat_cont(&self) -> Ann {
        Ann::Pre
    }

    fn decorate_image(&mut self, _src: &str, title: &str) -> (String, Ann) {
        (format!("[image: {title}]"), Ann::Image)
    }

    fn header_prefix(&self, level: usize) -> String {
        // One mark per column of the final "## " prefix.
        let level = level.clamp(1, 6);
        HEADING_MARKS[level - 1].to_string().repeat(level + 1)
    }

    fn quote_prefix(&self) -> String {
        format!("{QUOTE_MARK} ")
    }

    fn unordered_item_prefix(&self) -> String {
        format!("{BULLET_MARK} ")
    }

    fn ordered_item_prefix(&self, i: i64) -> String {
        format!("{i}. ")
    }

    fn make_subblock_decorator(&self) -> Self {
        self.clone()
    }

    fn finalise(&mut self, urls: Vec<String>) -> Vec<TaggedLine<Ann>> {
        if urls.is_empty() {
            return Vec::new();
        }
        let mut lines = vec![TaggedLine::from_string(String::new(), &Ann::Dim)];
        lines.extend(
            urls.into_iter()
                .enumerate()
                .map(|(i, url)| TaggedLine::from_string(format!("[{}]: {url}", i + 1), &Ann::Dim)),
        );
        lines
    }
}

/// Renders HTML to styled terminal lines.
#[derive(Debug, Clone)]
pub struct HtmlRenderer {
    theme: Theme,
}

impl HtmlRenderer {
    pub fn new(theme: Theme) -> Self {
        Self { theme }
    }

    pub fn theme(&self) -> &Theme {
        &self.theme
    }

    /// Renders `html` into lines no wider than `width` columns (tables and
    /// other unbreakable content may exceed it on very narrow terminals).
    ///
    /// The returned lines contain ANSI styling if the theme has colour
    /// enabled, and no trailing newlines. Leading and trailing blank lines are
    /// removed.
    pub fn render(&self, html: &str, width: usize) -> Vec<String> {
        let html = strip_controls(html);
        let has_code_block = html.contains("<pre");
        let gutter = self.theme.glyphs.code_gutter;
        let gutter_width = console::measure_text_width(gutter);
        let width = width.max(8);
        let render_width = if has_code_block {
            width.saturating_sub(gutter_width).max(8)
        } else {
            width
        };

        let config = html2text::config::with_decorator(TermDecorator)
            .link_footnotes(true)
            .allow_width_overflow();
        let lines = match config.lines_from_read(html.as_bytes(), render_width) {
            Ok(lines) => lines,
            // Not expected given `allow_width_overflow`, but never lose the message.
            Err(_) => return fallback_lines(&html),
        };

        let code_lines = code_line_flags(&lines);
        let mut previous_heading = None;
        let mut out: Vec<String> = Vec::with_capacity(lines.len());
        for (line, &is_code) in lines.iter().zip(&code_lines) {
            let level = heading_level(line);
            // Wrapped heading lines repeat the prefix; show the `#`s only once.
            let continuation = level.is_some() && level == previous_heading;
            previous_heading = level;
            let body = self.style_line(line, continuation);
            out.push(if is_code {
                format!("{}{body}", self.theme.paint(&self.theme.gutter, gutter))
            } else {
                body
            });
        }

        while out.last().is_some_and(|l| l.trim().is_empty()) {
            out.pop();
        }
        let leading = out.iter().take_while(|l| l.trim().is_empty()).count();
        out.drain(..leading);
        out
    }

    fn style_line(&self, line: &TaggedLine<Vec<Ann>>, continuation: bool) -> String {
        let theme = &self.theme;
        let heading = heading_level(line);

        let mut marks_seen = 0;
        let mut out = String::new();
        for ts in line.tagged_strings() {
            let style = |extra_dim: bool| {
                let mut style = console::Style::new();
                if let Some(level) = heading {
                    style = theme.heading[level].clone();
                }
                for ann in &ts.tag {
                    style = match ann {
                        Ann::Strong => style.bold(),
                        Ann::Emphasis => style.italic(),
                        Ann::Strikeout => style.strikethrough(),
                        Ann::Code | Ann::Pre => style.cyan(),
                        Ann::Link => style.blue().underlined(),
                        Ann::Image => style.magenta().italic(),
                        Ann::Dim => style.dim(),
                        Ann::Plain => style,
                    };
                }
                if extra_dim { style.dim() } else { style }
            };

            let plain = ts.tag.iter().all(|a| *a == Ann::Plain);
            let mut run = String::new();
            let mut run_dim = false;
            let mut chars = ts.s.chars();
            let flush = |run: &mut String, dim: bool, out: &mut String| {
                if !run.is_empty() {
                    out.push_str(&theme.paint(&style(dim), &*run));
                    run.clear();
                }
            };
            while let Some(c) = chars.next() {
                if HEADING_MARKS.contains(&c) {
                    flush(&mut run, run_dim, &mut out);
                    // The first `level` marks become `#`; the last is a space.
                    let level = heading.map_or(0, |l| l + 1);
                    if marks_seen < level && !continuation {
                        out.push_str(&theme.paint(&theme.dim, '#'));
                    } else {
                        out.push(' ');
                    }
                    marks_seen += 1;
                    continue;
                }
                if c == QUOTE_MARK {
                    flush(&mut run, run_dim, &mut out);
                    out.push_str(&theme.paint(&theme.quote, theme.glyphs.quote));
                    continue;
                }
                if c == BULLET_MARK {
                    flush(&mut run, run_dim, &mut out);
                    out.push_str(theme.glyphs.bullet);
                    continue;
                }
                // Table borders and rules are visual chrome: keep them quiet.
                let dim = plain && ('\u{2500}'..='\u{257f}').contains(&c);
                if dim != run_dim {
                    flush(&mut run, run_dim, &mut out);
                    run_dim = dim;
                }
                run.push(c);
            }
            flush(&mut run, run_dim, &mut out);
        }
        out
    }
}

/// The zero-based heading level of a laid-out line, if it is part of a heading.
fn heading_level(line: &TaggedLine<Vec<Ann>>) -> Option<usize> {
    line.tagged_strings()
        .flat_map(|ts| ts.s.chars())
        .find_map(|c| HEADING_MARKS.iter().position(|&m| m == c))
}

/// For each line, whether it belongs to a preformatted block. Blank lines
/// between two preformatted lines count as part of the block.
fn code_line_flags(lines: &[TaggedLine<Vec<Ann>>]) -> Vec<bool> {
    let is_pre: Vec<bool> = lines
        .iter()
        .map(|l| l.tagged_strings().any(|ts| ts.tag.contains(&Ann::Pre)))
        .collect();
    let is_blank: Vec<bool> = lines
        .iter()
        .map(|l| l.tagged_strings().all(|ts| ts.s.trim().is_empty()))
        .collect();
    (0..lines.len())
        .map(|i| {
            is_pre[i]
                || (is_blank[i]
                    && is_pre[..i].last().copied().unwrap_or(false)
                    && is_pre[i + 1..]
                        .iter()
                        .position(|&p| p)
                        .is_some_and(|off| is_blank[i + 1..i + 1 + off].iter().all(|&b| b)))
        })
        .collect()
}

/// Removes control characters (except newline and tab) so that input can
/// neither smuggle terminal escapes through nor collide with the markers
/// used by the decorator.
fn strip_controls(html: &str) -> String {
    html.chars()
        .filter(|&c| !c.is_control() || c == '\n' || c == '\t')
        .collect()
}

fn fallback_lines(html: &str) -> Vec<String> {
    html.lines().map(str::to_owned).collect()
}

#[cfg(test)]
mod tests {
    use console::strip_ansi_codes;

    use super::*;

    fn plain(html: &str, width: usize) -> Vec<String> {
        HtmlRenderer::new(Theme::new(false, true)).render(html, width)
    }

    fn colored(html: &str, width: usize) -> Vec<String> {
        HtmlRenderer::new(Theme::new(true, true)).render(html, width)
    }

    #[test]
    fn wraps_paragraphs() {
        let lines = plain("<p>the quick brown fox jumps over the lazy dog</p>", 16);
        assert_eq!(
            lines,
            vec!["the quick brown", "fox jumps over", "the lazy dog"]
        );
    }

    #[test]
    fn separates_paragraphs_with_blank_line() {
        assert_eq!(plain("<p>one</p><p>two</p>", 40), vec!["one", "", "two"]);
    }

    #[test]
    fn inline_styles_emit_escapes_only_when_enabled() {
        let html = "<p>a <strong>b</strong> <em>c</em> <code>d</code></p>";
        let styled = colored(html, 40);
        assert_eq!(styled.len(), 1);
        assert!(styled[0].contains('\x1b'));
        assert_eq!(strip_ansi_codes(&styled[0]), "a b c d");
        assert_eq!(plain(html, 40), vec!["a b c d"]);
    }

    #[test]
    fn headings_lose_markers_and_show_their_level() {
        let html = "<h1>Title</h1><h3>Sub</h3><p>text</p>";
        assert_eq!(plain(html, 40), vec!["# Title", "", "### Sub", "", "text"]);
        let styled = colored(html, 40);
        assert!(styled[0].contains('\x1b'));
        assert!(!styled[0].contains('\x01'));
    }

    #[test]
    fn wrapped_headings_stay_clean_and_hang_under_the_text() {
        let lines = plain("<h2>one two three four five</h2>", 14);
        assert!(lines.len() > 1);
        assert!(lines.iter().all(|l| !l.chars().any(|c| c.is_control())));
        assert!(lines[0].starts_with("## one"), "{lines:?}");
        assert!(lines[1].starts_with("   "), "{lines:?}");
    }

    #[test]
    fn lists_use_bullets_and_numbers() {
        let lines = plain(
            "<ul><li>a</li><li>b</li></ul><ol><li>x</li><li>y</li></ol>",
            40,
        );
        assert_eq!(lines[0], "• a");
        assert_eq!(lines[1], "• b");
        assert!(lines.contains(&"1. x".to_string()));
        assert!(lines.contains(&"2. y".to_string()));
    }

    #[test]
    fn ascii_theme_uses_ascii_glyphs() {
        let lines = HtmlRenderer::new(Theme::new(false, false)).render("<ul><li>a</li></ul>", 40);
        assert_eq!(lines, vec!["* a"]);
    }

    #[test]
    fn block_quotes_get_a_quote_gutter() {
        let lines = plain(
            "<blockquote><p>wise words that wrap around</p></blockquote>",
            14,
        );
        assert!(lines.len() > 1);
        assert!(lines.iter().all(|l| l.starts_with("▎ ")), "{lines:?}");
    }

    #[test]
    fn code_blocks_get_a_gutter() {
        let lines = plain("<pre><code>fn main() {\n    run();\n}\n</code></pre>", 40);
        assert_eq!(lines, vec!["▏ fn main() {", "▏     run();", "▏ }"]);
    }

    #[test]
    fn links_become_footnotes() {
        let lines = plain(
            "<p>see <a href=\"https://example.com\">the docs</a></p>",
            60,
        );
        assert_eq!(lines[0], "see the docs[1]");
        assert_eq!(lines.last().unwrap(), "[1]: https://example.com");
    }

    #[test]
    fn images_show_alt_text() {
        let lines = plain("<p><img src=\"x.png\" alt=\"a cat\"></p>", 40);
        assert_eq!(lines, vec!["[image: a cat]"]);
    }

    #[test]
    fn tables_render_with_borders_within_width() {
        let lines = plain(
            "<table><tr><th>name</th><th>qty</th></tr><tr><td>apple</td><td>3</td></tr></table>",
            30,
        );
        assert!(lines.len() >= 3, "{lines:?}");
        assert!(lines.iter().any(|l| l.contains("apple")));
        assert!(lines.iter().all(|l| console::measure_text_width(l) <= 30));
    }

    #[test]
    fn terminal_escapes_in_input_are_dropped() {
        let styled = plain("<p>hi\x1b[2J there\x07</p>", 40);
        assert_eq!(styled, vec!["hi[2J there"]);
    }

    #[test]
    fn empty_input_renders_nothing() {
        assert!(plain("", 40).is_empty());
        assert!(plain("<p></p>", 40).is_empty());
    }

    #[test]
    fn narrow_widths_do_not_fail() {
        let lines = plain("<p>unbreakable_word_that_is_long</p>", 3);
        assert!(!lines.is_empty());
    }
}
