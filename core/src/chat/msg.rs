use std::time::SystemTime;

use serde_json::Value;
use smallvec::SmallVec;

use crate::chat::{
    chat::{ContentPart, ToolCallRequest},
    member::MemberId,
};

#[derive(Debug, Clone, PartialEq)]
pub enum Msg {
    Platform(String),
    Developer(String),
    User {
        sender: MemberId,
        timestamp: Option<SystemTime>,
        content: Vec<ContentPart>,
    },
    Agent {
        sender: MemberId,
        timestamp: Option<SystemTime>,
        content: Vec<ContentPart>,
        tool_calls: Vec<ToolCallRequest>,
    },
    Direct {
        sender: MemberId,
        timestamp: Option<SystemTime>,
        tool_call_id: String, // ?
        recipients: SmallVec<[MemberId; 1]>,
        content: Vec<ContentPart>,
    },
    ToolResult {
        recipients: SmallVec<[MemberId; 1]>,
        timestamp: Option<SystemTime>,
        tool_call_id: String,
        content: Vec<ContentPart>,
    },
    Event {
        recipients: SmallVec<[MemberId; 1]>,
        timestamp: Option<SystemTime>,
        source: String,
        content: Value,
    },
    ControlSignal {
        recipients: SmallVec<[MemberId; 1]>, // ?
        compaction_start: usize,
        content: String,
    },
}

impl Msg {
    /// Returns true if this message was sent by the first-person subject ("I")
    /// in a given context, false otherwise.
    ///
    /// See [`Member::context`] for more information about the way context is
    /// relative to a specific group member.
    pub fn belongs_to_first_person(&self) -> bool {
        match self {
            Msg::User { sender, .. } | Msg::Agent { sender, .. } => sender.is_first_person(),
            _ => false,
        }
    }
}

pub trait IterMsg: Iterator<Item = Msg> + Sized {
    fn map_first_person<F>(self, f: F) -> MapFirstPerson<Self, F>
    where
        F: FnMut(&mut Msg),
    {
        MapFirstPerson { iter: self, f }
    }

    fn map_others<F>(self, f: F) -> MapOthers<Self, F>
    where
        F: FnMut(&mut Msg),
    {
        MapOthers { iter: self, f }
    }

    fn map_platform_to_developer(self) -> MapPlatformToDeveloper<Self>
    where
        Self: Sized,
    {
        MapPlatformToDeveloper { iter: self }
    }

    fn chunk_by_role(self) -> ChunkByRole<Self> {
        ChunkByRole {
            iter: self.peekable(),
        }
    }

    fn circumfix_content(
        self,
        prefix: impl Into<String>,
        suffix: impl Into<String>,
    ) -> CircumfixContent<Self>
    where
        Self: Sized,
    {
        CircumfixContent {
            iter: self,
            prefix: prefix.into(),
            suffix: suffix.into(),
        }
    }

    fn circumfix_content_if<F>(
        self,
        prefix: impl Into<String>,
        suffix: impl Into<String>,
        predicate: F,
    ) -> CircumfixContentIf<Self, F>
    where
        Self: Sized,
        F: FnMut(&mut Msg) -> bool,
    {
        CircumfixContentIf {
            iter: self,
            prefix: prefix.into(),
            suffix: suffix.into(),
            predicate,
        }
    }

    fn circumfix_content_with<P, S, F>(self, f: F) -> CircumfixContentWith<Self, F>
    where
        Self: Sized,
        P: Into<String>,
        S: Into<String>,
        F: FnMut(&mut Msg) -> (P, S),
    {
        CircumfixContentWith { iter: self, f }
    }

    fn prefix_content(self, prefix: impl Into<String>) -> CircumfixContent<Self>
    where
        Self: Sized,
    {
        CircumfixContent {
            iter: self,
            prefix: prefix.into(),
            suffix: String::new(),
        }
    }

    fn prefix_content_if<F>(
        self,
        prefix: impl Into<String>,
        predicate: F,
    ) -> CircumfixContentIf<Self, F>
    where
        Self: Sized,
        F: FnMut(&mut Msg) -> bool,
    {
        CircumfixContentIf {
            iter: self,
            prefix: prefix.into(),
            suffix: String::new(),
            predicate,
        }
    }

