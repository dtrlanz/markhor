//! Terminal output routing.
//!
//! All output of the UI goes through [`Terminal`]. It decides where lines go
//! (above live progress bars on a TTY, straight to stdout when piped, or into
//! a buffer in tests) and lets the UI temporarily *hold* output while the user
//! is typing, so that background activity cannot garble the input line.

use std::{
    io::{self, IsTerminal, Write},
    sync::{Arc, Mutex},
    time::Duration,
};

use indicatif::{MultiProgress, ProgressBar, ProgressDrawTarget, ProgressStyle};

/// Width used when output is not a terminal.
const PIPED_WIDTH: usize = 100;
const REFRESH_HZ: u8 = 20;

/// Lines captured in tests.
pub(crate) type Capture = Arc<Mutex<Vec<String>>>;

enum Mode {
    /// Interactive terminal: progress bars are drawn.
    Tty,
    /// Output is redirected: no progress bars, plain lines.
    Plain,
    /// Lines are collected in memory (tests).
    #[cfg_attr(not(test), allow(dead_code))]
    Capture { lines: Capture, width: usize },
}

pub(crate) struct Terminal {
    mode: Mode,
    multi: MultiProgress,
    /// `Some` while output is held; lines queue up here.
    held: Mutex<Option<Vec<String>>>,
    flags: Mutex<Flags>,
}

/// Tracks spacing so callers can ask for blank lines without doubling them.
struct Flags {
    /// A blank line is owed before the next non-blank output.
    gap: bool,
    last_blank: bool,
}

impl Default for Flags {
    fn default() -> Self {
        Self {
            gap: false,
            last_blank: true,
        }
    }
}

impl Terminal {
    pub fn detect() -> Self {
        let tty = io::stdout().is_terminal();
        let multi = MultiProgress::with_draw_target(if tty {
            ProgressDrawTarget::stdout_with_hz(REFRESH_HZ)
        } else {
            ProgressDrawTarget::hidden()
        });
        Self {
            mode: if tty { Mode::Tty } else { Mode::Plain },
            multi,
            held: Mutex::new(None),
            flags: Mutex::default(),
        }
    }

    #[cfg(test)]
    pub fn capture(width: usize) -> (Self, Capture) {
        let lines = Capture::default();
        let term = Self {
            mode: Mode::Capture {
                lines: lines.clone(),
                width,
            },
            multi: MultiProgress::with_draw_target(ProgressDrawTarget::hidden()),
            held: Mutex::new(None),
            flags: Mutex::default(),
        };
        (term, lines)
    }

    pub fn is_interactive(&self) -> bool {
        matches!(self.mode, Mode::Tty) && io::stdin().is_terminal()
    }

    /// Current terminal width in columns.
    pub fn width(&self) -> usize {
        match &self.mode {
            Mode::Tty => console::Term::stdout().size().1 as usize,
            Mode::Plain => PIPED_WIDTH,
            Mode::Capture { width, .. } => *width,
        }
    }

    /// Prints a block of lines atomically, above any progress bars.
    pub fn print(&self, lines: &[String]) {
        if lines.is_empty() {
            return;
        }
        let owed_gap = {
            let mut flags = self.flags.lock().unwrap();
            std::mem::take(&mut flags.gap) && !lines[0].is_empty()
        };
        let lines: Vec<String> = if owed_gap {
            std::iter::once(String::new())
                .chain(lines.iter().cloned())
                .collect()
        } else {
            lines.to_vec()
        };
        let mut held = self.held.lock().unwrap();
        if let Some(queue) = held.as_mut() {
            queue.extend(lines);
            return;
        }
        self.write(&lines);
    }

    pub fn println(&self, line: impl Into<String>) {
        self.print(&[line.into()]);
    }

    /// Requests a blank line before whatever is printed next, unless that
    /// output starts with one anyway.
    pub fn gap_after(&self) {
        self.flags.lock().unwrap().gap = true;
    }

    /// Prints a blank line now unless the output already ends with one.
    pub fn ensure_blank(&self) {
        let needed = {
            let mut flags = self.flags.lock().unwrap();
            let needed = !flags.last_blank;
            flags.gap = false;
            needed
        };
        if needed {
            self.print(&[String::new()]);
        }
    }

