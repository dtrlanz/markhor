//! The console implementation of [`ChatUi`].

use std::{
    collections::HashMap,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

use async_trait::async_trait;
use console::Style;
use dialoguer::{
    FuzzySelect, Input, Select,
    theme::{ColorfulTheme, SimpleTheme, Theme as DialoguerTheme},
};
use indicatif::ProgressBar;
use markhor_core::chat::{
    ChatUi, MemberId, Msg, MsgEvent, MsgId, ReasoningEvent, ToolCallEvent, WorkerEvent,
    common::{ContentPart, ToolCallRequest, ToolResult},
};
use serde_json::Value;

use crate::{
    convert::{MarkdownToHtml, TextToHtml, parts_to_html},
    html::HtmlRenderer,
    prompt::{BoundedHistory, InputTheme, SlashCompleter, read_line_plain},
    stream::{AtEnd, FlushPolicy, skip_committed, tail_preview, word_count},
    term::Terminal,
    text::{sanitize, truncate, wrap_plain},
    theme::Theme,
};

/// Left indent of message bodies and status lines.
const INDENT: usize = 2;
const MIN_WIDTH: usize = 24;

/// How much of a model's reasoning to show.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ReasoningDisplay {
    /// Only show a live "thinking" status.
    Hidden,
    /// Show the interim summaries as they arrive (or the thinking time if
    /// there were none).
    #[default]
    Summaries,
    /// Show the complete reasoning trace once it is finished.
    Full,
}

/// Options for [`ConsoleChatUi`].
#[derive(Debug, Clone)]
pub struct ChatUiConfig {
    /// Maximum text width; long lines are easier to read when wrapped
    /// earlier than the terminal edge.
    pub max_width: usize,
    pub reasoning: ReasoningDisplay,
    /// Text before the input glyph; empty by default so the prompt is just `›`.
    pub prompt_label: String,
    /// Returned by `prompt` when input ends (Ctrl-C, Ctrl-D, closed stdin).
    pub exit_command: String,
    pub show_timestamps: bool,
    /// How many trailing lines of tool output to keep on screen after a tool
    /// call completes.
    pub tool_output_lines: usize,
    /// Option lists longer than this use a type-to-filter selector.
    pub fuzzy_select_threshold: usize,
    pub history_size: usize,
}

impl Default for ChatUiConfig {
    fn default() -> Self {
        Self {
            max_width: 100,
            reasoning: ReasoningDisplay::default(),
            prompt_label: String::new(),
            exit_command: "/exit".to_string(),
            show_timestamps: true,
            tool_output_lines: 3,
            fuzzy_select_threshold: 8,
            history_size: 200,
        }
    }
}

/// A classic, stdout-first terminal chat interface.
///
/// Output is a plain scrolling transcript: completed messages are rendered
/// (Markdown, tables, code, ...) and printed; activity in progress (streaming
/// text, reasoning, tool calls, worker agents) is shown with live spinners
/// beneath the transcript. Interaction is strictly turn-based: while the user
/// is typing, background activity is queued and shown afterwards.
///
/// Colour and Unicode glyphs are chosen from the environment (`NO_COLOR`,
/// `CLICOLOR`, locale, TTY detection); redirected output is plain text.
pub struct ConsoleChatUi {
    inner: Arc<Inner>,
}

impl Clone for ConsoleChatUi {
    fn clone(&self) -> Self {
        Self {
            inner: self.inner.clone(),
        }
    }
}

struct Inner {
    term: Terminal,
    theme: Theme,
    renderer: HtmlRenderer,
    converter: Arc<dyn TextToHtml>,
    flush: Arc<dyn FlushPolicy>,
    config: ChatUiConfig,
    /// Starts as `config.reasoning`; can be changed while running.
    reasoning: Mutex<ReasoningDisplay>,
    state: Mutex<State>,
    history: Mutex<BoundedHistory>,
    hinted: AtomicBool,
}

#[derive(Default)]
struct State {
    msgs: HashMap<usize, MsgState>,
    tools: HashMap<usize, ToolState>,
    workers: HashMap<String, WorkerState>,
    /// What the user just typed, so the matching `Msg::User` is not shown twice.
    last_prompt: Option<String>,
}

struct MsgState {
    sender: MemberId,
    header_printed: bool,
    bar: Option<ProgressBar>,
    /// All text streamed so far.
    text: String,
    /// Bytes of `text` already shown permanently.
    committed: usize,
    committed_any: bool,
    reasoning: Option<ReasoningState>,
}

struct ReasoningState {
    started: Instant,
    title: Option<String>,
    summaries: usize,
    raw: String,
}

struct ToolState {
    name: String,
    args: String,
    bar: ProgressBar,
    started: Instant,
    output: Vec<String>,
}

struct WorkerState {
    name: String,
    bar: ProgressBar,
    started: Instant,
}

impl Default for ConsoleChatUi {
    fn default() -> Self {
        Self::new()
    }
}

impl ConsoleChatUi {
    pub fn new() -> Self {
        Self::build(Terminal::detect(), Theme::detect())
    }

    fn build(term: Terminal, theme: Theme) -> Self {
        let config = ChatUiConfig::default();
        Self {
            inner: Arc::new(Inner {
                term,
                renderer: HtmlRenderer::new(theme.clone()),
                theme,
                converter: Arc::new(MarkdownToHtml),
                flush: Arc::new(AtEnd),
                history: Mutex::new(BoundedHistory::new(config.history_size)),
                reasoning: Mutex::new(config.reasoning),
                config,
                state: Mutex::default(),
                hinted: AtomicBool::new(false),
            }),
        }
    }

    fn inner_mut(&mut self) -> &mut Inner {
        Arc::get_mut(&mut self.inner).expect("configure the UI before sharing it")
    }

    /// Replaces the text-to-HTML conversion (Markdown by default).
    pub fn with_converter(mut self, converter: impl TextToHtml + 'static) -> Self {
        self.inner_mut().converter = Arc::new(converter);
        self
    }

    /// Replaces the policy for showing text while it streams.
    pub fn with_flush_policy(mut self, policy: impl FlushPolicy + 'static) -> Self {
        self.inner_mut().flush = Arc::new(policy);
        self
    }

    pub fn with_config(mut self, config: ChatUiConfig) -> Self {
        let inner = self.inner_mut();
        inner.history = Mutex::new(BoundedHistory::new(config.history_size));
        inner.reasoning = Mutex::new(config.reasoning);
        inner.config = config;
        self
    }

