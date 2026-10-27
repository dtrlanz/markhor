use console::{Color, Style};

/// Glyphs used for decoration, with ASCII fallbacks for terminals that cannot
/// be trusted to render Unicode.
#[derive(Debug, Clone)]
pub struct Glyphs {
    pub bullet: &'static str,
    pub quote: &'static str,
    pub bar: &'static str,
    pub code_gutter: &'static str,
    pub rule: &'static str,
    pub ok: &'static str,
    pub fail: &'static str,
    pub info: &'static str,
    pub arrow: &'static str,
    pub dot: &'static str,
    pub tool: &'static str,
    pub worker: &'static str,
    pub thought: &'static str,
    pub ellipsis: &'static str,
    pub prompt: &'static str,
    pub spinner: &'static [&'static str],
}

impl Glyphs {
    pub const UNICODE: Glyphs = Glyphs {
        bullet: "•",
        quote: "▎",
        bar: "┃",
        code_gutter: "▏ ",
        rule: "─",
        ok: "✔",
        fail: "✘",
        info: "ℹ",
        arrow: "→",
        dot: "·",
        tool: "⚙",
        worker: "◈",
        thought: "✻",
        ellipsis: "…",
        prompt: "›",
        spinner: &["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏", "✔"],
    };

    pub const ASCII: Glyphs = Glyphs {
        bullet: "*",
        quote: "|",
        bar: "|",
        code_gutter: "| ",
        rule: "-",
        ok: "+",
        fail: "x",
        info: "i",
        arrow: "->",
        dot: "-",
        tool: "*",
        worker: "*",
        thought: "*",
        ellipsis: "...",
        prompt: ">",
        spinner: &["-", "\\", "|", "/", "+"],
    };
}

/// Visual settings: colours and glyphs.
///
/// All painting goes through [`Theme::paint`] so that colour can be switched
/// off per instance (rather than via global state), which also keeps tests
/// independent of each other.
#[derive(Debug, Clone)]
pub struct Theme {
    colors: bool,
    pub glyphs: Glyphs,
    pub heading: [Style; 6],
    pub strong: Style,
    pub emphasis: Style,
    pub strikeout: Style,
    pub code: Style,
    pub link: Style,
    pub image: Style,
    pub dim: Style,
    pub quote: Style,
    pub gutter: Style,
    pub ok: Style,
    pub fail: Style,
    pub info: Style,
    pub user: Style,
    pub platform: Style,
    pub reasoning: Style,
    pub tool: Style,
    pub worker: Style,
    agent_palette: &'static [Color],
}

impl Theme {
    pub fn new(colors: bool, unicode: bool) -> Self {
        let heading = |c: Color| Style::new().bold().fg(c);
        Self {
            colors,
            glyphs: if unicode {
                Glyphs::UNICODE
            } else {
                Glyphs::ASCII
            },
            heading: [
                heading(Color::Magenta).underlined(),
                heading(Color::Magenta),
                heading(Color::Yellow),
                heading(Color::Yellow),
                Style::new().bold(),
                Style::new().bold().dim(),
            ],
            strong: Style::new().bold(),
            emphasis: Style::new().italic(),
            strikeout: Style::new().strikethrough(),
            code: Style::new().cyan(),
            link: Style::new().blue().underlined(),
            image: Style::new().magenta().italic(),
            dim: Style::new().dim(),
            quote: Style::new().dim(),
            gutter: Style::new().dim(),
            ok: Style::new().green(),
            fail: Style::new().red(),
            info: Style::new().yellow().dim(),
            user: Style::new().green().bold(),
            platform: Style::new().yellow().bold(),
            reasoning: Style::new().dim().italic(),
            tool: Style::new().blue().dim(),
            worker: Style::new().magenta().dim(),
            agent_palette: &[
                Color::Cyan,
                Color::Magenta,
                Color::Blue,
                Color::Yellow,
                Color::Red,
            ],
        }
    }

    /// Chooses colour and glyph support from the environment: `NO_COLOR`,
    /// `CLICOLOR` and TTY detection (via `console`), and the locale.
    pub fn detect() -> Self {
        Self::new(console::colors_enabled(), detect_unicode())
    }

    pub fn colors(&self) -> bool {
        self.colors
    }

    /// Applies `style` to `text`, or returns `text` unchanged if colour is off.
    pub fn paint(&self, style: &Style, text: impl std::fmt::Display) -> String {
        if self.colors {
            style.clone().force_styling(true).apply_to(text).to_string()
        } else {
            text.to_string()
        }
    }

    /// A stable style for the given agent number (e.g. derived from a hash of
    /// the member id), so that each agent keeps its colour.
    pub fn agent_style(&self, index: usize) -> Style {
        Style::new()
            .bold()
            .fg(self.agent_palette[index % self.agent_palette.len()])
    }
}

fn detect_unicode() -> bool {
    if cfg!(windows) {
        return true;
    }
    if std::env::var("TERM").is_ok_and(|t| t == "linux" || t == "dumb") {
        return false;
    }
    ["LC_ALL", "LC_CTYPE", "LANG"]
        .iter()
        .filter_map(|k| std::env::var(k).ok())
        .find(|v| !v.is_empty())
        .is_some_and(|v| {
            let v = v.to_ascii_lowercase();
            v.contains("utf-8") || v.contains("utf8")
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paint_respects_color_flag() {
        let on = Theme::new(true, true);
        let off = Theme::new(false, true);
        assert_ne!(on.paint(&on.strong, "x"), "x");
        assert_eq!(console::strip_ansi_codes(&on.paint(&on.strong, "x")), "x");
        assert_eq!(off.paint(&off.strong, "x"), "x");
    }

    #[test]
    fn agent_styles_wrap_around() {
        let t = Theme::new(true, true);
        assert_eq!(
            t.paint(&t.agent_style(0), "a"),
            t.paint(&t.agent_style(5), "a")
        );
    }
}
