use super::{
    Msg,
    chat_ui::{MsgEvent, MsgId, ReasoningEvent, ToolCallEvent, WorkerEvent},
    member::{Member, MemberId},
};
use crate::{event::Subscription, events};
use crossbeam_skiplist::{SkipMap, map::Entry};
use std::{
    collections::HashMap,
    fmt::Debug,
    sync::{
        Arc, Mutex, Weak,
        atomic::{AtomicUsize, Ordering},
    },
};
use thiserror::Error;
use uuid::Uuid;

#[derive(Debug)]
pub struct Group {
    pub(super) members: Mutex<HashMap<MemberId, Arc<Thread>>>,
    pub(super) primary: Arc<Thread>,
    pub(super) secondary: Mutex<Vec<Arc<Thread>>>,
}

impl Group {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            members: Default::default(),
            primary: Arc::new(Thread::new()),
            secondary: Default::default(),
        })
    }

    pub fn add_member(self: &Arc<Self>, id: MemberId) -> Result<Member, AddMemberError> {
        self.add_member_with_thread(id, &self.primary)
    }

    pub(super) fn add_member_with_thread(
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
        let ptcp = Member::new(id, Arc::clone(self), new_or_existing_thread);
        if inserted {
            Ok(ptcp)
        } else {
            Err(AddMemberError::AlreadyExists(ptcp))
        }
    }

    pub fn append(&self, message: Msg) -> Result<&Self, AppendMessageError> {
        match &message {
            Msg::Platform(_) | Msg::Developer(_) => {
                self.primary.append(message, None);
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
                ptcp.append(message.clone(), None);
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
                    t.append(message.clone(), None);
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

    /// Starts a message from `sender`. `MsgEvent::Begin` is dispatched to the
    /// sender's thread immediately, before the message exists.
    ///
    /// Complete the returned [`PendingMsg`] with [`PendingMsg::complete`] or
    /// report a failure with [`PendingMsg::fail`]. Dropping it counts as a
    /// failure.
    pub fn begin_message(&self, sender: &MemberId) -> Result<PendingMsg, AppendMessageError> {
        let thread = self
            .members
            .lock()
            .unwrap()
            .get(sender)
            .cloned()
            .ok_or_else(|| AppendMessageError::UnknownSender(sender.to_string()))?;
        Ok(PendingMsg::begin(sender.clone(), thread))
    }
}

/// A message that has begun but not yet been completed. See
/// [`Group::begin_message`].
#[must_use = "a pending message reports a failure when dropped; complete or fail it"]
pub struct PendingMsg {
    id: MsgId,
    sender: MemberId,
    thread: Arc<Thread>,
    settled: bool,
}

impl PendingMsg {
    fn begin(sender: MemberId, thread: Arc<Thread>) -> Self {
        let id = MsgId::next();
        thread.dispatch_msg(MsgEvent::Begin(id, sender.clone()));
        Self {
            id,
            sender,
            thread,
            settled: false,
        }
    }

    /// The id that this message's events are tagged with.
    pub fn id(&self) -> MsgId {
        self.id
    }

    /// Appends the completed message to the sender's thread, followed by
    /// `MsgEvent::Full`.
    ///
    /// Returns [`AppendMessageError::SenderMismatch`] unless `msg` is a
    /// `User` or `Agent` message sent by the member this message began for.
    pub fn complete(mut self, msg: Msg) -> Result<(), AppendMessageError> {
        match &msg {
            Msg::User { sender, .. } | Msg::Agent { sender, .. } if sender == &self.sender => {}
            _ => return Err(AppendMessageError::SenderMismatch(self.sender.to_string())),
        }
        self.settled = true;
        self.thread.append(msg, Some(self.id));
        Ok(())
    }

    /// Reports that the message will not be completed, by dispatching
    /// `MsgEvent::Log` with `reason` in place of `MsgEvent::Full`.
    pub fn fail(mut self, reason: impl Into<String>) {
        self.settled = true;
        self.thread
            .dispatch_msg(MsgEvent::Log(self.id, reason.into()));
    }
}

impl Drop for PendingMsg {
    fn drop(&mut self) {
        if !self.settled {
            self.thread.dispatch_msg(MsgEvent::Log(
                self.id,
                "message was abandoned before completion".into(),
            ));
        }
    }
}

#[derive(Error, Debug)]
pub enum AddMemberError {
    #[error("Member with id '{}' already exists in the group", .0.id())]
    AlreadyExists(Member),
}

#[derive(Debug)]
pub enum AppendMessageError {
    UnknownSender(String),
    UnknownRecipient(String),
    /// A pending message was completed with a message not sent by its sender.
    SenderMismatch(String),
}

#[derive(Debug)]
pub enum InitContext {
    None,
    Fork,
}

events! {
    struct ThreadEvents {
        message_appended: (Msg, usize),
        message: (MsgEvent,),
        reasoning: (ReasoningEvent,),
        tool_call: (ToolCallEvent,),
        worker: (WorkerEvent,),
    }
}

/// The member that authored `msg`, if the message has one.
fn sender_of(msg: &Msg) -> Option<&MemberId> {
    match msg {
        Msg::User { sender, .. } | Msg::Agent { sender, .. } | Msg::Direct { sender, .. } => {
            Some(sender)
        }
        Msg::Platform(_) | Msg::Developer(_) | Msg::ToolResult { .. } | Msg::Event { .. } | Msg::ControlSignal { .. } => None,
    }
}

pub struct Thread {
    id: Uuid,
    items: SkipMap<usize, Msg>,
    len: AtomicUsize,
    previous: Option<(Arc<Thread>, usize)>,
    forks: Mutex<Vec<Weak<Thread>>>,
    events: ThreadEvents,
}

impl Thread {
    pub(super) fn new() -> Self {
        Self {
            id: Uuid::new_v4(),
            items: SkipMap::new(),
            len: AtomicUsize::new(0),
            previous: None,
            forks: Mutex::new(Vec::new()),
            events: ThreadEvents::default(),
        }
    }

    /// Registers a listener that is called with `(message, index)` after a
    /// message has been appended to this thread and is readable via `len` and
    /// `iter_until`. The listener stays registered while the returned
    /// `Subscription` is alive.
    pub fn on_message_appended(
        &self,
        callback: impl Fn(&Msg, usize) + Send + Sync + 'static,
    ) -> Subscription {
        self.events
            .message_appended
            // Deref index for convenience (or because it's more common to pass
            // indices by value).
            .add_listener(move |msg: &Msg, idx: &usize| callback(msg, *idx))
    }

    /// Registers a listener for message lifecycle events (`Begin`, `Part`,
    /// `Full`, `Log`) on this thread. Mirrors `ChatUi::handle_message`.
    /// `Part` is not yet emitted, since model responses are not streamed.
    pub fn on_message(&self, callback: impl Fn(&MsgEvent) + Send + Sync + 'static) -> Subscription {
        self.events.message.add_listener(move |e: &MsgEvent| callback(e))
    }

    /// Registers a listener for reasoning events. Mirrors
    /// `ChatUi::handle_reasoning`.
    ///
    /// Not yet emitted: reasoning dispatch is deferred until its design is settled.
    pub fn on_reasoning(
        &self,
        callback: impl Fn(&ReasoningEvent) + Send + Sync + 'static,
    ) -> Subscription {
        self.events
            .reasoning
            .add_listener(move |e: &ReasoningEvent| callback(e))
    }

    /// Registers a listener for tool call events. Mirrors
    /// `ChatUi::handle_tool_call`.
    ///
    /// Not yet emitted: tool call dispatch is deferred until its design is settled.
    pub fn on_tool_call(
        &self,
        callback: impl Fn(&ToolCallEvent) + Send + Sync + 'static,
    ) -> Subscription {
        self.events
            .tool_call
            .add_listener(move |e: &ToolCallEvent| callback(e))
    }

    /// Registers a listener for worker events. Mirrors `ChatUi::handle_worker`.
    ///
    /// Not yet emitted: core has no worker concept yet.
    pub fn on_worker(&self, callback: impl Fn(&WorkerEvent) + Send + Sync + 'static) -> Subscription {
        self.events.worker.add_listener(move |e: &WorkerEvent| callback(e))
    }

    /// The unique identifier of this thread. Threads compare equal iff their
    /// ids match.
    pub fn id(&self) -> Uuid {
        self.id
    }

    pub(super) fn fork(self: &Arc<Self>) -> Arc<Self> {
        let len = self.len.load(Ordering::Acquire);
        let fork = Arc::new(Self {
            id: Uuid::new_v4(),
            items: SkipMap::new(),
            len: AtomicUsize::new(len),
            previous: Some((self.clone(), len)),
            forks: Mutex::new(Vec::new()),
            events: ThreadEvents::default(),
        });
        self.forks.lock().unwrap().push(Arc::downgrade(&fork));
        fork
    }

    /// Stores `message` and dispatches its events. If `pending` is `Some`, the
    /// `Begin` event was already dispatched under that id, so only `Full` is
    /// sent. Otherwise a `Begin` precedes `Full` when the message has a sender.
    /// `message_appended` is dispatched last.
    fn append(&self, message: Msg, pending: Option<MsgId>) {
        let idx = self.len.fetch_add(1, Ordering::AcqRel);
        self.items.insert(idx, message.clone());
        match (pending, sender_of(&message)) {
            (Some(id), _) => self.dispatch_msg(MsgEvent::Full(id, message.clone())),
            (None, Some(sender)) => {
                let id = MsgId::next();
                self.dispatch_msg(MsgEvent::Begin(id, sender.clone()));
                self.dispatch_msg(MsgEvent::Full(id, message.clone()));
            }
            (None, None) => self.dispatch_msg(MsgEvent::Full(MsgId::next(), message.clone())),
        }
        self.events.message_appended.dispatch(&(message, idx));
    }

    fn dispatch_msg(&self, event: MsgEvent) {
        self.events.message.dispatch(&(event,));
    }


    /// Iterates over messages with indices less than `end`, including
    /// messages inherited from previous segments.
    pub(super) fn iter_until(
        &'_ self,
        end: usize,
    ) -> impl Iterator<Item = Entry<'_, usize, Msg>> + '_ {
        // Include previous list segments, each clamped to `end`
        let mut stack = Vec::new();
        let mut current = Some((self, end));
        while let Some((list, stop)) = current {
            stack.push((list.items.iter(), stop));
            current = list
                .previous
                .as_ref()
                .map(|(arc, s)| (&**arc, (*s).min(end)));
        }
        // Start at beginning
        stack
            .into_iter()
            .rev()
            .flat_map(|(iter, stop)| iter.take_while(move |e| *e.key() < stop))
    }

    pub fn len(&self) -> usize {
        self.len.load(Ordering::Acquire)
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
            .field("id", &self.id)
            .field("items", &Items(&self.items))
            .field("len", &self.len)
            .field("previous", &self.previous)
            .field("forks", &forks)
            .finish()
    }
}

// Threads are identified by their unique `id`, not by their contents or address.
impl PartialEq for Thread {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id
    }
}

impl Eq for Thread {}

impl std::hash::Hash for Thread {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.id.hash(state);
    }
}