    /// Changes how much reasoning is shown, effective for later events.
    pub fn set_reasoning(&self, mode: ReasoningDisplay) {
        *self.inner.reasoning.lock().unwrap() = mode;
    }

    pub fn reasoning(&self) -> ReasoningDisplay {
        self.inner.reasoning_mode()
    }

    pub fn with_theme(mut self, theme: Theme) -> Self {
        let inner = self.inner_mut();
        inner.renderer = HtmlRenderer::new(theme.clone());
        inner.theme = theme;
        self
    }
}

fn fnv(text: &str) -> usize {
    text.bytes().fold(0xcbf29ce484222325u64, |h, b| {
        (h ^ u64::from(b)).wrapping_mul(0x100000001b3)
    }) as usize
}

fn fmt_duration(d: Duration) -> String {
    let ms = d.as_millis();
    if ms < 1000 {
        format!("{ms}ms")
    } else if ms < 60_000 {
        format!("{:.1}s", ms as f64 / 1000.0)
    } else {
        format!("{}m {:02}s", ms / 60_000, (ms / 1000) % 60)
    }
}

fn plain_text(parts: &[ContentPart]) -> String {
    parts
        .iter()
        .filter_map(|p| match p {
            ContentPart::Text(t) => Some(t.as_str()),
            _ => None,
        })
        .collect()
}

fn compact_json(value: &Value) -> String {
    sanitize(&serde_json::to_string(value).unwrap_or_default())
}

/// Renders tool arguments as `key: value, ...` for display.
fn format_args(args: &Value) -> String {
    match args {
        Value::Object(map) => map
            .iter()
            .map(|(k, v)| {
                let v = match v {
                    Value::String(s) => format!("\"{}\"", truncate(&sanitize(s), 40, "…")),
                    other => truncate(&compact_json(other), 40, "…"),
                };
                format!("{}: {v}", sanitize(k))
            })
            .collect::<Vec<_>>()
            .join(", "),
        Value::Null => String::new(),
        other => truncate(&compact_json(other), 60, "…"),
    }
}

impl Inner {
    fn text_width(&self, indent: usize) -> usize {
        self.term
            .width()
            .min(self.config.max_width)
            .saturating_sub(indent)
            .max(MIN_WIDTH)
    }

    fn reasoning_mode(&self) -> ReasoningDisplay {
        *self.reasoning.lock().unwrap()
    }

    fn member_style(&self, member: &MemberId) -> Style {
        if member.human() {
            self.theme.user.clone()
        } else if member.id() == "platform" {
            self.theme.platform.clone()
        } else {
            self.theme.agent_style(fnv(member.id()))
        }
    }

    fn dim(&self, text: impl std::fmt::Display) -> String {
        self.theme.paint(&self.theme.dim, text)
    }

    fn indented(&self, lines: Vec<String>, indent: usize) -> Vec<String> {
        let pad = " ".repeat(indent);
        lines
            .into_iter()
            .map(|l| if l.is_empty() { l } else { format!("{pad}{l}") })
            .collect()
    }

    /// Renders content to indented, styled lines.
    fn body(&self, parts: &[ContentPart], indent: usize) -> Vec<String> {
        let html = parts_to_html(&*self.converter, parts);
        let lines = self.renderer.render(&html, self.text_width(indent));
        self.indented(lines, indent)
    }

    /// Renders what a participant said. Messages from agents get a bar in the
    /// sender's colour so they stand out from the dimmer activity around them.
    fn message_body(&self, sender: &MemberId, parts: &[ContentPart]) -> Vec<String> {
        if sender.human() {
            return self.body(parts, INDENT);
        }
        let html = parts_to_html(&*self.converter, parts);
        let mut lines = self.renderer.render(&html, self.text_width(INDENT));
        while lines.last().is_some_and(|l| l.is_empty()) {
            lines.pop();
        }
        let skip = lines.iter().take_while(|l| l.is_empty()).count();
        let bar = self
            .theme
            .paint(&self.member_style(sender), self.theme.glyphs.bar);
        lines
            .into_iter()
            .skip(skip)
            .map(|l| {
                if l.is_empty() {
                    bar.clone()
                } else {
                    format!("{bar} {l}")
                }
            })
            .collect()
    }

    /// The blank line between two chunks of one message.
    fn gap_bar(&self, sender: &MemberId) -> String {
        if sender.human() {
            String::new()
        } else {
            self.theme
                .paint(&self.member_style(sender), self.theme.glyphs.bar)
        }
    }

    fn header(&self, sender: &MemberId, to: &[MemberId]) -> Vec<String> {
        let mut line = self
            .theme
            .paint(&self.member_style(sender), sanitize(&sender.to_string()));
        if !to.is_empty() {
            let names: Vec<String> = to.iter().map(|m| sanitize(&m.to_string())).collect();
            line.push_str(&self.dim(format!(" {} {}", self.theme.glyphs.arrow, names.join(", "))));
        }
        if self.config.show_timestamps {
            line.push_str(&self.dim(format!(
                " {} {}",
                self.theme.glyphs.dot,
                chrono::Local::now().format("%H:%M")
            )));
        }
        vec![String::new(), line]
    }

    fn ensure_header(&self, st: &mut MsgState) {
        if !st.header_printed {
            st.header_printed = true;
            self.term.print(&self.header(&st.sender, &[]));
        }
    }

    /// Prints a platform notice: a one-off line of information not
    /// attributed to a participant.
    fn notice(&self, text: &str) {
        let html = self.converter.to_html(text);
        let lines = self.renderer.render(&html, self.text_width(INDENT));
        let mut out = vec![String::new()];
        for (i, line) in lines.into_iter().enumerate() {
            let prefix = if i == 0 {
                format!(
                    "{} ",
                    self.theme.paint(&self.theme.info, self.theme.glyphs.info)
                )
            } else {
                " ".repeat(INDENT)
            };
            out.push(format!("{prefix}{line}"));
        }
        self.term.print(&out);
    }

    /// A dim, possibly multi-line, status line indented below the message.
    fn status_lines(
        &self,
        glyph: &str,
        glyph_style: &Style,
        text: &str,
        style: &Style,
    ) -> Vec<String> {
        let width = self.text_width(INDENT + 2);
        wrap_plain(&sanitize(text), width)
            .into_iter()
            .enumerate()
            .map(|(i, line)| {
                let lead = if i == 0 {
                    format!(
                        "{}{} ",
                        " ".repeat(INDENT),
                        self.theme.paint(glyph_style, glyph)
                    )
                } else {
                    " ".repeat(INDENT + 2)
                };
                format!("{lead}{}", self.theme.paint(style, line))
            })
            .collect()
    }