    fn prefix_content_with<P, F>(self, f: F) -> PrefixContentWith<Self, F>
    where
        Self: Sized,
        P: Into<String>,
        F: FnMut(&mut Msg) -> P,
    {
        PrefixContentWith { iter: self, f }
    }

    fn suffix_content(self, suffix: impl Into<String>) -> CircumfixContent<Self>
    where
        Self: Sized,
    {
        CircumfixContent {
            iter: self,
            prefix: String::new(),
            suffix: suffix.into(),
        }
    }

    fn suffix_content_if<F>(
        self,
        suffix: impl Into<String>,
        predicate: F,
    ) -> CircumfixContentIf<Self, F>
    where
        Self: Sized,
        F: FnMut(&mut Msg) -> bool,
    {
        CircumfixContentIf {
            iter: self,
            prefix: String::new(),
            suffix: suffix.into(),
            predicate,
        }
    }

    fn suffix_content_with<S, F>(self, f: F) -> SuffixContentWith<Self, F>
    where
        Self: Sized,
        S: Into<String>,
        F: FnMut(&mut Msg) -> S,
    {
        SuffixContentWith { iter: self, f }
    }

    /// Returns an iterator that skips all platform and developer messages
    /// that appear after messages in other roles.
    ///
    /// More specifically, platform messages appearing after any other roles
    /// (including developer) will be skipped. Developer messages appearing
    /// after any other roles (except platform) will be skipped.
    fn skip_orphaned_instructions(self) -> SkipOrphanedInstructions<Self>
    where
        Self: Sized,
    {
        SkipOrphanedInstructions {
            iter: self,
            seen_developer: false,
            seen_other: false,
        }
    }
}

impl<I: Iterator<Item = Msg>> IterMsg for I {}

pub struct MapFirstPerson<I, F> {
    iter: I,
    f: F,
}

pub struct MapOthers<I, F> {
    iter: I,
    f: F,
}

pub struct MapPlatformToDeveloper<I> {
    iter: I,
}

impl<I> Iterator for MapPlatformToDeveloper<I>
where
    I: Iterator<Item = Msg>,
{
    type Item = Msg;

    fn next(&mut self) -> Option<Self::Item> {
        self.iter.next().map(|msg| {
            if let Msg::Platform(content) = msg {
                Msg::Developer(content)
            } else {
                msg
            }
        })
    }
}

impl<I, F> Iterator for MapFirstPerson<I, F>
where
    I: Iterator<Item = Msg>,
    F: FnMut(&mut Msg),
{
    type Item = Msg;

    fn next(&mut self) -> Option<Self::Item> {
        let mut msg = self.iter.next()?;
        if msg.belongs_to_first_person() {
            (self.f)(&mut msg);
        }
        Some(msg)
    }
}

impl<I, F> Iterator for MapOthers<I, F>
where
    I: Iterator<Item = Msg>,
    F: FnMut(&mut Msg),
{
    type Item = Msg;

    fn next(&mut self) -> Option<Self::Item> {
        let mut msg = self.iter.next()?;
        if !msg.belongs_to_first_person() {
            (self.f)(&mut msg);
        }
        Some(msg)
    }
}

/// Applies prefix and suffix to the content of a message. Platform, developer,
/// and control signal content is wrapped as a string; content parts are
/// wrapped by inserting new text parts at the beginning and end.
fn circumfix_msg(msg: &mut Msg, prefix: &str, suffix: &str) {
    if prefix.is_empty() && suffix.is_empty() {
        return;
    }
    match msg {
        Msg::Platform(s) | Msg::Developer(s) => *s = format!("{prefix}{s}{suffix}"),
        Msg::ControlSignal { content, .. } => *content = format!("{prefix}{content}{suffix}"),
        Msg::User { content, .. }
        | Msg::Agent { content, .. }
        | Msg::Direct { content, .. }
        | Msg::ToolResult { content, .. } => {
            if !prefix.is_empty() {
                content.insert(0, ContentPart::Text(prefix.into()));
            }
            if !suffix.is_empty() {
                content.push(ContentPart::Text(suffix.into()));
            }
        }
        Msg::Event { .. } => unimplemented!("circumfixing event messages"),
    }
}

