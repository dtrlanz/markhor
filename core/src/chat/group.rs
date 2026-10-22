use crossbeam_skiplist::{SkipMap, map::Entry};
use serde_json::Value;
use smallvec::SmallVec;
use std::{
    collections::HashMap,
    fmt::Debug,
    num::NonZero,
    sync::{
        Arc, Mutex, Weak,
        atomic::{AtomicUsize, Ordering},
    },
    time::SystemTime,
};

use crate::chat::{
    Msg,
    chat::{ContentPart, ToolCallRequest},
};

#[derive(Debug)]
pub struct Group {
    members: Mutex<HashMap<MemberId, Arc<Thread>>>,
    primary: Arc<Thread>,
    secondary: Mutex<Vec<Arc<Thread>>>,
}

impl Group {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            members: Default::default(),
            primary: Arc::new(Thread::new()),
            secondary: Default::default(),
        })
    }

    pub fn add_member(self: &Arc<Self>, id: MemberId) -> Result<Member, AddMemberError> {
        self.add_member_with_thread(id, &self.primary)
    }

    fn add_member_with_thread(
        self: &Arc<Self>,
        id: MemberId,
        thread: &Arc<Thread>,
    ) -> Result<Member, AddMemberError> {
        let mut inserted = false;
        let new_or_existing_thread = self
            .members
            .lock()
            .unwrap()
            .entry(id.clone())
            .or_insert_with(|| {
                inserted = true;
                Arc::clone(thread)
            })
            .clone();
        let ptcp = Member {
            id,
            group: Arc::clone(self),
            context: new_or_existing_thread,
        };
        if inserted {
            Ok(ptcp)
        } else {
            Err(AddMemberError::AlreadyExists(ptcp))
        }
    }

    pub fn append(&self, message: Msg) -> Result<&Self, AppendMessageError> {
        match &message {
            Msg::Platform(_) | Msg::Developer(_) => {
                self.primary.append(message);
                Ok(self)
            }
            Msg::User { sender, .. } | Msg::Agent { sender, .. } => {
                let ptcp = self
                    .members
                    .lock()
                    .unwrap()
                    .get(sender)
                    .map(|t| Arc::clone(t))
                    .ok_or(AppendMessageError::UnknownSender(sender.to_string()))?;
                ptcp.append(message.clone());
                Ok(self)
            }
            Msg::Direct { recipients, .. }
            | Msg::ToolResult { recipients, .. }
            | Msg::Event { recipients, .. } => {
                // Use HashMap for efficient deduplication
                let mut threads = HashMap::new();
                {
                    let guard = self.members.lock().unwrap();
                    for recp in recipients {
                        if let Some(context) = guard.get(recp) {
                            threads
                                .entry(Arc::as_ptr(context))
                                .or_insert_with(|| Arc::clone(context));
                        } else {
                            return Err(AppendMessageError::UnknownRecipient(recp.to_string()));
                        }
                    }
                }
                // Append message to each thread with at least one recipient represented
                for t in threads.values() {
                    t.append(message.clone());
                }
                Ok(self)
            }
            Msg::ControlSignal {
                recipients,
                compaction_start,
                content,
            } => {
                // same as above?
                unimplemented!()
            }
        }
    }
}

#[derive(Debug)]
pub enum AddMemberError {
    AlreadyExists(Member),
}

#[derive(Debug)]
pub enum AppendMessageError {
    UnknownSender(String),
    UnknownRecipient(String),
}

#[derive(Debug, Clone)]
pub struct Member {
    id: MemberId,
    group: Arc<Group>,
    context: Arc<Thread>,
}

impl Member {
    pub fn add_peer(&self, id: MemberId) -> Result<Member, AddMemberError> {
        self.group.add_member_with_thread(id, &self.context)
    }