#[cfg(test)]
pub(super) mod thread_builder {
    use super::*;
    use crate::{
        chat::common::{ContentPart, ToolCallRequest},
        extension::ModelId,
    };
    use smallvec::SmallVec;
    use std::time::SystemTime;

    #[derive(Debug)]
    pub(crate) struct ThreadBuilder {
        thread: Arc<Thread>,
        time: Option<SystemTime>,
    }

    impl ThreadBuilder {
        pub fn new() -> Self {
            Self {
                thread: Arc::new(Thread::new()),
                time: None,
            }
        }

        pub fn with_time(time: SystemTime) -> Self {
            Self {
                thread: Arc::new(Thread::new()),
                time: Some(time),
            }
        }

        pub fn fork(&self) -> Self {
            Self {
                thread: self.thread.fork(),
                time: self.time.clone(),
            }
        }

        pub fn msg(self, msg: Msg) -> Self {
            self.thread.append(msg, None);
            self
        }

        pub fn platform(self, text: &str) -> Self {
            self.msg(Msg::Platform(text.into()))
        }

        pub fn developer(self, text: &str) -> Self {
            self.msg(Msg::Developer(text.into()))
        }

        pub fn user(mut self, id: &str, text: &str) -> Self {
            let mut next_time = self
                .time
                .map(|t| t.checked_add(std::time::Duration::from_secs(1)).unwrap());
            std::mem::swap(&mut self.time, &mut next_time);
            self.msg(Msg::User {
                sender: MemberId::new(id, true),
                timestamp: next_time,
                content: vec![ContentPart::Text(text.into())],
            })
        }

