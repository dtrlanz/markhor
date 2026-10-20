use crossbeam_skiplist::{SkipMap, map::Entry};
use serde_json::Value;
use smallvec::SmallVec;
use std::{
    collections::HashMap,
    fmt::Debug,
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

    pub fn add_member(self: &Arc<Self>, name: &str, human: bool) -> Result<Member, AddMemberError> {
        self.add_member_with_thread(name, human, &self.primary)
    }

    fn add_member_with_thread(
        self: &Arc<Self>,
        name: &str,
        human: bool,
        thread: &Arc<Thread>,
    ) -> Result<Member, AddMemberError> {
        let id = MemberId::new(name, human);
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
    pub fn add_peer(&self, name: &str, human: bool) -> Result<Member, AddMemberError> {
        self.group
            .add_member_with_thread(name, human, &self.context)
    }

    pub fn add_worker(&self, name: &str, context: InitContext) -> Result<Member, AddMemberError> {
        let thread = match context {
            InitContext::None => Arc::new(Thread::new()),
            InitContext::Fork => self.context.fork(),
        };
        let result = self.group.add_member_with_thread(name, false, &thread);
        self.group.secondary.lock().unwrap().push(thread);
        result
    }

    pub fn group(&self) -> &Arc<Group> {
        &self.group
    }

    pub fn id(&self) -> &MemberId {
        &self.id
    }

    pub fn human(&self) -> bool {
        self.id.human()
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
///
/// To keep `MemberId` the size of a single fat pointer (16 bytes on 64-bit
/// platforms), every field -- the name/id split point, flags, and the
/// combined name+id text -- is packed into a single `Arc<[u8]>` allocation
/// rather than stored as separate struct fields.
///
/// Byte layout: `[0]` = split (byte offset, within the text that follows,
/// where the name ends and the id begins; `0` means there is no name),
/// `[1]` = flags (bit `0` = human, remaining bits reserved/unused),
/// `[2..]` = UTF-8 encoded `name` followed immediately by `id`.
#[derive(Clone)]
pub struct MemberId {
    inner: Arc<[u8]>,
}

const SPLIT_BYTE: usize = 0;
const FLAGS_BYTE: usize = 1;
const HEADER_LEN: usize = 2;
const HUMAN_FLAG: u8 = 0b01;

impl MemberId {
    fn new(id: &str, human: bool) -> Self {
        let mut bytes = Vec::with_capacity(HEADER_LEN + id.len());
        bytes.push(0); // split: no name for now
        bytes.push(if human { HUMAN_FLAG } else { 0 });
        bytes.extend_from_slice(id.as_bytes());
        Self {
            inner: bytes.into(),
        }
    }

    fn split(&self) -> usize {
        self.inner[SPLIT_BYTE] as usize
    }

    fn text(&self) -> &str {
        str::from_utf8(&self.inner[HEADER_LEN..])
            .expect("MemberId text is valid UTF-8 by construction")
    }

    /// The display name, if one was given when this id was created.
    ///
    /// Not used anywhere yet; reserved for future functionality.
    pub fn name(&self) -> Option<&str> {
        let split = self.split();
        if split == 0 {
            None
        } else {
            Some(&self.text()[..split])
        }
    }

    /// The id portion. If no name was given, this is the entire text.
    pub fn id(&self) -> &str {
        &self.text()[self.split()..]
    }

    /// Whether this member represents a human, as opposed to an AI agent or
    /// other automated participant.
    pub fn human(&self) -> bool {
        self.inner[FLAGS_BYTE] & HUMAN_FLAG != 0
    }
}

impl std::fmt::Debug for MemberId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MemberId")
            .field("name", &self.name())
            .field("id", &self.id())
            .field("human", &self.human())
            .finish()
    }
}

impl std::ops::Deref for MemberId {
    type Target = str;

    fn deref(&self) -> &str {
        // Name isn't used yet; fall back to id when there's no name.
        self.name().unwrap_or_else(|| self.id())
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
    use std::{fmt::Debug, time::Duration};

    // Builds a `MemberId` with `human = false`, for tests that don't care
    // about the flag.
    fn member_id(id: &str) -> MemberId {
        MemberId::new(id, false)
    }

    use crate::tool::Tool;

    use super::*;

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

        fn user(mut self, name: &str, text: &str) -> Self {
            let mut next_time = self
                .time
                .map(|t| t.checked_add(Duration::from_secs(1)).unwrap());
            std::mem::swap(&mut self.time, &mut next_time);
            self.thread.append(Msg::User {
                sender: member_id(name),
                timestamp: next_time,
                content: vec![ContentPart::Text(text.into())],
            });
            self
        }

        fn agent(mut self, name: &str, text: &str, tool_calls: Vec<ToolCallRequest>) -> Self {
            let mut next_time = self
                .time
                .map(|t| t.checked_add(Duration::from_secs(1)).unwrap());
            std::mem::swap(&mut self.time, &mut next_time);
            self.thread.append(Msg::Agent {
                sender: member_id(name),
                timestamp: next_time,
                content: vec![ContentPart::Text(text.into())],
                tool_calls,
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
        assert_eq!(id.name(), None);
        assert!(id.human());

        let ai_id = MemberId::new("Robbie", false);
        assert!(!ai_id.human());
    }

    #[test]
    fn member_id_equality_depends_only_on_id() {
        // Same id, different `human` flag: still equal.
        let a = MemberId::new("Alice123", true);
        let b = MemberId::new("Alice123", false);
        assert_eq!(a, b);

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
        let human = group.add_member("Gloria", true).unwrap();
        let ai = group.add_member("Robbie", false).unwrap();

        group.append(Msg::Developer("You are a helpful assistant.".into()));

        group
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

        dbg!(&group);

        assert_eq!(
            History(&group.primary),
            History(
                ThreadBuilder::new()
                    .developer("You are a helpful assistant.")
                    .user(&human.id, "Good morning")
                    .agent(&ai.id, "Hello! How can I help you today?", vec![])
                    .as_ref()
            )
        );

        assert_eq!(human.context.len.load(Ordering::SeqCst), 3);
        assert_eq!(ai.context.len.load(Ordering::SeqCst), 3);
    }
}