    fn update_msg_status(&self, st: &MsgState) {
        let Some(bar) = &st.bar else { return };
        let g = &self.theme.glyphs;
        let name = sanitize(&st.sender.to_string());
        let budget = self
            .text_width(INDENT + 6)
            .saturating_sub(console::measure_text_width(&name) + 12);
        let rest = if let Some(r) = &st.reasoning {
            let detail = match (&r.title, r.raw.is_empty()) {
                (Some(title), _) => format!(" {} {}", g.dot, sanitize(title)),
                (None, false) if self.reasoning_mode() != ReasoningDisplay::Hidden => {
                    format!(
                        " {} {}",
                        g.dot,
                        tail_preview(&r.raw, budget.saturating_sub(16))
                    )
                }
                _ => String::new(),
            };
            format!("is thinking{}{detail}", g.ellipsis)
        } else if st.text.is_empty() {
            g.ellipsis.to_string()
        } else {
            let pending = &st.text[st.committed..];
            let words = word_count(&st.text);
            let head = format!(
                "is writing{} {words} word{}",
                g.ellipsis,
                if words == 1 { "" } else { "s" }
            );
            let room = budget.saturating_sub(console::measure_text_width(&head) + 3);
            let tail = tail_preview(pending, room);
            if tail.is_empty() || room < 8 {
                head
            } else {
                format!("{head} {} {tail}", g.dot)
            }
        };
        bar.set_message(format!(
            "{} {}",
            self.theme.paint(&self.member_style(&st.sender), name),
            self.dim(rest)
        ));
    }

    fn new_msg_state(&self, sender: MemberId) -> MsgState {
        let bar = (!sender.human()).then(|| {
            let name = sanitize(&sender.to_string());
            self.term.spinner(
                self.theme.glyphs.spinner,
                format!(
                    "{} {}",
                    self.theme.paint(&self.member_style(&sender), name),
                    self.dim(self.theme.glyphs.ellipsis)
                ),
            )
        });
        MsgState {
            sender,
            header_printed: false,
            bar,
            text: String::new(),
            committed: 0,
            committed_any: false,
            reasoning: None,
        }
    }