        pub fn agent(mut self, id: &str, text: &str, tool_calls: Vec<ToolCallRequest>) -> Self {
            let mut next_time = self
                .time
                .map(|t| t.checked_add(std::time::Duration::from_secs(1)).unwrap());
            std::mem::swap(&mut self.time, &mut next_time);
            self.msg(Msg::Agent {
                sender: MemberId::new(id, false),
                timestamp: next_time,
                content: vec![ContentPart::Text(text.into())],
                tool_calls,
                model: ModelId::new("spiff-3-nano", "zygomorphic-labs"),
                reasoning_content: Default::default(),
                metadata: Default::default(),
            })
        }

        pub fn tool_result(mut self, recipient: &str, call_id: &str, content: &str) -> Self {
            let mut next_time = self
                .time
                .map(|t| t.checked_add(std::time::Duration::from_secs(1)).unwrap());
            std::mem::swap(&mut self.time, &mut next_time);
            self.msg(Msg::ToolResult {
                recipients: SmallVec::from_elem(MemberId::new(recipient, true), 1),
                timestamp: next_time,
                tool_call_id: call_id.into(),
                content: vec![ContentPart::Text(content.into())],
            })
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
}

#[cfg(test)]
mod tests {
    use crate::chat::common::{ContentPart, ToolCallRequest};
    use crate::extension::ModelId;

