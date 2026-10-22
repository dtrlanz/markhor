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
    member::{Member, MemberId},
};

#[derive(Debug)]
pub struct Group {
    pub(super) members: Mutex<HashMap<MemberId, Arc<Thread>>>,
    pub(super) primary: Arc<Thread>,
    pub(super) secondary: Mutex<Vec<Arc<Thread>>>,
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

#[derive(Debug)]
pub enum InitContext {
    None,
    Fork,
}

pub(super) struct Thread {
    items: SkipMap<usize, Msg>,
    len: AtomicUsize,
    previous: Option<(Arc<Thread>, usize)>,
    forks: Mutex<Vec<Weak<Thread>>>,
}

impl Thread {
    pub(super) fn new() -> Self {
        Self {
            items: SkipMap::new(),
            len: AtomicUsize::new(0),
            previous: None,
            forks: Mutex::new(Vec::new()),
        }
    }

    pub(super) fn fork(self: &Arc<Self>) -> Arc<Self> {
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

    pub(super) fn iter(&'_ self) -> impl Iterator<Item = Entry<'_, usize, Msg>> + '_ {
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