    pub fn add_worker(&self, id: MemberId, context: InitContext) -> Result<Member, AddMemberError> {
        let thread = match context {
            InitContext::None => Arc::new(Thread::new()),
            InitContext::Fork => self.context.fork(),
        };
        let result = self.group.add_member_with_thread(id, &thread);
        self.group.secondary.lock().unwrap().push(thread);
        result
    }

    /// Returns an iterator over the messages in this member's context.
    ///
    /// Unlike the underlying thread of messages, the resulting context is
    /// relative to this member. Even when two members are sharing the same
    /// chat, the context returned by this method will be different for each
    /// member. Specifically:
    ///
    /// - Messages are filtered to include only those visible to this member.
    /// - Agents only see their own tool calls.
    /// - For messages belonging to this member, `Msg::belongs_to_first_person`
    ///   will return true, so that downstream code can format the context in
    ///   first-person perspective as needed.
    // It would be nice to iterate over references instead of cloning, but
    // that's tricky with `SkipMap`, and besides, we need to modify some
    // messages anyway. If performance matters (and it might not), the
    // lowest-hanging fruit is probably in the `Msg` enum rather than here
    // (minimize cloning of content fields, maybe with shared ownership &
    // clone-on-write logic).
    pub fn context(&self) -> impl Iterator<Item = Msg> + '_ {
        let first_person =
            MemberId::new_internal(self.id.id(), self.id.display_name(), self.id.human(), true);
        self.context
            .iter()
            .filter_map(move |entry| match entry.value() {
                // Messages with specific recipients are only visible to those
                // recipients; all other messages are visible to everyone.
                Msg::Direct { recipients, .. }
                | Msg::ToolResult { recipients, .. }
                | Msg::Event { recipients, .. }
                | Msg::ControlSignal { recipients, .. } => {
                    if recipients.iter().any(|r| r == &self.id) {
                        Some(entry.value().clone())
                    } else {
                        None
                    }
                }
                // For user and agent messages, set the first-person flag for
                // messages sent by this member.
                Msg::User {
                    sender,
                    timestamp,
                    content,
                } => {
                    if sender == &self.id {
                        Some(Msg::User {
                            sender: first_person.clone(),
                            timestamp: timestamp.clone(),
                            content: content.clone(),
                        })
                    } else {
                        Some(entry.value().clone())
                    }
                }
                Msg::Agent {
                    sender,
                    timestamp,
                    content,
                    tool_calls,
                } => {
                    if sender == &self.id {
                        Some(Msg::Agent {
                            sender: first_person.clone(),
                            timestamp: timestamp.clone(),
                            content: content.clone(),
                            tool_calls: tool_calls.clone(),
                        })
                    } else {
                        Some(Msg::Agent {
                            sender: sender.clone(),
                            timestamp: timestamp.clone(),
                            content: content.clone(),
                            tool_calls: if !self.human() {
                                // Hide tool calls from other agents
                                Vec::new()
                            } else {
                                // But not from humans
                                tool_calls.clone()
                            },
                        })
                    }
                }
                // General instructions are yielded as-is.
                _ => Some(entry.value().clone()),
            })
    }

    pub fn group(&self) -> &Arc<Group> {
        &self.group
    }

    pub fn human(&self) -> bool {
        self.id.human()
    }

    pub fn id(&self) -> &MemberId {
        &self.id
    }
}

#[derive(Debug)]
pub enum InitContext {
    None,
    Fork,
}

struct Thread {
    items: SkipMap<usize, Msg>,
    len: AtomicUsize,
    previous: Option<(Arc<Thread>, usize)>,
    forks: Mutex<Vec<Weak<Thread>>>,
}

impl Thread {
    fn new() -> Self {
        Self {
            items: SkipMap::new(),
            len: AtomicUsize::new(0),
            previous: None,
            forks: Mutex::new(Vec::new()),
        }
    }

    fn fork(self: &Arc<Self>) -> Arc<Self> {
        let len = self.len.load(Ordering::Acquire);
        let fork = Arc::new(Self {
            items: SkipMap::new(),
            len: AtomicUsize::new(len),
            previous: Some((self.clone(), len)),
            forks: Mutex::new(Vec::new()),
        });
        self.forks.lock().unwrap().push(Arc::downgrade(&fork));
        fork
    }