    use super::thread_builder::ThreadBuilder;
    use super::*;
    use std::fmt::Debug;

    // Wrapper type for comparing the histories of conversations
    struct History<'a>(&'a Thread);

    impl<'a> PartialEq for History<'a> {
        fn eq(&self, other: &Self) -> bool {
            let mut lhs = self.0.iter_until(usize::MAX);
            let mut rhs = other.0.iter_until(usize::MAX);
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
            Vec::from_iter(self.0.iter_until(usize::MAX).map(|e| e.value().clone())).fmt(f)
        }
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
                model: ModelId::new("spiff-3-nano", "zygomorphic-labs"),
                reasoning_content: Default::default(),
                metadata: Default::default(),
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

        assert_eq!(human.thread.len.load(Ordering::SeqCst), 3);
        assert_eq!(ai.thread.len.load(Ordering::SeqCst), 3);
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
            .iter_until(usize::MAX)
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

    fn keys(thread: &Thread, end: usize) -> Vec<usize> {
        thread.iter_until(end).map(|e| *e.key()).collect()
    }

    #[test]
    fn iter_until_prefix() {
        let t: Arc<Thread> = ThreadBuilder::new()
            .developer("a")
            .developer("b")
            .developer("c")
            .into();

        assert_eq!(keys(&t, 0), Vec::<usize>::new());
        assert_eq!(keys(&t, 1), vec![0]);
        assert_eq!(keys(&t, 2), vec![0, 1]);
        assert_eq!(keys(&t, 3), vec![0, 1, 2]);
        assert_eq!(keys(&t, 100), vec![0, 1, 2]);
        assert_eq!(
            History(&t)
                .0
                .iter_until(usize::MAX)
                .map(|e| *e.key())
                .collect::<Vec<_>>(),
            keys(&t, usize::MAX)
        );
    }

    #[test]
    fn iter_until_clamps_parent_segments_at_fork() {
        let root = ThreadBuilder::new().developer("a").developer("b");
        let fork_builder = root.fork();
        let root: Arc<Thread> = root.developer("c").into();
        let fork: Arc<Thread> = fork_builder.developer("d").into();

        // Fork inherits parent's [0, 1] and then its own message at index 2.
        assert_eq!(keys(&fork, usize::MAX), vec![0, 1, 2]);
        // Ending before the fork point must not leak parent messages past `end`.
        assert_eq!(keys(&fork, 1), vec![0]);
        assert_eq!(keys(&fork, 0), Vec::<usize>::new());
        // Parent is unaffected by the fork's messages.
        assert_eq!(keys(&root, usize::MAX), vec![0, 1, 2]);
        assert_eq!(keys(&root, 2), vec![0, 1]);
    }

    #[tokio::test]
    async fn member_context_until() {
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
            .iter_until(usize::MAX)
            .map(|e| e.value().clone())
            .collect::<Vec<_>>();

        for msg in &msg_vec {
            group.append(msg.clone()).unwrap();
        }

        let human_cx = human.context_until(2).collect::<Vec<_>>();
        assert_eq!(human_cx, msg_vec[0..2]);
        assert!(!human_cx[0].belongs_to_first_person());
        assert!(human_cx[1].belongs_to_first_person());

        let ai_cx = ai.context_until(3).collect::<Vec<_>>();
        assert_eq!(ai_cx, msg_vec[0..3]);
        assert!(ai_cx[2].belongs_to_first_person());

        assert_eq!(ai.context_until(0).count(), 0);
        assert_eq!(ai.context_until(100).count(), 4);
        assert_eq!(
            ai.context_until(usize::MAX).collect::<Vec<_>>(),
            ai.context().collect::<Vec<_>>()
        );
    }

