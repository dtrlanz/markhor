//! Input helpers: slash-command completion and input history for `dialoguer`.

use std::{
    collections::VecDeque,
    fmt,
    io::{self, BufRead, Write},
    sync::Mutex,
};

use console::Style;
use dialoguer::{Completion, History, theme::Theme};

/// A minimal input theme: just a coloured glyph before the cursor, and a dim
/// glyph in front of the line once it has been submitted.
///
/// The prompt text handed to dialoguer is the glyph itself.
pub(crate) struct InputTheme {
    active: Style,
    submitted: Style,
}

impl InputTheme {
    pub fn new(colors: bool) -> Self {
        if colors {
            Self {
                active: Style::new().green().bold(),
                submitted: Style::new().green().dim(),
            }
        } else {
            Self {
                active: Style::new(),
                submitted: Style::new(),
            }
        }
    }
}

impl Theme for InputTheme {
    fn format_input_prompt(
        &self,
        f: &mut dyn fmt::Write,
        prompt: &str,
        _default: Option<&str>,
    ) -> fmt::Result {
        write!(f, "{} ", self.active.apply_to(prompt))
    }

    fn format_input_prompt_selection(
        &self,
        f: &mut dyn fmt::Write,
        prompt: &str,
        sel: &str,
    ) -> fmt::Result {
        write!(f, "{} {}", self.submitted.apply_to(prompt), sel)
    }
}

/// Tab completion for slash commands.
///
/// Completes `/mo` to `/model` when unambiguous. With several candidates it
/// first extends to their longest common prefix, then cycles through them on
/// repeated Tab presses.
pub(crate) struct SlashCompleter {
    commands: Vec<String>,
    last: Mutex<Option<Cycle>>,
}

struct Cycle {
    /// The completion most recently returned.
    shown: String,
    candidates: Vec<String>,
    next: usize,
}

impl SlashCompleter {
    pub fn new(mut commands: Vec<String>) -> Self {
        commands.sort();
        commands.dedup();
        Self {
            commands,
            last: Mutex::new(None),
        }
    }

    fn candidates(&self, prefix: &str) -> Vec<String> {
        self.commands
            .iter()
            .filter(|c| c.starts_with(prefix))
            .map(|c| format!("/{c}"))
            .collect()
    }
}

impl Completion for SlashCompleter {
    fn get(&self, input: &str) -> Option<String> {
        let prefix = input.strip_prefix('/')?;
        if prefix.contains(char::is_whitespace) {
            return None;
        }
        let mut last = self.last.lock().unwrap();

        // Pressing Tab again on our own suggestion cycles to the next one.
        if let Some(cycle) = last.as_mut().filter(|c| c.shown == input) {
            let choice = cycle.candidates[cycle.next % cycle.candidates.len()].clone();
            cycle.next += 1;
            cycle.shown = choice.clone();
            return Some(choice);
        }

        let candidates = self.candidates(prefix);
        let completion = match candidates.len() {
            0 => {
                *last = None;
                return None;
            }
            1 => candidates[0].clone(),
            _ => {
                let common = common_prefix(&candidates);
                if common.len() > input.len() {
                    common
                } else {
                    candidates[0].clone()
                }
            }
        };
        let next = candidates
            .iter()
            .position(|c| *c == completion)
            .map_or(0, |i| i + 1);
        *last = (candidates.len() > 1).then(|| Cycle {
            shown: completion.clone(),
            candidates,
            next,
        });
        Some(completion)
    }
}

fn common_prefix(items: &[String]) -> String {
    let mut prefix = items[0].as_str();
    for item in &items[1..] {
        let len = prefix
            .chars()
            .zip(item.chars())
            .take_while(|(a, b)| a == b)
            .map(|(c, _)| c.len_utf8())
            .sum();
        prefix = &prefix[..len];
    }
    prefix.to_string()
}

/// Bounded in-memory input history, newest first.
pub(crate) struct BoundedHistory {
    entries: VecDeque<String>,
    max: usize,
}

impl BoundedHistory {
    pub fn new(max: usize) -> Self {
        Self {
            entries: VecDeque::new(),
            max,
        }
    }
}

impl History<String> for BoundedHistory {
    fn read(&self, pos: usize) -> Option<String> {
        self.entries.get(pos).cloned()
    }

    fn write(&mut self, val: &String) {
        if val.trim().is_empty() || self.entries.front() == Some(val) {
            return;
        }
        self.entries.push_front(val.clone());
        self.entries.truncate(self.max);
    }
}

/// Reads one line from stdin without any terminal features. Used when input
/// is piped. Returns `None` on EOF or error.
pub(crate) fn read_line_plain(prompt: &str) -> Option<String> {
    print!("{prompt} ");
    let _ = io::stdout().flush();
    let mut line = String::new();
    match io::stdin().lock().read_line(&mut line) {
        Ok(0) | Err(_) => None,
        Ok(_) => Some(line.trim_end_matches(['\r', '\n']).to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn completer() -> SlashCompleter {
        SlashCompleter::new(
            ["help", "history", "exit", "stream", "select"]
                .map(String::from)
                .into(),
        )
    }

    #[test]
    fn unique_prefix_completes() {
        let c = completer();
        assert_eq!(c.get("/e").as_deref(), Some("/exit"));
        assert_eq!(c.get("/str").as_deref(), Some("/stream"));
    }

    #[test]
    fn non_commands_are_not_completed() {
        let c = completer();
        assert_eq!(c.get("hello"), None);
        assert_eq!(c.get("/zzz"), None);
        assert_eq!(c.get("/help me"), None);
        assert_eq!(c.get(""), None);
    }

    #[test]
    fn ambiguous_prefix_extends_then_cycles() {
        let c = completer();
        // "/h" -> candidates help, history; common prefix is "/h" itself, so
        // the first candidate is offered, then the next one.
        assert_eq!(c.get("/h").as_deref(), Some("/help"));
        assert_eq!(c.get("/help").as_deref(), Some("/history"));
        assert_eq!(c.get("/history").as_deref(), Some("/help"));
        // "/s" -> select, stream
        assert_eq!(c.get("/s").as_deref(), Some("/select"));
        assert_eq!(c.get("/select").as_deref(), Some("/stream"));
    }

    #[test]
    fn common_prefix_is_used_when_longer() {
        let c = SlashCompleter::new(vec!["status".into(), "stats".into()]);
        assert_eq!(c.get("/s").as_deref(), Some("/stat"));
    }

    #[test]
    fn history_is_bounded_deduplicated_and_newest_first() {
        let mut h = BoundedHistory::new(2);
        h.write(&"a".to_string());
        h.write(&"a".to_string());
        h.write(&"  ".to_string());
        h.write(&"b".to_string());
        h.write(&"c".to_string());
        assert_eq!(h.read(0).as_deref(), Some("c"));
        assert_eq!(h.read(1).as_deref(), Some("b"));
        assert_eq!(h.read(2), None);
    }
}