    fn append(&self, message: Msg) {
        let idx = self.len.fetch_add(1, Ordering::AcqRel);
        self.items.insert(idx, message);
    }

    fn iter(&'_ self) -> impl Iterator<Item = Entry<'_, usize, Msg>> + '_ {
        // Include previous list segments
        let mut stack = Vec::new();
        let mut current = Some((self, usize::MAX));
        while let Some((list, stop)) = current {
            stack.push((list.items.iter(), stop));
            current = list.previous.as_ref().map(|(arc, s)| (&**arc, *s));
        }
        // Start at beginning
        stack
            .into_iter()
            .rev()
            .flat_map(|(iter, stop)| iter.take_while(move |e| *e.key() < stop))
    }
}

impl Debug for Thread {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        struct Items<'a>(&'a SkipMap<usize, Msg>);
        impl<'a> Debug for Items<'a> {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                let mut f = f.debug_list();
                for e in self.0.iter() {
                    f.entry(&(e.key(), e.value()));
                }
                f.finish()
            }
        }
        let forks = self.forks.lock().unwrap().clone();
        f.debug_struct("List")
            .field("items", &Items(&self.items))
            .field("len", &self.len)
            .field("previous", &self.previous)
            .field("forks", &forks)
            .finish()
    }
}

/// Identifies a member within a [`Group`].
#[derive(Clone)]
pub struct MemberId {
    repr: Repr,
}

#[derive(Clone)]
enum Repr {
    Inline {
        text: [u8; INLINE_CAP],
        meta: NonZero<u8>,
    },
    Boxed(Box<BoxedText>),
}

#[derive(Clone)]
struct BoxedText {
    split: usize,
    flags: u8,
    text: Box<str>,
}

// Text (name+id, or id alone when the name is a prefix of the id) is stored
// inline when it fits in `INLINE_CAP` bytes. The inline text is zero-padded,
// and inline text never contains a NUL byte, so its length is the position of
// the first NUL (or `INLINE_CAP` if there is none).
//
// `meta` packs the split position and flags into one nonzero byte, which also
// lets the enum use a niche to fit in 16 bytes.
const INLINE_CAP: usize = 15;
const SENTINEL_BIT: u8 = 0b1000_0000;
const OVERLAP_FLAG: u8 = 0b0100_0000;
const SPLIT_SHIFT: u32 = 2;
const SPLIT_MASK: u8 = 0b1111;
const HUMAN_FLAG: u8 = 0b01;
const FIRST_PERSON_FLAG: u8 = 0b10;
const FLAGS_MASK: u8 = HUMAN_FLAG | FIRST_PERSON_FLAG | OVERLAP_FLAG;

impl MemberId {
    pub fn new(id: &str, human: bool) -> Self {
        Self::new_internal(id, None, human, false)
    }

    pub fn with_name(id: &str, name: &str, human: bool) -> Self {
        Self::new_internal(id, Some(name), human, false)
    }

    fn new_internal(id: &str, name: Option<&str>, human: bool, first_person: bool) -> Self {
        let flags = (if human { HUMAN_FLAG } else { 0 })
            | (if first_person { FIRST_PERSON_FLAG } else { 0 });
        let split = name.map_or(0, str::len);
        let overlap = name.is_some_and(|n| id.starts_with(n));
        let flags = flags | if overlap { OVERLAP_FLAG } else { 0 };
        // Parts of the stored text. When the name is a prefix of the id, only
        // the id is stored, and the name shares its bytes.
        let parts: [&str; 2] = match name {
            Some(n) if !overlap => [n, id],
            _ => ["", id],
        };
        let total = parts[0].len() + parts[1].len();
        let has_nul = parts.iter().any(|p| p.as_bytes().contains(&0));

        let repr = if split <= SPLIT_MASK as usize && total <= INLINE_CAP && !has_nul {
            let mut text = [0u8; INLINE_CAP];
            let mid = parts[0].len();
            text[..mid].copy_from_slice(parts[0].as_bytes());
            text[mid..total].copy_from_slice(parts[1].as_bytes());
            let meta = SENTINEL_BIT | ((split as u8) << SPLIT_SHIFT) | flags;
            Repr::Inline {
                text,
                meta: NonZero::new(meta).expect("sentinel bit is always set"),
            }
        } else {
            let mut text = String::with_capacity(total);
            text.push_str(parts[0]);
            text.push_str(parts[1]);
            Repr::Boxed(Box::new(BoxedText {
                split,
                flags,
                text: text.into_boxed_str(),
            }))
        };
        Self { repr }
    }