    fn msg_state<'a>(&self, state: &'a mut State, id: MsgId) -> &'a mut MsgState {
        state
            .msgs
            .entry(*id)
            .or_insert_with(|| self.new_msg_state(MemberId::new("unknown", false)))
    }

    fn on_message(&self, event: &MsgEvent) {
        let mut state = self.state.lock().unwrap();
        match event {
            MsgEvent::Begin(id, sender) => {
                let st = self.new_msg_state(sender.clone());
                if let Some(old) = state.msgs.insert(**id, st) {
                    if let Some(bar) = old.bar {
                        bar.finish_and_clear();
                    }
                }
            }
            MsgEvent::Part(id, part) => {
                let st = self.msg_state(&mut state, *id);
                if let ContentPart::Text(text) = part {
                    st.text.push_str(text);
                    self.commit_ready(st);
                }
                self.update_msg_status(st);
            }
            MsgEvent::Full(id, msg) => {
                let st = state
                    .msgs
                    .remove(&**id)
                    .unwrap_or_else(|| self.new_msg_state(MemberId::new("unknown", false)));
                self.complete(&mut state, st, msg);
            }
            MsgEvent::Log(id, text) => {
                let st = self.msg_state(&mut state, *id);
                self.ensure_header(st);
                self.term.print(&self.status_lines(
                    self.theme.glyphs.info,
                    &self.theme.info,
                    text,
                    &self.theme.dim,
                ));
            }
        }
    }

    /// Shows any streamed text the flush policy considers final.
    fn commit_ready(&self, st: &mut MsgState) {
        let pending = &st.text[st.committed..];
        let n = self.flush.ready_prefix(pending);
        if n == 0 || !pending.is_char_boundary(n) {
            return;
        }
        let chunk = pending[..n].to_string();
        let lines = self.message_body(&st.sender, &[ContentPart::Text(chunk)]);
        st.committed += n;
        if lines.is_empty() {
            return;
        }
        let had_header = st.header_printed;
        self.ensure_header(st);
        let mut out = Vec::new();
        if st.committed_any {
            out.push(self.gap_bar(&st.sender));
        } else if had_header {
            out.push(String::new());
        }
        out.extend(lines);
        st.committed_any = true;
        self.term.print(&out);
    }

    fn complete(&self, state: &mut State, mut st: MsgState, msg: &Msg) {
        if let Some(bar) = st.bar.take() {
            bar.finish_and_clear();
        }
        match msg {
            Msg::User {
                sender, content, ..
            } => {
                let text = plain_text(content);
                if state.last_prompt.as_deref() == Some(text.trim()) {
                    state.last_prompt = None;
                    return;
                }
                self.print_message(&mut st, sender, &[], content, false);
            }
            Msg::Agent {
                sender,
                content,
                tool_calls,
                ..
            } => {
                self.print_message(&mut st, sender, &[], content, !tool_calls.is_empty());
            }
            Msg::Direct {
                sender,
                recipients,
                content,
                ..
            } => {
                self.print_message(&mut st, sender, recipients, content, false);
            }
            Msg::Platform(text) | Msg::Developer(text) => self.notice(text),
            Msg::Event { source, .. } => {
                self.term.println(self.dim(format!(
                    "{}{} {}",
                    " ".repeat(INDENT),
                    self.theme.glyphs.info,
                    sanitize(source)
                )));
            }
            Msg::ToolResult { .. } | Msg::ControlSignal { .. } => {}
        }
    }

    fn print_message(
        &self,
        st: &mut MsgState,
        sender: &MemberId,
        to: &[MemberId],
        content: &[ContentPart],
        force_header: bool,
    ) {
        let remaining = skip_committed(content, &st.text[..st.committed]);
        let mut lines = self.message_body(sender, &remaining);
        let spoke = !sender.human() && (!lines.is_empty() || st.committed_any);
        if lines.is_empty() && !force_header {
            if spoke {
                self.term.gap_after();
            }
            return;
        }
        let had_header = st.header_printed;
        if !lines.is_empty() && !st.committed_any && had_header {
            lines.insert(0, String::new());
        }
        if !st.header_printed {
            st.header_printed = true;
            let mut block = self.header(sender, to);
            block.append(&mut lines);
            self.term.print(&block);
        } else {
            if st.committed_any && !lines.is_empty() {
                lines.insert(0, self.gap_bar(sender));
            }
            self.term.print(&lines);
        }
        if spoke {
            self.term.gap_after();
        }
    }

    fn on_reasoning(&self, event: &ReasoningEvent) {
        let mut state = self.state.lock().unwrap();
        let id = match event {
            ReasoningEvent::Begin(id)
            | ReasoningEvent::Part(id, _)
            | ReasoningEvent::Summary(id, ..)
            | ReasoningEvent::Full(id, _)
            | ReasoningEvent::Log(id, _) => *id,
        };
        let mode = self.reasoning_mode();
        let st = self.msg_state(&mut state, id);
        let g = &self.theme.glyphs;

        match event {
            ReasoningEvent::Begin(_) => {
                st.reasoning = Some(ReasoningState {
                    started: Instant::now(),
                    title: None,
                    summaries: 0,
                    raw: String::new(),
                });
            }
            ReasoningEvent::Part(_, text) => {
                if let Some(r) = st.reasoning.as_mut() {
                    r.raw.push_str(text);
                }
            }
            ReasoningEvent::Summary(_, title, prose) => {
                if let Some(r) = st.reasoning.as_mut() {
                    r.title = Some(title.clone());
                    r.summaries += 1;
                }
                if mode == ReasoningDisplay::Summaries {
                    self.ensure_header(st);
                    self.term.print(&self.status_lines(
                        g.thought,
                        &self.theme.dim,
                        &format!("{}: {}", title.trim(), prose.trim()),
                        &self.theme.reasoning,
                    ));
                }
            }
            ReasoningEvent::Full(_, trace) => {
                let r = st.reasoning.take();
                let took = r.as_ref().map(|r| r.started.elapsed());
                let summaries = r.as_ref().map_or(0, |r| r.summaries);
                match mode {
                    ReasoningDisplay::Hidden => {}
                    ReasoningDisplay::Summaries if summaries > 0 => {}
                    ReasoningDisplay::Summaries => {
                        self.ensure_header(st);
                        let took = took
                            .map(|d| format!(" for {}", fmt_duration(d)))
                            .unwrap_or_default();
                        self.term.print(&self.status_lines(
                            g.thought,
                            &self.theme.dim,
                            &format!("Thought{took}"),
                            &self.theme.reasoning,
                        ));
                    }
                    ReasoningDisplay::Full => {
                        self.ensure_header(st);
                        let took = took
                            .map(|d| format!(" ({})", fmt_duration(d)))
                            .unwrap_or_default();
                        let mut out = self.status_lines(
                            g.thought,
                            &self.theme.dim,
                            &format!("Thinking{took}"),
                            &self.theme.reasoning,
                        );
                        let width = self.text_width(INDENT + 2);
                        for line in wrap_plain(&sanitize(trace.trim()), width) {
                            out.push(format!(
                                "{}{}",
                                " ".repeat(INDENT + 2),
                                self.theme.paint(&self.theme.reasoning, line)
                            ));
                        }
                        self.term.print(&out);
                    }
                }
            }
            ReasoningEvent::Log(_, text) => {
                if mode != ReasoningDisplay::Hidden {
                    self.ensure_header(st);
                    self.term.print(&self.status_lines(
                        g.info,
                        &self.theme.info,
                        text,
                        &self.theme.dim,
                    ));
                }
            }
        }
        self.update_msg_status(st);
    }

    fn tool_message(&self, name: &str, args: &str, latest: Option<&str>) -> String {
        let budget = self.text_width(INDENT + 6);
        let label = truncate(&format!("{name}({args})"), budget / 2 + 10, "…");
        let (head, tail) = match label.split_once('(') {
            Some((n, rest)) => (n.to_string(), format!("({rest}")),
            None => (label, String::new()),
        };
        let mut out = format!(
            "{} {}{}",
            self.theme.paint(&self.theme.tool, self.theme.glyphs.tool),
            self.theme.paint(&self.theme.tool, head),
            self.dim(tail)
        );
        if let Some(latest) = latest {
            let room = budget.saturating_sub(console::measure_text_width(&out) / 2 + 4);
            out.push_str(&self.dim(format!(
                " {} {}",
                self.theme.glyphs.arrow,
                truncate(latest, room.max(10), "…")
            )));
        }
        out
    }

    fn on_tool_call(&self, event: &ToolCallEvent) {
        let mut state = self.state.lock().unwrap();
        match event {
            ToolCallEvent::Begin(msg, call, request) => {
                if let Some(st) = state.msgs.get_mut(&**msg) {
                    self.ensure_header(st);
                }
                let ToolCallRequest {
                    name, arguments, ..
                } = request;
                let name = sanitize(name);
                let args = format_args(arguments);
                let bar = self.term.spinner(
                    self.theme.glyphs.spinner,
                    self.tool_message(&name, &args, None),
                );
                state.tools.insert(
                    **call,
                    ToolState {
                        name,
                        args,
                        bar,
                        started: Instant::now(),
                        output: Vec::new(),
                    },
                );
            }
            ToolCallEvent::Part(call, text) => {
                if let Some(tool) = state.tools.get_mut(&**call) {
                    tool.output.extend(
                        sanitize(text)
                            .lines()
                            .filter(|l| !l.trim().is_empty())
                            .map(|l| l.trim_end().to_string()),
                    );
                    let latest = tool.output.last().map(String::as_str);
                    tool.bar
                        .set_message(self.tool_message(&tool.name, &tool.args, latest));
                }
            }
            ToolCallEvent::Full(call, result) => {
                let tool = state.tools.remove(&**call);
                self.finish_tool(tool, result);
            }
            ToolCallEvent::Log(call, text) => {
                let name = state
                    .tools
                    .get(&**call)
                    .map_or_else(|| "tool".to_string(), |t| t.name.clone());
                self.term.print(&self.status_lines(
                    self.theme.glyphs.info,
                    &self.theme.info,
                    &format!("{name}: {text}"),
                    &self.theme.dim,
                ));
            }
        }
    }

    fn finish_tool(&self, tool: Option<ToolState>, result: &ToolResult) {
        let g = &self.theme.glyphs;
        let (name, args, elapsed, output) = match tool {
            Some(t) => {
                t.bar.finish_and_clear();
                (t.name, t.args, Some(t.started.elapsed()), t.output)
            }
            None => (sanitize(&result.name), String::new(), None, Vec::new()),
        };
        let failed = result.content.get("error").is_some();
        let (glyph, style) = if failed {
            (g.fail, &self.theme.fail)
        } else {
            (g.ok, &self.theme.ok)
        };

        let width = self.text_width(INDENT + 2);
        let head = truncate(&format!("{name}({args})"), width / 2, "…");
        let time = elapsed
            .map(|d| format!(" ({})", fmt_duration(d)))
            .unwrap_or_default();
        let room = width.saturating_sub(console::measure_text_width(&head) + time.len() + 4);
        let summary = truncate(&compact_json(&result.content), room.max(10), "…");
        let (head_name, head_args) = match head.split_once('(') {
            Some((n, rest)) => (n.to_string(), format!("({rest}")),
            None => (head, String::new()),
        };
        let mut out = vec![format!(
            "{}{} {}{} {} {}{}",
            " ".repeat(INDENT),
            self.theme.paint(style, glyph),
            self.theme.paint(&self.theme.tool, head_name),
            self.dim(head_args),
            self.dim(g.arrow),
            self.theme.paint(
                if failed {
                    &self.theme.fail
                } else {
                    &self.theme.dim
                },
                summary
            ),
            self.dim(time),
        )];

        let keep = self.config.tool_output_lines;
        if keep > 0 && !output.is_empty() {
            let skipped = output.len().saturating_sub(keep);
            if skipped > 0 {
                out.push(self.dim(format!(
                    "{}{} {skipped} earlier line{}",
                    " ".repeat(INDENT + 2),
                    g.ellipsis,
                    if skipped == 1 { "" } else { "s" }
                )));
            }
            for line in &output[skipped..] {
                out.push(self.dim(format!(
                    "{}{}",
                    " ".repeat(INDENT + 2),
                    truncate(line, width, "…")
                )));
            }
        }
        self.term.print(&out);
    }

    fn worker_message(&self, name: &str, detail: &str) -> String {
        let g = &self.theme.glyphs;
        let budget = self
            .text_width(INDENT + 6)
            .saturating_sub(console::measure_text_width(name) + 4);
        format!(
            "{} {} {}",
            self.theme.paint(&self.theme.worker, g.worker),
            self.theme.paint(&self.theme.worker, name),
            self.dim(truncate(
                &format!("{} {}", g.dot, sanitize(detail)),
                budget.max(10),
                "…"
            ))
        )
    }

    fn on_worker(&self, event: &WorkerEvent) {
        let mut state = self.state.lock().unwrap();
        let g = &self.theme.glyphs;
        match event {
            WorkerEvent::Begin(member) => {
                let name = sanitize(&member.to_string());
                let bar = self
                    .term
                    .spinner(g.spinner, self.worker_message(&name, "starting"));
                state.workers.insert(
                    member.id().to_string(),
                    WorkerState {
                        name,
                        bar,
                        started: Instant::now(),
                    },
                );
            }
            WorkerEvent::Summary(member, title, prose) => {
                if let Some(w) = state.workers.get(member.id()) {
                    w.bar.set_message(
                        self.worker_message(
                            &w.name,
                            &format!("{}: {}", title.trim(), prose.trim()),
                        ),
                    );
                }
            }
            WorkerEvent::Full(member, text) => {
                let (name, took) = match state.workers.remove(member.id()) {
                    Some(w) => {
                        w.bar.finish_and_clear();
                        (w.name, Some(w.started.elapsed()))
                    }
                    None => (sanitize(&member.to_string()), None),
                };
                let time = took
                    .map(|d| format!(" ({})", fmt_duration(d)))
                    .unwrap_or_default();
                let mut out = vec![format!(
                    "{}{} {} {}{}",
                    " ".repeat(INDENT),
                    self.theme.paint(&self.theme.ok, g.ok),
                    self.theme.paint(&self.theme.worker, name),
                    self.dim("finished"),
                    self.dim(time),
                )];
                if !text.trim().is_empty() {
                    out.extend(self.body(&[ContentPart::Text(text.clone())], INDENT + 2));
                }
                self.term.print(&out);
            }
        }
    }

    // --- prompting -------------------------------------------------------

    fn dialoguer_theme(&self) -> Box<dyn DialoguerTheme> {
        if self.theme.colors() {
            Box::new(ColorfulTheme {
                prompt_style: Style::new().green().bold(),
                ..ColorfulTheme::default()
            })
        } else {
            Box::new(SimpleTheme)
        }
    }

    fn read_prompt(&self, commands: Vec<String>) -> String {
        let interactive = self.term.is_interactive();
        self.term.ensure_blank();
        let line = if interactive {
            if !commands.is_empty() && !self.hinted.swap(true, Ordering::Relaxed) {
                self.term.println(self.dim(format!(
                    "{}Commands start with /, Tab completes, ↑/↓ browse history, \\ continues on a new line.",
                    " ".repeat(INDENT)
                )));
            }
            let _hold = HoldGuard::new(&self.term);
            self.read_interactive(commands)
        } else {
            read_line_plain(&format!(
                "{}{}",
                self.config.prompt_label, self.theme.glyphs.prompt
            ))
        };

        match line {
            Some(line) => {
                let line = line.trim().to_string();
                self.state.lock().unwrap().last_prompt = Some(line.clone());
                line
            }
            None => self.config.exit_command.clone(),
        }
    }

    fn read_interactive(&self, commands: Vec<String>) -> Option<String> {
        let term = console::Term::stdout();
        let theme = InputTheme::new(self.theme.colors());
        let completer = SlashCompleter::new(commands);
        let mut history = self.history.lock().unwrap();
        let mut lines: Vec<String> = Vec::new();
        let mut label = format!("{}{}", self.config.prompt_label, self.theme.glyphs.prompt);
        loop {
            let line = Input::<String>::with_theme(&theme)
                .with_prompt(label.clone())
                .allow_empty(true)
                .completion_with(&completer)
                .history_with(&mut *history)
                .interact_text_on(&term)
                .ok()?;
            match line.strip_suffix('\\') {
                Some(partial) => {
                    lines.push(partial.to_string());
                    label = self.theme.glyphs.ellipsis.to_string();
                }
                None => {
                    lines.push(line);
                    return Some(lines.join("\n"));
                }
            }
        }
    }

    fn read_select(&self, options: &[String]) -> Option<usize> {
        if options.is_empty() {
            return None;
        }
        let items: Vec<String> = options.iter().map(|o| sanitize(o)).collect();
        let choice = if self.term.is_interactive() {
            let fuzzy = items.len() > self.config.fuzzy_select_threshold;
            self.term.println(self.dim(format!(
                "{}{}",
                " ".repeat(INDENT),
                if fuzzy {
                    "Type to filter, ↑/↓ to move, Enter to choose, Esc to skip."
                } else {
                    "↑/↓ to move, Enter to choose, Esc to skip."
                }
            )));
            let _hold = HoldGuard::new(&self.term);
            let term = console::Term::stdout();
            let theme = self.dialoguer_theme();
            let result = if fuzzy {
                FuzzySelect::with_theme(&*theme)
                    .items(&items)
                    .default(0)
                    .report(false)
                    .interact_on_opt(&term)
            } else {
                Select::with_theme(&*theme)
                    .items(&items)
                    .default(0)
                    .report(false)
                    .interact_on_opt(&term)
            };
            result.ok().flatten()
        } else {
            let mut lines: Vec<String> = items
                .iter()
                .enumerate()
                .map(|(i, o)| format!("  {}) {o}", i + 1))
                .collect();
            lines.push(String::new());
            self.term.print(&lines);
            read_line_plain(&format!("select{}", self.theme.glyphs.prompt))
                .and_then(|l| l.trim().parse::<usize>().ok())
                .filter(|n| (1..=items.len()).contains(n))
                .map(|n| n - 1)
        };

        if let Some(i) = choice {
            self.term.println(format!(
                "{}{} {}",
                " ".repeat(INDENT),
                self.theme.paint(&self.theme.ok, self.theme.glyphs.ok),
                items[i]
            ));
        }
        choice
    }
}