impl<I> Iterator for CircumfixContent<I>
where
    I: Iterator<Item = Msg>,
{
    type Item = Msg;

    fn next(&mut self) -> Option<Self::Item> {
        let mut msg = self.iter.next()?;
        circumfix_msg(&mut msg, &self.prefix, &self.suffix);
        Some(msg)
    }
}

impl<I, F> Iterator for CircumfixContentIf<I, F>
where
    I: Iterator<Item = Msg>,
    F: FnMut(&mut Msg) -> bool,
{
    type Item = Msg;

    fn next(&mut self) -> Option<Self::Item> {
        let mut msg = self.iter.next()?;
        if (self.predicate)(&mut msg) {
            circumfix_msg(&mut msg, &self.prefix, &self.suffix);
        }
        Some(msg)
    }
}

impl<I, F, P, S> Iterator for CircumfixContentWith<I, F>
where
    I: Iterator<Item = Msg>,
    F: FnMut(&mut Msg) -> (P, S),
    P: Into<String>,
    S: Into<String>,
{
    type Item = Msg;

    fn next(&mut self) -> Option<Self::Item> {
        let mut msg = self.iter.next()?;
        let (prefix, suffix) = (self.f)(&mut msg);
        let prefix: String = prefix.into();
        let suffix: String = suffix.into();
        circumfix_msg(&mut msg, &prefix, &suffix);
        Some(msg)
    }
}

impl<I, F, P> Iterator for PrefixContentWith<I, F>
where
    I: Iterator<Item = Msg>,
    F: FnMut(&mut Msg) -> P,
    P: Into<String>,
{
    type Item = Msg;

    fn next(&mut self) -> Option<Self::Item> {
        let mut msg = self.iter.next()?;
        let prefix: String = (self.f)(&mut msg).into();
        circumfix_msg(&mut msg, &prefix, "");
        Some(msg)
    }
}

impl<I, F, S> Iterator for SuffixContentWith<I, F>
where
    I: Iterator<Item = Msg>,
    F: FnMut(&mut Msg) -> S,
    S: Into<String>,
{
    type Item = Msg;

    fn next(&mut self) -> Option<Self::Item> {
        let mut msg = self.iter.next()?;
        let suffix: String = (self.f)(&mut msg).into();
        circumfix_msg(&mut msg, "", &suffix);
        Some(msg)
    }
}

pub struct CircumfixContent<I> {
    iter: I,
    prefix: String,
    suffix: String,
}

pub struct CircumfixContentIf<I, F> {
    iter: I,
    prefix: String,
    suffix: String,
    predicate: F,
}

pub struct CircumfixContentWith<I, F> {
    iter: I,
    f: F,
}

pub struct PrefixContentWith<I, F> {
    iter: I,
    f: F,
}

pub struct SuffixContentWith<I, F> {
    iter: I,
    f: F,
}

pub struct ChunkByRole<I: Iterator<Item = Msg>> {
    iter: std::iter::Peekable<I>,
}

impl<I> Iterator for ChunkByRole<I>
where
    I: Iterator<Item = Msg>,
{
    type Item = Vec<Msg>;

    fn next(&mut self) -> Option<Self::Item> {
        let first = self.iter.next()?;
        let kind = std::mem::discriminant(&first);
        let mut chunk = vec![first];
        while let Some(msg) = self.iter.next_if(|m| std::mem::discriminant(m) == kind) {
            chunk.push(msg);
        }
        Some(chunk)
    }
}

pub struct SkipOrphanedInstructions<I> {
    iter: I,
    seen_developer: bool,
    seen_other: bool,
}

