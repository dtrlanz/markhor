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

use crate::chat::chat::{ContentPart, ToolCallRequest};

#[derive(Debug)]
pub struct Group {
    participants: Mutex<HashMap<Arc<str>, Arc<Thread>>>,
    primary: Arc<Thread>,
    secondary: Mutex<Vec<Arc<Thread>>>,
}

impl Group {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            participants: Default::default(),
            primary: Arc::new(Thread::new()),
            secondary: Default::default(),
        })
    }

    pub fn add_participant(
        self: &Arc<Self>,
        name: &str,
        human: bool,
    ) -> Result<Participant, AddParticipantError> {
        let mut inserted = false;
        let thread = self
            .participants
            .lock()
            .unwrap()
            .entry(name.into())
            .or_insert_with(|| {
                inserted = true;
                Arc::clone(&self.primary)
            })
            .clone();
        let ptcp = Participant {
            id: name.into(),
            human,
            group: Arc::clone(self),
            context: thread,
        };
        if inserted {
            Ok(ptcp)
        } else {
            Err(AddParticipantError::AlreadyExists(ptcp))
        }
    }

    fn append(&self, message: Msg) -> Result<&Self, AppendMessageError> {
        match &message {
            Msg::Platform(_) | Msg::Developer(_) => {
                self.primary.append(message);
                // // Or allow appending to secondary threads, too?
                // // I don't see a use case right now.
                // let mut threads = vec![self.primary.clone()];
                // {
                //     let mut guard = self.secondary.lock().unwrap();
                //     let mut idx = 0;
                //     while idx < guard.len() {
                //         if let Some(t) = Weak::upgrade(&guard[idx]) {
                //             threads.push(t);
                //             idx += 1;
                //         } else {
                //             guard.remove(idx);
                //         }
                //     }
                // }
                // for t in threads {
                //     t.append(message.clone());
                // }
                Ok(self)
            }
            Msg::User { sender, .. } | Msg::Agent { sender, .. } => {
                let ptcp = self
                    .participants
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
                    let guard = self.participants.lock().unwrap();
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
pub enum AddParticipantError {
    AlreadyExists(Participant),
}

#[derive(Debug)]
pub enum AppendMessageError {
    UnknownSender(String),
    UnknownRecipient(String),
}

#[derive(Debug, Clone)]
pub struct Participant {
    id: Arc<str>,
    human: bool,
    group: Arc<Group>,
    context: Arc<Thread>,
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

#[derive(Debug, Clone, PartialEq)]
pub enum Msg {
    Platform(String),
    Developer(String),
    User {
        sender: Arc<str>,
        timestamp: Option<SystemTime>,
        content: Vec<ContentPart>,
    },
    Agent {
        sender: Arc<str>,
        timestamp: Option<SystemTime>,
        content: Vec<ContentPart>,
        tool_calls: Vec<ToolCallRequest>,
    },
    Direct {
        sender: Arc<str>,
        timestamp: Option<SystemTime>,
        tool_call_id: String, // ?
        recipients: SmallVec<[Arc<str>; 1]>,
        content: Vec<ContentPart>,
    },
    ToolResult {
        recipients: SmallVec<[Arc<str>; 1]>,
        timestamp: Option<SystemTime>,
        tool_call_id: String,
        content: Vec<ContentPart>,
    },
    Event {
        recipients: SmallVec<[Arc<str>; 1]>,
        timestamp: Option<SystemTime>,
        source: String,
        content: Value,
    },
    ControlSignal {
        recipients: SmallVec<[Arc<str>; 1]>, // ?
        compaction_start: usize,
        content: String,
    },
}

struct Name {
    inner: Arc<(u8, str)>,
}

#[cfg(test)]
mod tests {
    use std::{fmt::Debug, time::Duration};

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
                sender: name.into(),
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
                sender: name.into(),
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
        let human = group.add_participant("Gloria", true).unwrap();
        let ai = group.add_participant("Robbie", false).unwrap();

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
