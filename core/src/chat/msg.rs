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