/// Holds terminal output while the user is typing; releases on drop.
struct HoldGuard<'a>(&'a Terminal);

impl<'a> HoldGuard<'a> {
    fn new(term: &'a Terminal) -> Self {
        term.hold();
        Self(term)
    }
}

impl Drop for HoldGuard<'_> {
    fn drop(&mut self) {
        self.0.release();
    }
}

impl Drop for Inner {
    fn drop(&mut self) {
        if let Ok(state) = self.state.get_mut() {
            for st in state.msgs.values() {
                if let Some(bar) = &st.bar {
                    bar.finish_and_clear();
                }
            }
            for tool in state.tools.values() {
                tool.bar.finish_and_clear();
            }
            for worker in state.workers.values() {
                worker.bar.finish_and_clear();
            }
        }
        self.term.clear();
    }
}

#[async_trait]
impl ChatUi for ConsoleChatUi {
    /// Returns [`ChatUiConfig::exit_command`] (`/exit` by default) when input
    /// ends, e.g. on Ctrl-C or end of stdin.
    async fn prompt_with_commands(&self, commands: Vec<String>) -> String {
        let inner = self.inner.clone();
        let exit = inner.config.exit_command.clone();
        tokio::task::spawn_blocking(move || inner.read_prompt(commands))
            .await
            .unwrap_or(exit)
    }