    #[tokio::test]
    async fn message_appended_passes_message_and_index_in_order() {
        let group = Group::new();
        let human = group.add_member(MemberId::new("Gloria", true)).unwrap();

        let seen = Arc::new(Mutex::new(Vec::new()));
        let s = seen.clone();
        let _sub = group.primary.on_message_appended(move |msg, idx| {
            s.lock().unwrap().push((msg.clone(), idx));
        });

        let first = Msg::Developer("You are a helpful assistant.".into());
        let second = Msg::User {
            sender: human.id.clone(),
            timestamp: None,
            content: vec![ContentPart::Text("Good morning".into())],
        };
        group
            .append(first.clone())
            .unwrap()
            .append(second.clone())
            .unwrap();

        assert_eq!(*seen.lock().unwrap(), vec![(first, 0), (second, 1)]);
    }

    #[tokio::test]
    async fn message_appended_fires_once_per_thread_for_multi_recipient_message() {
        let group = Group::new();
        let a = group.add_member(MemberId::new("A", true)).unwrap();
        let b = group.add_member(MemberId::new("B", false)).unwrap();
        // C lives on a separate thread, so a message to A and C reaches two threads.
        let other = Arc::new(Thread::new());
        let c = group
            .add_member_with_thread(MemberId::new("C", false), &other)
            .unwrap();

        let primary_count = Arc::new(AtomicUsize::new(0));
        let other_count = Arc::new(AtomicUsize::new(0));
        let pc = primary_count.clone();
        let _primary_sub = group.primary.on_message_appended(move |_, _| {
            pc.fetch_add(1, Ordering::SeqCst);
        });
        let oc = other_count.clone();
        let _other_sub = other.on_message_appended(move |_, _| {
            oc.fetch_add(1, Ordering::SeqCst);
        });

        // A and B share the primary thread, so the primary thread gets one copy.
        group
            .append(Msg::Direct {
                sender: b.id.clone(),
                timestamp: None,
                tool_call_id: String::new(),
                recipients: smallvec::smallvec![a.id.clone(), b.id.clone(), c.id.clone()],
                content: vec![ContentPart::Text("psst".into())],
            })
            .unwrap();

        assert_eq!(primary_count.load(Ordering::SeqCst), 1);
        assert_eq!(other_count.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn dropping_message_appended_subscription_stops_listener() {
        let thread: Arc<Thread> = ThreadBuilder::new().into();
        let count = Arc::new(AtomicUsize::new(0));
        let c = count.clone();
        let sub = thread.on_message_appended(move |_, _| {
            c.fetch_add(1, Ordering::SeqCst);
        });

        thread.append(Msg::Developer("one".into()), None);
        drop(sub);
        thread.append(Msg::Developer("two".into()), None);

        assert_eq!(count.load(Ordering::SeqCst), 1);
        assert_eq!(thread.len(), 2);
    }

    #[test]
    fn fork_has_its_own_message_appended_listeners() {
        let root: Arc<Thread> = ThreadBuilder::new().developer("a").into();
        let root_count = Arc::new(AtomicUsize::new(0));
        let rc = root_count.clone();
        let _root_sub = root.on_message_appended(move |_, _| {
            rc.fetch_add(1, Ordering::SeqCst);
        });

        let fork = root.fork();
        let fork_count = Arc::new(AtomicUsize::new(0));
        let fc = fork_count.clone();
        let _fork_sub = fork.on_message_appended(move |_, _| {
            fc.fetch_add(1, Ordering::SeqCst);
        });

        fork.append(Msg::Developer("fork only".into()), None);
        assert_eq!(root_count.load(Ordering::SeqCst), 0);
        assert_eq!(fork_count.load(Ordering::SeqCst), 1);

        root.append(Msg::Developer("root only".into()), None);
        assert_eq!(root_count.load(Ordering::SeqCst), 1);
        assert_eq!(fork_count.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn message_appended_listener_sees_stored_message() {
        let thread: Arc<Thread> = ThreadBuilder::new().into();
        // Weak avoids a reference cycle through the listener's captured handle.
        let weak = Arc::downgrade(&thread);
        let seen = Arc::new(Mutex::new(Vec::new()));
        let s = seen.clone();
        let _sub = thread.on_message_appended(move |msg, idx| {
            let thread = weak.upgrade().unwrap();
            let stored = thread
                .iter_until(usize::MAX)
                .find(|e| *e.key() == idx)
                .map(|e| e.value().clone());
            s.lock().unwrap().push((stored, msg.clone()));
        });

        thread.append(Msg::Developer("hello".into()), None);

        let seen = seen.lock().unwrap();
        assert_eq!(seen.len(), 1);
        let (stored, msg) = &seen[0];
        assert_eq!(stored.as_ref(), Some(msg));
        assert_eq!(thread.len(), 1);
    }

    fn user_msg(sender: &str, text: &str) -> Msg {
        Msg::User {
            sender: MemberId::new(sender, true),
            timestamp: None,
            content: vec![ContentPart::Text(text.into())],
        }
    }

    fn record_message_events(thread: &Thread) -> (Subscription, Arc<Mutex<Vec<MsgEvent>>>) {
        let events = Arc::new(Mutex::new(Vec::new()));
        let e = events.clone();
        let sub = thread.on_message(move |event| e.lock().unwrap().push(event.clone()));
        (sub, events)
    }

    #[test]
    fn pending_message_begins_before_it_is_completed() {
        let group = Group::new();
        let alice = group.add_member(MemberId::new("alice", true)).unwrap();
        let (_sub, events) = record_message_events(alice.thread());

        let pending = group.begin_message(alice.id()).unwrap();
        let id = pending.id();
        assert_eq!(events.lock().unwrap().len(), 1);
        pending.complete(user_msg("alice", "hi")).unwrap();

        let seen = events.lock().unwrap();
        match &seen[..] {
            [MsgEvent::Begin(b, sender), MsgEvent::Full(f, Msg::User { .. })] => {
                assert_eq!(*b, id);
                assert_eq!(*f, id);
                assert_eq!(sender, alice.id());
            }
            other => panic!("unexpected events: {other:?}"),
        }
        assert_eq!(alice.thread().len(), 1);
    }

    #[test]
    fn failed_pending_message_logs_without_full() {
        let group = Group::new();
        let alice = group.add_member(MemberId::new("alice", true)).unwrap();
        let (_sub, events) = record_message_events(alice.thread());

        let pending = group.begin_message(alice.id()).unwrap();
        let id = pending.id();
        pending.fail("boom");

        let seen = events.lock().unwrap();
        match &seen[..] {
            [MsgEvent::Begin(b, _), MsgEvent::Log(l, reason)] => {
                assert_eq!(*b, id);
                assert_eq!(*l, id);
                assert_eq!(reason, "boom");
            }
            other => panic!("unexpected events: {other:?}"),
        }
        assert_eq!(alice.thread().len(), 0);
    }

    #[test]
    fn dropped_pending_message_logs_instead_of_completing() {
        let group = Group::new();
        let alice = group.add_member(MemberId::new("alice", true)).unwrap();
        let (_sub, events) = record_message_events(alice.thread());

        drop(group.begin_message(alice.id()).unwrap());

        let seen = events.lock().unwrap();
        assert!(matches!(
            &seen[..],
            [MsgEvent::Begin(_, _), MsgEvent::Log(_, _)]
        ));
        assert_eq!(alice.thread().len(), 0);
    }

    #[test]
    fn completing_pending_message_with_another_senders_message_is_rejected() {
        let group = Group::new();
        let alice = group.add_member(MemberId::new("alice", true)).unwrap();
        group.add_member(MemberId::new("bob", true)).unwrap();

        let pending = group.begin_message(alice.id()).unwrap();
        let result = pending.complete(user_msg("bob", "not mine"));

        assert!(matches!(result, Err(AppendMessageError::SenderMismatch(_))));
        assert_eq!(alice.thread().len(), 0);
    }

    #[test]
    fn group_append_emits_begin_then_full_for_sender() {
        let group = Group::new();
        let alice = group.add_member(MemberId::new("alice", true)).unwrap();
        let (_sub, events) = record_message_events(alice.thread());

        group.append(user_msg("alice", "hi")).unwrap();

        let seen = events.lock().unwrap();
        match &seen[..] {
            [MsgEvent::Begin(b, _), MsgEvent::Full(f, _)] => assert_eq!(b, f),
            other => panic!("unexpected events: {other:?}"),
        }
    }

    #[test]
    fn message_without_sender_emits_full_only() {
        let group = Group::new();
        let alice = group.add_member(MemberId::new("alice", true)).unwrap();
        let (_sub, events) = record_message_events(alice.thread());

        group.append(Msg::Platform("notice".into())).unwrap();

        let seen = events.lock().unwrap();
        assert!(matches!(&seen[..], [MsgEvent::Full(_, Msg::Platform(_))]));
    }
}