    /// Returns the stored text, the split position, and the flag bits.
    fn parts(&self) -> (&str, usize, u8) {
        match &self.repr {
            Repr::Inline { text, meta } => {
                let len = text.iter().position(|&b| b == 0).unwrap_or(INLINE_CAP);
                let s = str::from_utf8(&text[..len])
                    .expect("MemberId text is valid UTF-8 by construction");
                let split = ((meta.get() >> SPLIT_SHIFT) & SPLIT_MASK) as usize;
                (s, split, meta.get() & FLAGS_MASK)
            }
            Repr::Boxed(b) => (&b.text, b.split, b.flags),
        }
    }

    /// The display name, if one was given when this id was created.
    pub fn display_name(&self) -> Option<&str> {
        let (text, split, _) = self.parts();
        if split == 0 {
            None
        } else {
            Some(&text[..split])
        }
    }

    /// The id portion. If no name was given, this is the entire text.
    pub fn id(&self) -> &str {
        let (text, split, flags) = self.parts();
        if flags & OVERLAP_FLAG != 0 {
            text
        } else {
            &text[split..]
        }
    }

    /// Whether this member represents a human, as opposed to an AI agent or
    /// other automated participant.
    ///
    /// This value does not affect equality or hashing since groups may not
    /// contain multiple members with the same id but different `human` flags
    /// (e.g. a human and an AI agent both named "Alice").
    pub fn human(&self) -> bool {
        self.parts().2 & HUMAN_FLAG != 0
    }

    /// Value used to flag the first-person subject in a specific context.
    /// This is only relevant within a chat context, and even then it is
    /// typically not used directly but via `Msg::belongs_to_context_owner()`.
    ///
    /// This value does not affect equality or hashing.
    pub fn is_first_person(&self) -> bool {
        self.parts().2 & FIRST_PERSON_FLAG != 0
    }
}

impl std::fmt::Debug for MemberId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MemberId")
            .field("display_name", &self.display_name())
            .field("id", &self.id())
            .field("human", &self.human())
            .finish()
    }
}

impl std::fmt::Display for MemberId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.display_name().unwrap_or_else(|| self.id()))
    }
}

// Enables looking members up by their raw string representation, e.g. as
// found in `Msg::User::sender`, without allocating a `MemberId`.
impl std::borrow::Borrow<str> for MemberId {
    fn borrow(&self) -> &str {
        self.id()
    }
}

// Equality and hashing are based on `id` alone (never `name` or `split`), so
// that two `MemberId`s are never considered equal unless their ids actually
// match -- e.g. name = "Al", id = "ice123" must not equal name = "", id =
// "Alice123" just because their underlying text happens to coincide.
impl PartialEq for MemberId {
    fn eq(&self, other: &Self) -> bool {
        self.id() == other.id()
    }
}

impl Eq for MemberId {}

impl std::hash::Hash for MemberId {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.id().hash(state);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{fmt::Debug, time::Duration};

    #[derive(Debug)]
    struct ThreadBuilder {
        thread: Arc<Thread>,
        time: Option<SystemTime>,
    }