    async fn select(&self, options: &Vec<String>) -> Option<usize> {
        let inner = self.inner.clone();
        let options = options.clone();
        tokio::task::spawn_blocking(move || inner.read_select(&options))
            .await
            .ok()
            .flatten()
    }

    fn handle_message(&self, event: &MsgEvent) {
        self.inner.on_message(event);
    }

    fn handle_reasoning(&self, event: &ReasoningEvent) {
        self.inner.on_reasoning(event);
    }

    fn handle_tool_call(&self, event: &ToolCallEvent) {
        self.inner.on_tool_call(event);
    }

    fn handle_worker(&self, event: &WorkerEvent) {
        self.inner.on_worker(event);
    }
}

#[cfg(test)]
mod tests {
    use std::time::SystemTime;

    use markhor_core::{chat::ToolCallId, extension::ModelId};
    use serde_json::json;

    use super::*;
    use crate::term::Capture;

    fn ui(width: usize) -> (ConsoleChatUi, Capture) {
        let (term, lines) = Terminal::capture(width);
        let ui = ConsoleChatUi::build(term, Theme::new(false, true));
        let ui = ui.with_config(ChatUiConfig {
            show_timestamps: false,
            ..ChatUiConfig::default()
        });
        (ui, lines)
    }

    fn agent() -> MemberId {
        MemberId::with_display_name("assistant", "Assistant", false)
    }

    fn human() -> MemberId {
        MemberId::with_display_name("user", "You", true)
    }

    fn agent_msg(sender: &MemberId, text: &str) -> Msg {
        Msg::Agent {
            sender: sender.clone(),
            timestamp: Some(SystemTime::now()),
            content: vec![ContentPart::Text(text.to_string())],
            tool_calls: vec![],
            model: ModelId::new("spiff-3-nano", "zygomorphic-labs"),
        }
    }

    fn lines(capture: &Capture) -> Vec<String> {
        capture.lock().unwrap().clone()
    }

    #[test]
    fn agent_message_prints_header_and_rendered_body() {
        let (ui, out) = ui(60);
        let id = MsgId(1);
        ui.handle_message(&MsgEvent::Begin(id, agent()));
        ui.handle_message(&MsgEvent::Part(id, ContentPart::Text("Hello **wor".into())));
        ui.handle_message(&MsgEvent::Part(id, ContentPart::Text("ld**".into())));
        ui.handle_message(&MsgEvent::Full(id, agent_msg(&agent(), "Hello **world**")));
        assert_eq!(lines(&out), vec!["", "Assistant", "┃ Hello world"]);
    }

    #[test]
    fn markdown_structures_are_rendered() {
        let (ui, out) = ui(60);
        let id = MsgId(1);
        ui.handle_message(&MsgEvent::Begin(id, agent()));
        ui.handle_message(&MsgEvent::Full(
            id,
            agent_msg(&agent(), "# Title\n\n- one\n- two\n\n```\ncode\n```\n"),
        ));
        let out = lines(&out);
        assert!(out.contains(&"┃ # Title".to_string()), "{out:?}");
        assert!(out.contains(&"┃ • one".to_string()), "{out:?}");
        assert!(out.contains(&"┃ ▏ code".to_string()), "{out:?}");
        assert!(
            out.contains(&"┃".to_string()),
            "bar continues across gaps: {out:?}"
        );
    }