impl<I> Iterator for SkipOrphanedInstructions<I>
where
    I: Iterator<Item = Msg>,
{
    type Item = Msg;

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            let msg = self.iter.next()?;
            match msg {
                Msg::Platform(_) => {
                    let orphaned = self.seen_developer || self.seen_other;
                    if !orphaned {
                        return Some(msg);
                    }
                }
                Msg::Developer(_) => {
                    let orphaned = self.seen_other;
                    self.seen_developer = true;
                    if !orphaned {
                        return Some(msg);
                    }
                }
                _ => {
                    self.seen_other = true;
                    return Some(msg);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chat::{
        Member,
        group::{Group, Thread, thread_builder::ThreadBuilder},
    };
    use std::sync::Arc;

    fn collect(builder: ThreadBuilder) -> Vec<Msg> {
        let thread: Arc<Thread> = builder.into();
        thread.iter().map(|e| e.value().clone()).collect()
    }

    /// Returns (me, ai) members of a group whose history is one user message
    /// from "me" followed by one agent message from "ai".
    fn conversation() -> (Member, Member) {
        let group = Group::new();
        let me = group.add_member(MemberId::new("me", true)).unwrap();
        let ai = group.add_member(MemberId::new("ai", false)).unwrap();
        group
            .append(Msg::User {
                sender: me.id.clone(),
                timestamp: None,
                content: vec![ContentPart::Text("hi".into())],
            })
            .unwrap()
            .append(Msg::Agent {
                sender: ai.id.clone(),
                timestamp: None,
                content: vec![ContentPart::Text("hey".into())],
                tool_calls: vec![],
            })
            .unwrap();
        (me, ai)
    }

    #[test]
    fn map_first_person_only_touches_first_person() {
        let (me, ai) = conversation();
        let output: Vec<Msg> = me
            .context()
            .map_first_person(|m| {
                if let Msg::User { content, .. } | Msg::Agent { content, .. } = m {
                    *content = vec![ContentPart::Text("changed".into())];
                }
            })
            .collect();
        assert_eq!(
            output,
            vec![
                Msg::User {
                    sender: me.id.clone(),
                    timestamp: None,
                    content: vec![ContentPart::Text("changed".into())],
                },
                Msg::Agent {
                    sender: ai.id.clone(),
                    timestamp: None,
                    content: vec![ContentPart::Text("hey".into())],
                    tool_calls: vec![],
                },
            ]
        );
    }

    #[test]
    fn map_others_only_touches_others() {
        let (me, ai) = conversation();
        let output: Vec<Msg> = me
            .context()
            .map_others(|m| {
                if let Msg::User { content, .. } | Msg::Agent { content, .. } = m {
                    *content = vec![ContentPart::Text("changed".into())];
                }
            })
            .collect();
        assert_eq!(
            output,
            vec![
                Msg::User {
                    sender: me.id.clone(),
                    timestamp: None,
                    content: vec![ContentPart::Text("hi".into())],
                },
                Msg::Agent {
                    sender: ai.id.clone(),
                    timestamp: None,
                    content: vec![ContentPart::Text("changed".into())],
                    tool_calls: vec![],
                },
            ]
        );
    }

    #[test]
    fn map_platform_to_developer() {
        let output: Vec<Msg> = collect(ThreadBuilder::new().platform("p").developer("d"))
            .into_iter()
            .map_platform_to_developer()
            .collect();
        assert_eq!(
            output,
            collect(ThreadBuilder::new().developer("p").developer("d"))
        );
    }

    #[test]
    fn prefix_content_wraps_strings_and_prepends_parts() {
        let input = ThreadBuilder::new().developer("d").user("me", "hi");
        let output: Vec<Msg> = collect(input).into_iter().prefix_content("> ").collect();
        assert_eq!(
            output,
            collect(ThreadBuilder::new().developer("> d").msg(Msg::User {
                sender: MemberId::new("me", true),
                timestamp: None,
                content: vec![
                    ContentPart::Text("> ".into()),
                    ContentPart::Text("hi".into()),
                ],
            }))
        );
    }

    #[test]
    fn suffix_content_appends_parts() {
        let input = ThreadBuilder::new().developer("d").user("me", "hi");
        let output: Vec<Msg> = collect(input).into_iter().suffix_content(" <").collect();
        assert_eq!(
            output,
            collect(ThreadBuilder::new().developer("d <").msg(Msg::User {
                sender: MemberId::new("me", true),
                timestamp: None,
                content: vec![
                    ContentPart::Text("hi".into()),
                    ContentPart::Text(" <".into()),
                ],
            }))
        );
    }

    #[test]
    fn circumfix_content_if_applies_only_when_predicate_matches() {
        let input = collect(ThreadBuilder::new().developer("a").developer("b"));
        let output: Vec<Msg> = input
            .into_iter()
            .circumfix_content_if(
                "<",
                ">",
                |m| matches!(m, Msg::Developer(s) if s.as_str() == "a"),
            )
            .collect();
        assert_eq!(
            output,
            collect(ThreadBuilder::new().developer("<a>").developer("b"))
        );
    }

    #[test]
    fn circumfix_content_with_uses_closure_output() {
        let input = collect(ThreadBuilder::new().developer("a").developer("b"));
        let output: Vec<Msg> = input
            .into_iter()
            .circumfix_content_with(|m| match m {
                Msg::Developer(s) if s.as_str() == "a" => ("[", "]"),
                _ => ("", ""),
            })
            .collect();
        assert_eq!(
            output,
            collect(ThreadBuilder::new().developer("[a]").developer("b"))
        );
    }

    #[test]
    fn prefix_and_suffix_content_with_use_closure_output() {
        let input = collect(ThreadBuilder::new().developer("a").developer("b"));
        let output: Vec<Msg> = input
            .into_iter()
            .prefix_content_with(|m| match m {
                Msg::Developer(s) if s.as_str() == "a" => "pre:",
                _ => "",
            })
            .suffix_content_with(|m| match m {
                Msg::Developer(s) if s.as_str() == "b" => ":suf",
                _ => "",
            })
            .collect();
        assert_eq!(
            output,
            collect(ThreadBuilder::new().developer("pre:a").developer("b:suf"))
        );
    }

    #[test]
    fn skip_orphaned_instructions_drops_late_instructions() {
        let input = collect(
            ThreadBuilder::new()
                .platform("p1")
                .developer("d1")
                .user("me", "u")
                .platform("p2")
                .developer("d2"),
        );
        let output: Vec<Msg> = input.into_iter().skip_orphaned_instructions().collect();
        assert_eq!(
            output,
            collect(
                ThreadBuilder::new()
                    .platform("p1")
                    .developer("d1")
                    .user("me", "u")
            )
        );
    }

    #[test]
    fn chunk_by_role_groups_consecutive_variants() {
        let input = collect(
            ThreadBuilder::new()
                .developer("d")
                .user("me", "u1")
                .user("me", "u2")
                .agent("ai", "a", vec![])
                .tool_result("ai", "c1", "r1")
                .msg(Msg::Direct {
                    sender: MemberId::new("ai", false),
                    timestamp: None,
                    tool_call_id: "c2".into(),
                    recipients: SmallVec::from_elem(MemberId::new("me", true), 1),
                    content: vec![ContentPart::Text("direct".into())],
                })
                .tool_result("ai", "c3", "r3"),
        );
        let chunks: Vec<Vec<Msg>> = input.into_iter().chunk_by_role().collect();
        assert_eq!(
            chunks,
            vec![
                collect(ThreadBuilder::new().developer("d")),
                collect(ThreadBuilder::new().user("me", "u1").user("me", "u2")),
                collect(ThreadBuilder::new().agent("ai", "a", vec![])),
                collect(ThreadBuilder::new().tool_result("ai", "c1", "r1")),
                collect(ThreadBuilder::new().msg(Msg::Direct {
                    sender: MemberId::new("ai", false),
                    timestamp: None,
                    tool_call_id: "c2".into(),
                    recipients: SmallVec::from_elem(MemberId::new("me", true), 1),
                    content: vec![ContentPart::Text("direct".into())],
                })),
                collect(ThreadBuilder::new().tool_result("ai", "c3", "r3")),
            ]
        );
    }
}