    fn write(&self, lines: &[String]) {
        if let Some(last) = lines.last() {
            self.flags.lock().unwrap().last_blank = last.is_empty();
        }
        match &self.mode {
            Mode::Tty => {
                let mut text = lines.join("\n");
                // indicatif drops a trailing empty line unless it is doubled.
                if lines.last().is_some_and(|l| l.is_empty()) {
                    text.push('\n');
                }
                let _ = self.multi.println(text);
            }
            Mode::Plain => {
                let mut out = io::stdout().lock();
                for line in lines {
                    if writeln!(out, "{line}").is_err() {
                        break;
                    }
                }
            }
            Mode::Capture { lines: sink, .. } => sink.lock().unwrap().extend_from_slice(lines),
        }
    }

    /// Creates a spinner below the printed output.
    ///
    /// The spinner appears after the first tick interval, so activity that
    /// finishes immediately never flickers on screen. Without a TTY the
    /// spinner is invisible.
    pub fn spinner(&self, glyphs: &'static [&'static str], message: String) -> ProgressBar {
        if matches!(self.mode, Mode::Plain | Mode::Capture { .. }) {
            return ProgressBar::hidden();
        }
        let style = ProgressStyle::with_template("  {spinner:.cyan} {msg} {elapsed:.dim}")
            .expect("valid spinner template")
            .tick_strings(glyphs);
        let bar = ProgressBar::new_spinner()
            .with_style(style)
            .with_message(message);
        let bar = self.multi.add(bar);
        bar.enable_steady_tick(Duration::from_millis(100));
        bar
    }

    /// Hides progress bars and queues all output until [`Terminal::release`].
    pub fn hold(&self) {
        let mut held = self.held.lock().unwrap();
        if held.is_none() {
            *held = Some(Vec::new());
            if matches!(self.mode, Mode::Tty) {
                let _ = self.multi.clear();
                self.multi.set_draw_target(ProgressDrawTarget::hidden());
            }
        }
    }

    /// Shows progress bars again and flushes everything queued while held.
    pub fn release(&self) {
        let mut held = self.held.lock().unwrap();
        let Some(queued) = held.take() else {
            return;
        };
        if matches!(self.mode, Mode::Tty) {
            self.multi
                .set_draw_target(ProgressDrawTarget::stdout_with_hz(REFRESH_HZ));
        }
        if !queued.is_empty() {
            self.write(&queued);
        }
    }

    pub fn clear(&self) {
        let _ = self.multi.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn held_output_is_queued_until_release() {
        let (term, lines) = Terminal::capture(80);
        term.println("a");
        term.hold();
        term.println("b");
        term.print(&["c".to_string(), "d".to_string()]);
        assert_eq!(*lines.lock().unwrap(), vec!["a"]);
        term.release();
        assert_eq!(*lines.lock().unwrap(), vec!["a", "b", "c", "d"]);
        term.println("e");
        assert_eq!(lines.lock().unwrap().len(), 5);
    }

    #[test]
    fn gap_adds_one_blank_line_before_the_next_output() {
        let (term, lines) = Terminal::capture(80);
        term.println("a");
        term.gap_after();
        term.println("b");
        term.println("c");
        assert_eq!(*lines.lock().unwrap(), vec!["a", "", "b", "c"]);
    }

    #[test]
    fn gap_is_not_doubled_before_output_that_starts_blank() {
        let (term, lines) = Terminal::capture(80);
        term.println("a");
        term.gap_after();
        term.print(&["".to_string(), "header".to_string()]);
        assert_eq!(*lines.lock().unwrap(), vec!["a", "", "header"]);
    }

    #[test]
    fn ensure_blank_only_prints_when_needed() {
        let (term, lines) = Terminal::capture(80);
        term.ensure_blank();
        term.println("a");
        term.ensure_blank();
        term.ensure_blank();
        term.gap_after();
        term.ensure_blank();
        assert_eq!(*lines.lock().unwrap(), vec!["a", ""]);
    }

    #[test]
    fn release_without_hold_is_a_noop() {
        let (term, lines) = Terminal::capture(80);
        term.release();
        assert!(lines.lock().unwrap().is_empty());
    }
}