    #[test]
    fn user_echo_is_suppressed_once() {
        let (ui, out) = ui(60);
        ui.inner.state.lock().unwrap().last_prompt = Some("hi there".into());
        let msg = Msg::User {
            sender: human(),
            timestamp: None,
            content: vec![ContentPart::Text("hi there".into())],
        };
        ui.handle_message(&MsgEvent::Begin(MsgId(1), human()));
        ui.handle_message(&MsgEvent::Full(MsgId(1), msg.clone()));
        assert!(lines(&out).is_empty());

        // Same text again (e.g. sent by another client) is displayed.
        ui.handle_message(&MsgEvent::Begin(MsgId(2), human()));
        ui.handle_message(&MsgEvent::Full(MsgId(2), msg));
        assert_eq!(lines(&out), vec!["", "You", "  hi there"]);
    }

    #[test]
    fn platform_messages_are_notices() {
        let (ui, out) = ui(60);
        let platform = MemberId::new("platform", false);
        ui.handle_message(&MsgEvent::Begin(MsgId(1), platform));
        ui.handle_message(&MsgEvent::Full(
            MsgId(1),
            Msg::Platform("Usage: /agents <n>".into()),
        ));
        assert_eq!(lines(&out), vec!["", "ℹ Usage: /agents <n>"]);
    }

    #[test]
    fn reasoning_summaries_appear_under_the_header_before_the_body() {
        let (ui, out) = ui(60);
        let id = MsgId(1);
        ui.handle_message(&MsgEvent::Begin(id, agent()));
        ui.handle_reasoning(&ReasoningEvent::Begin(id));
        ui.handle_reasoning(&ReasoningEvent::Summary(
            id,
            "Checking".into(),
            "Looking at notes.".into(),
        ));
        ui.handle_reasoning(&ReasoningEvent::Full(id, "raw".into()));
        ui.handle_message(&MsgEvent::Full(id, agent_msg(&agent(), "Done.")));
        assert_eq!(
            lines(&out),
            vec![
                "",
                "Assistant",
                "  ✻ Checking: Looking at notes.",
                "",
                "┃ Done."
            ]
        );
    }

    #[test]
    fn reasoning_without_summaries_reports_thinking_time() {
        let (ui, out) = ui(60);
        let id = MsgId(1);
        ui.handle_message(&MsgEvent::Begin(id, agent()));
        ui.handle_reasoning(&ReasoningEvent::Begin(id));
        ui.handle_reasoning(&ReasoningEvent::Full(id, "raw".into()));
        let out = lines(&out);
        assert!(out[2].starts_with("  ✻ Thought for "), "{out:?}");
    }

    #[test]
    fn full_reasoning_mode_prints_the_trace() {
        let (term, out) = Terminal::capture(60);
        let ui = ConsoleChatUi::build(term, Theme::new(false, true)).with_config(ChatUiConfig {
            reasoning: ReasoningDisplay::Full,
            show_timestamps: false,
            ..ChatUiConfig::default()
        });
        let id = MsgId(1);
        ui.handle_message(&MsgEvent::Begin(id, agent()));
        ui.handle_reasoning(&ReasoningEvent::Begin(id));
        ui.handle_reasoning(&ReasoningEvent::Summary(id, "T".into(), "ignored".into()));
        ui.handle_reasoning(&ReasoningEvent::Full(id, "first thought".into()));
        let out = lines(&out);
        assert!(out.iter().any(|l| l.contains("Thinking")), "{out:?}");
        assert!(out.contains(&"    first thought".to_string()), "{out:?}");
        assert!(!out.iter().any(|l| l.contains("ignored")), "{out:?}");
    }

    #[test]
    fn hidden_reasoning_prints_nothing() {
        let (term, out) = Terminal::capture(60);
        let ui = ConsoleChatUi::build(term, Theme::new(false, true)).with_config(ChatUiConfig {
            reasoning: ReasoningDisplay::Hidden,
            ..ChatUiConfig::default()
        });
        let id = MsgId(1);
        ui.handle_message(&MsgEvent::Begin(id, agent()));
        ui.handle_reasoning(&ReasoningEvent::Begin(id));
        ui.handle_reasoning(&ReasoningEvent::Summary(id, "T".into(), "p".into()));
        ui.handle_reasoning(&ReasoningEvent::Full(id, "raw".into()));
        assert!(lines(&out).is_empty());
    }

    #[test]
    fn agent_replies_are_followed_by_a_blank_line() {
        let (ui, out) = ui(80);
        let id = MsgId(1);
        ui.handle_message(&MsgEvent::Begin(id, agent()));
        ui.handle_message(&MsgEvent::Full(id, agent_msg(&agent(), "Hi.")));
        let call = ToolCallId(1);
        let request = ToolCallRequest {
            id: "r1".into(),
            name: "calc".into(),
            arguments: json!({}),
        };
        ui.handle_tool_call(&ToolCallEvent::Begin(id, call, request));
        ui.handle_tool_call(&ToolCallEvent::Full(
            call,
            ToolResult {
                call_id: "r1".into(),
                name: "calc".into(),
                content: json!(1),
            },
        ));
        let out = lines(&out);
        assert_eq!(&out[..4], ["", "Assistant", "┃ Hi.", ""], "{out:?}");
        assert!(out[4].contains("calc"), "{out:?}");
    }

    #[test]
    fn tool_calls_print_a_result_line_with_recent_output() {
        let (ui, out) = ui(80);
        let call = ToolCallId(1);
        let request = ToolCallRequest {
            id: "r1".into(),
            name: "search_notes".into(),
            arguments: json!({ "query": "chunk" }),
        };
        ui.handle_tool_call(&ToolCallEvent::Begin(MsgId(9), call, request));
        ui.handle_tool_call(&ToolCallEvent::Part(call, "line one\nline two\n".into()));
        ui.handle_tool_call(&ToolCallEvent::Log(call, "retrying".into()));
        ui.handle_tool_call(&ToolCallEvent::Full(
            call,
            ToolResult {
                call_id: "r1".into(),
                name: "search_notes".into(),
                content: json!({ "matches": 3 }),
            },
        ));
        let out = lines(&out);
        assert_eq!(out[0], "  ℹ search_notes: retrying");
        assert!(
            out[1].starts_with("  ✔ search_notes(query: \"chunk\") → {\"matches\":3}"),
            "{out:?}"
        );
        assert_eq!(out[2], "    line one");
        assert_eq!(out[3], "    line two");
    }