    impl ThreadBuilder {
        fn new() -> Self {
            Self {
                thread: Arc::new(Thread::new()),
                time: None,
            }
        }

        fn with_time(time: SystemTime) -> Self {
            Self {
                thread: Arc::new(Thread::new()),
                time: Some(time),
            }
        }

        fn fork(&self) -> Self {
            Self {
                thread: self.thread.fork(),
                time: self.time.clone(),
            }
        }

        fn platform(self, text: &str) -> Self {
            self.thread.append(Msg::Platform(text.into()));
            self
        }

        fn developer(self, text: &str) -> Self {
            self.thread.append(Msg::Developer(text.into()));
            self
        }

        fn user(mut self, id: &str, text: &str) -> Self {
            let mut next_time = self
                .time
                .map(|t| t.checked_add(Duration::from_secs(1)).unwrap());
            std::mem::swap(&mut self.time, &mut next_time);
            self.thread.append(Msg::User {
                sender: MemberId::new(id, true),
                timestamp: next_time,
                content: vec![ContentPart::Text(text.into())],
            });
            self
        }

        fn agent(mut self, id: &str, text: &str, tool_calls: Vec<ToolCallRequest>) -> Self {
            let mut next_time = self
                .time
                .map(|t| t.checked_add(Duration::from_secs(1)).unwrap());
            std::mem::swap(&mut self.time, &mut next_time);
            self.thread.append(Msg::Agent {
                sender: MemberId::new(id, false),
                timestamp: next_time,
                content: vec![ContentPart::Text(text.into())],
                tool_calls,
            });
            self
        }

        fn tool_result(mut self, recipient: &str, call_id: &str, content: &str) -> Self {
            let mut next_time = self
                .time
                .map(|t| t.checked_add(Duration::from_secs(1)).unwrap());
            std::mem::swap(&mut self.time, &mut next_time);
            self.thread.append(Msg::ToolResult {
                recipients: SmallVec::from_elem(MemberId::new(recipient, true), 1),
                timestamp: next_time,
                tool_call_id: call_id.into(),
                content: vec![ContentPart::Text(content.into())],
            });
            self
        }
    }

    impl AsRef<Thread> for ThreadBuilder {
        fn as_ref(&self) -> &Thread {
            &self.thread
        }
    }

    impl From<ThreadBuilder> for Arc<Thread> {
        fn from(value: ThreadBuilder) -> Self {
            value.thread
        }
    }