    #[test]
    fn failed_tool_calls_use_the_failure_glyph() {
        let (ui, out) = ui(80);
        let call = ToolCallId(1);
        ui.handle_tool_call(&ToolCallEvent::Full(
            call,
            ToolResult {
                call_id: "r".into(),
                name: "calc".into(),
                content: json!({ "error": "boom" }),
            },
        ));
        assert!(lines(&out)[0].starts_with("  ✘ calc"), "{:?}", lines(&out));
    }

    #[test]
    fn long_tool_output_is_trimmed_to_the_last_lines() {
        let (ui, out) = ui(80);
        let call = ToolCallId(1);
        let request = ToolCallRequest {
            id: "r".into(),
            name: "t".into(),
            arguments: json!({}),
        };
        ui.handle_tool_call(&ToolCallEvent::Begin(MsgId(1), call, request));
        ui.handle_tool_call(&ToolCallEvent::Part(call, "1\n2\n3\n4\n5\n".into()));
        ui.handle_tool_call(&ToolCallEvent::Full(
            call,
            ToolResult {
                call_id: "r".into(),
                name: "t".into(),
                content: json!(null),
            },
        ));
        let out = lines(&out);
        assert!(out[1].contains("2 earlier lines"), "{out:?}");
        assert_eq!(&out[2..], ["    3", "    4", "    5"]);
    }

    #[test]
    fn workers_report_completion() {
        let (ui, out) = ui(80);
        let worker = MemberId::with_display_name("worker-1", "Worker 1", false);
        ui.handle_worker(&WorkerEvent::Begin(worker.clone()));
        ui.handle_worker(&WorkerEvent::Summary(
            worker.clone(),
            "Searching".into(),
            "notes".into(),
        ));
        ui.handle_worker(&WorkerEvent::Full(worker, "Found **two** notes.".into()));
        let out = lines(&out);
        assert!(out[0].starts_with("  ✔ Worker 1 finished ("), "{out:?}");
        assert_eq!(out[1], "    Found two notes.");
    }

    #[test]
    fn empty_text_worker_prints_only_the_status_line() {
        let (ui, out) = ui(80);
        let worker = MemberId::new("w", false);
        ui.handle_worker(&WorkerEvent::Begin(worker.clone()));
        ui.handle_worker(&WorkerEvent::Full(worker, String::new()));
        assert_eq!(lines(&out).len(), 1);
    }

    #[test]
    fn message_logs_print_below_the_header() {
        let (ui, out) = ui(60);
        let id = MsgId(1);
        ui.handle_message(&MsgEvent::Begin(id, agent()));
        ui.handle_message(&MsgEvent::Log(id, "generated slowly".into()));
        assert_eq!(lines(&out), vec!["", "Assistant", "  ℹ generated slowly"]);
    }

    #[test]
    fn agent_message_with_only_tool_calls_still_shows_a_header() {
        let (ui, out) = ui(60);
        let msg = Msg::Agent {
            sender: agent(),
            timestamp: None,
            content: vec![],
            tool_calls: vec![ToolCallRequest {
                id: "1".into(),
                name: "t".into(),
                arguments: json!({}),
            }],
            model: ModelId::new("spiff-3-nano", "zygomorphic-labs"),
        };
        ui.handle_message(&MsgEvent::Begin(MsgId(1), agent()));
        ui.handle_message(&MsgEvent::Full(MsgId(1), msg));
        assert_eq!(lines(&out), vec!["", "Assistant"]);
    }

    #[test]
    fn empty_agent_message_prints_nothing() {
        let (ui, out) = ui(60);
        ui.handle_message(&MsgEvent::Begin(MsgId(1), agent()));
        ui.handle_message(&MsgEvent::Full(MsgId(1), agent_msg(&agent(), "")));
        assert!(lines(&out).is_empty());
    }

    #[test]
    fn terminal_escapes_from_models_are_neutralized() {
        let (ui, out) = ui(60);
        ui.handle_message(&MsgEvent::Begin(MsgId(1), agent()));
        ui.handle_message(&MsgEvent::Full(
            MsgId(1),
            agent_msg(&agent(), "hi \x1b]0;pwned\x07 there"),
        ));
        assert!(
            lines(&out)
                .iter()
                .all(|l| !l.contains('\x1b') && !l.contains('\x07'))
        );
    }

    /// Commits each completed paragraph, to prove the policy seam works.
    struct Paragraphs;

    impl FlushPolicy for Paragraphs {
        fn ready_prefix(&self, pending: &str) -> usize {
            pending.rfind("\n\n").map_or(0, |i| i + 2)
        }
    }

    #[test]
    fn flush_policy_can_commit_text_while_streaming() {
        let (term, out) = Terminal::capture(60);
        let ui = ConsoleChatUi::build(term, Theme::new(false, true))
            .with_config(ChatUiConfig {
                show_timestamps: false,
                ..ChatUiConfig::default()
            })
            .with_flush_policy(Paragraphs);
        let id = MsgId(1);
        ui.handle_message(&MsgEvent::Begin(id, agent()));
        ui.handle_message(&MsgEvent::Part(
            id,
            ContentPart::Text("First para.\n\nSec".into()),
        ));
        assert_eq!(lines(&out), vec!["", "Assistant", "┃ First para."]);
        ui.handle_message(&MsgEvent::Part(id, ContentPart::Text("ond para.".into())));
        ui.handle_message(&MsgEvent::Full(
            id,
            agent_msg(&agent(), "First para.\n\nSecond para."),
        ));
        assert_eq!(
            lines(&out),
            vec!["", "Assistant", "┃ First para.", "┃", "┃ Second para."]
        );
    }

    #[test]
    fn formats_durations_and_arguments() {
        assert_eq!(fmt_duration(Duration::from_millis(850)), "850ms");
        assert_eq!(fmt_duration(Duration::from_millis(1250)), "1.2s");
        assert_eq!(fmt_duration(Duration::from_secs(125)), "2m 05s");
        assert_eq!(
            format_args(&json!({ "path": "a.md", "n": 3 })),
            "n: 3, path: \"a.md\""
        );
        assert_eq!(format_args(&Value::Null), "");
    }
}