    // Wrapper type for comparing the histories of conversations
    struct History<'a>(&'a Thread);

    impl<'a> PartialEq for History<'a> {
        fn eq(&self, other: &Self) -> bool {
            let mut lhs = self.0.iter();
            let mut rhs = other.0.iter();
            loop {
                match (lhs.next(), rhs.next()) {
                    (Some(l), Some(r)) => {
                        if l.value() != r.value() {
                            return false;
                        }
                    }
                    (Some(_), None) | (None, Some(_)) => return false,
                    (None, None) => return true,
                }
            }
        }
    }

    impl<'a> Debug for History<'a> {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            Vec::from_iter(self.0.iter().map(|e| e.value().clone())).fmt(f)
        }
    }

    #[test]
    fn member_id_basics() {
        let id = MemberId::new("Alice123", true);
        assert_eq!(id.id(), "Alice123");
        assert_eq!(id.display_name(), None);
        assert!(id.human());

        let with_name = MemberId::with_name("Alice123", "Alice", true);
        assert_eq!(with_name.id(), "Alice123");
        assert_eq!(with_name.display_name(), Some("Alice"));
        assert!(with_name.human());

        let ai_id = MemberId::new_internal("Robbie", None, false, true);
        assert!(!ai_id.human());
        assert!(ai_id.is_first_person());

        let non_ascii_id = MemberId::new("葛飾", true);
        assert_eq!(non_ascii_id.id(), "葛飾");

        // a few multi-char glyphs, in case we want to test for those
        // 👨‍👩‍👦‍👦
        // 🧑‍🚒
        // 👩🏼‍🚒
        // 👩🏾‍⚕️
    }

    #[test]
    fn member_id_equality_depends_only_on_id() {
        // Same id, different flags: still equal.
        let a = MemberId::new("Alice123", true);
        let b = MemberId::new("Alice123", false);
        let c = MemberId::new("Alice123", true);
        let d = MemberId::new("Alice123", false);
        assert_eq!(a, b);
        assert_eq!(a, c);
        assert_eq!(a, d);

        // Different ids must never compare equal, even if they'd produce the
        // same combined name+id text once `name` is implemented.
        let c = MemberId::new("Alice124", true);
        assert_ne!(a, c);

        // Usable as a HashMap key, looked up by raw `id`.
        let mut map = HashMap::new();
        map.insert(a.clone(), "first");
        assert_eq!(map.get(&b), Some(&"first"));
        assert_eq!(map.get(&c), None);
    }

    #[test]
    fn member_id_inline_or_boxed() {
        assert_eq!(std::mem::size_of::<MemberId>(), 16);
        let is_inline = |id: &MemberId| matches!(id.repr, Repr::Inline { .. });

        // Up to 15 bytes of stored text is inline.
        assert!(is_inline(&MemberId::new("abcdefghijklmno", true)));
        assert!(!is_inline(&MemberId::new("abcdefghijklmnop", true)));

        // Stored text is name + id, unless the name is a prefix of the id.
        assert!(is_inline(&MemberId::with_name("Alice123", "Alice", true)));
        assert!(is_inline(&MemberId::with_name(
            "Alice1234567890",
            "Alice",
            true
        )));
        assert!(!is_inline(&MemberId::with_name(
            "Alice123456789",
            "Bobbyyy",
            true
        )));

        // Split position must fit in 4 bits, so a longer name is boxed.
        assert!(!is_inline(&MemberId::with_name(
            "Alice",
            "abcdefghijklmnop",
            true
        )));

        // NUL bytes would clash with the zero padding used for inline text.
        let nul = MemberId::new("a\0b", true);
        assert!(!is_inline(&nul));
        assert_eq!(nul.id(), "a\0b");
    }

    #[test]
    fn member_id_name_prefix_shares_storage() {
        let addr = |s: &str| s.as_ptr() as usize;
        let within = |inner: &str, outer: &str| {
            addr(inner) >= addr(outer) && addr(inner) + inner.len() <= addr(outer) + outer.len()
        };

        // Name is a prefix of the id: the display name borrows from the id.
        let overlap = MemberId::with_name("Alice1234567890", "Alice", true);
        assert_eq!(overlap.id(), "Alice1234567890");
        assert_eq!(overlap.display_name(), Some("Alice"));
        assert_eq!(addr(overlap.display_name().unwrap()), addr(overlap.id()));
        assert!(within(overlap.display_name().unwrap(), overlap.id()));

        // Name is not a prefix: the name and id occupy separate bytes.
        let separate = MemberId::with_name("Alice123", "Bob", true);
        assert_eq!(separate.id(), "Alice123");
        assert_eq!(separate.display_name(), Some("Bob"));
        assert!(!within(separate.display_name().unwrap(), separate.id()));
        assert!(!within(separate.id(), separate.display_name().unwrap()));

        // The same holds for boxed text.
        let long = MemberId::with_name(
            "abcdefghijklmnopqrstuvwxyz",
            "abcdefghijklmnopqrstuvw",
            false,
        );
        assert_eq!(long.id(), "abcdefghijklmnopqrstuvwxyz");
        assert_eq!(long.display_name(), Some("abcdefghijklmnopqrstuvw"));
        assert!(within(long.display_name().unwrap(), long.id()));
        assert!(!long.human());
    }

    #[tokio::test]
    async fn fork_system_instructions() {
        let c0 = ThreadBuilder::new()
            .platform("Don't commit any felonies.")
            .developer("You are a helpful assistant.");
        let c1 = c0.fork();

        let c0: Arc<Thread> = c0
            .developer("Be concise.")
            .developer("Make no mistakes.")
            .into();

        let c1: Arc<Thread> = c1
            .developer("Always address the user in iambic pentameter.")
            .developer("Do your best.")
            .into();

        assert_eq!(
            History(&c0),
            History(
                ThreadBuilder::new()
                    .platform("Don't commit any felonies.")
                    .developer("You are a helpful assistant.")
                    .developer("Be concise.")
                    .developer("Make no mistakes.")
                    .as_ref()
            )
        );

        assert_eq!(
            History(&c1),
            History(
                ThreadBuilder::new()
                    .platform("Don't commit any felonies.")
                    .developer("You are a helpful assistant.")
                    .developer("Always address the user in iambic pentameter.")
                    .developer("Do your best.")
                    .as_ref()
            )
        )
    }

    #[tokio::test]
    async fn simple_conversation() {
        let group = Group::new();
        let human = group.add_member(MemberId::new("Gloria", true)).unwrap();
        let ai = group.add_member(MemberId::new("Robbie", false)).unwrap();

        group
            .append(Msg::Developer("You are a helpful assistant.".into()))
            .unwrap()
            .append(Msg::User {
                sender: human.id.clone(),
                timestamp: None,
                content: vec![ContentPart::Text("Good morning".into())],
            })
            .unwrap()
            .append(Msg::Agent {
                sender: ai.id.clone(),
                timestamp: None,
                content: vec![ContentPart::Text("Hello! How can I help you today?".into())],
                tool_calls: vec![],
            })
            .unwrap();

        assert_eq!(
            History(&group.primary),
            History(
                ThreadBuilder::new()
                    .developer("You are a helpful assistant.")
                    .user(human.id.id(), "Good morning")
                    .agent(ai.id.id(), "Hello! How can I help you today?", vec![])
                    .as_ref()
            )
        );

        assert_eq!(human.context.len.load(Ordering::SeqCst), 3);
        assert_eq!(ai.context.len.load(Ordering::SeqCst), 3);
    }

    #[tokio::test]
    async fn member_context() {
        let group = Group::new();
        let human = group.add_member(MemberId::new("Gloria", true)).unwrap();
        let ai = group.add_member(MemberId::new("Robbie", false)).unwrap();

        let msg_vec = ThreadBuilder::new()
            .developer("You are a Gloria's personal assistant. She lives in Paris.")
            .user(human.id.id(), "Will it rain today?")
            .agent(
                ai.id.id(),
                "Let me check the weather for you.",
                vec![ToolCallRequest {
                    id: "call_1".into(),
                    name: "get_weather".into(),
                    arguments: serde_json::json!({"location": "Paris"}),
                }],
            )
            .tool_result(
                ai.id.id(),
                "call_1".into(),
                "Temperature: 18°C, Chance of rain: 20%",
            )
            .as_ref()
            .iter()
            .map(|e| e.value().clone())
            .collect::<Vec<_>>();

        group
            .append(msg_vec[0].clone())
            .unwrap()
            .append(msg_vec[1].clone())
            .unwrap()
            .append(msg_vec[2].clone())
            .unwrap()
            .append(msg_vec[3].clone())
            .unwrap();

        let human_cx = human.context().collect::<Vec<_>>();
        assert_eq!(human_cx, msg_vec[0..3]);
        assert!(!human_cx[0].belongs_to_first_person());
        assert!(human_cx[1].belongs_to_first_person());
        assert!(!human_cx[2].belongs_to_first_person());

        let ai_cx = ai.context().collect::<Vec<_>>();
        assert_eq!(ai_cx, msg_vec);
        assert!(!ai_cx[0].belongs_to_first_person());
        assert!(!ai_cx[1].belongs_to_first_person());
        assert!(ai_cx[2].belongs_to_first_person());
        assert!(!ai_cx[3].belongs_to_first_person());
    }
}
