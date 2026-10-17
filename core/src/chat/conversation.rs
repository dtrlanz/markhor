use crossbeam_skiplist::{SkipMap, map::Entry};
use dashmap::DashMap;
use serde_json::Value;
use smallvec::SmallVec;
use std::{
    fmt::Debug,
    sync::{
        Arc, Mutex, Weak,
        atomic::{AtomicUsize, Ordering},
    },
    time::SystemTime,
};

use crate::chat::chat::{ContentPart, ToolCallRequest};

#[derive(Debug)]
pub struct Conversation {
    history: Arc<List>,
    participants: DashMap<Arc<str>, Participant>,
}

impl Conversation {
    fn new() -> Self {
        Self {
            history: Arc::new(List::new()),
            participants: DashMap::new(),
        }
    }

    fn add_participant(&self, name: &str, is_human: bool) {
        let participant = Participant {
            id: name.into(),
            human: is_human,
            context: self.history.clone(),
        };
        self.participants.insert(name.into(), participant);
    }

    fn fork(&self) -> Self {
        Self {
            history: self.history.fork(),
            participants: self.participants.clone(),
        }
    }

    fn append(&self, message: Msg) {
        match &message {
            Msg::Platform(_) | Msg::Developer(_) => {
                // something cleverer here?
                // any constraints etc.?
                self.history.append(message);
            }
            Msg::User { sender, .. } | Msg::Agent { sender, .. } => {
                self.history.append(message);
            }
            Msg::Direct { recipients, .. }
            | Msg::ToolResult { recipients, .. }
            | Msg::Event { recipients, .. } => {
                for recp in recipients {
                    if let Some(ptcp) = self.participants.get(recp) {
                        // TODO: deduplicate where multiple agents share context
                        // Also, consider cloning less
                        ptcp.append(message.clone());
                    }
                }
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

struct List {
    items: SkipMap<usize, Msg>,
    len: AtomicUsize,
    previous: Option<(Arc<List>, usize)>,
    forks: Mutex<Vec<Weak<List>>>,
}

impl List {
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
        dbg!(&stack);
        // Start at beginning
        stack
            .into_iter()
            .rev()
            .flat_map(|(iter, stop)| iter.take_while(move |e| *e.key() < stop))
    }
}

impl Debug for List {
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
enum Msg {
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

#[derive(Debug, Clone)]
struct Participant {
    id: Arc<str>,
    human: bool,
    context: Arc<List>,
}

impl Participant {
    fn append(&self, message: Msg) {
        self.context.append(message);
    }
}

#[cfg(test)]
mod tests {
    use std::{fmt::Debug, time::Duration};

    use crate::tool::Tool;

    use super::*;

    #[derive(Debug)]
    struct ConvBuilder {
        conv: Conversation,
        time: Option<SystemTime>,
    }

    impl ConvBuilder {
        fn new() -> Self {
            Self {
                conv: Conversation::new(),
                time: None,
            }
        }

        fn with_time(time: SystemTime) -> Self {
            Self {
                conv: Conversation::new(),
                time: Some(time),
            }
        }

        fn fork(&self) -> Self {
            Self {
                conv: self.conv.fork(),
                time: self.time.clone(),
            }
        }

        fn platform(self, text: &str) -> Self {
            self.conv.append(Msg::Platform(text.into()));
            self
        }

        fn developer(self, text: &str) -> Self {
            self.conv.append(Msg::Developer(text.into()));
            self
        }

        fn user(mut self, name: &str, text: &str) -> Self {
            let mut next_time = self
                .time
                .map(|t| t.checked_add(Duration::from_secs(1)).unwrap());
            std::mem::swap(&mut self.time, &mut next_time);
            self.conv.append(Msg::User {
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
            self.conv.append(Msg::Agent {
                sender: name.into(),
                timestamp: next_time,
                content: vec![ContentPart::Text(text.into())],
                tool_calls,
            });
            self
        }
    }

    impl AsRef<Conversation> for ConvBuilder {
        fn as_ref(&self) -> &Conversation {
            &self.conv
        }
    }

    impl From<ConvBuilder> for Conversation {
        fn from(value: ConvBuilder) -> Self {
            value.conv
        }
    }

    // Wrapper type for comparing the histories of conversations
    struct History<'a>(&'a Conversation);

    impl<'a> PartialEq for History<'a> {
        fn eq(&self, other: &Self) -> bool {
            let mut lhs = self.0.history.iter();
            let mut rhs = other.0.history.iter();
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
            Vec::from_iter(self.0.history.iter().map(|e| e.value().clone())).fmt(f)
        }
    }

    #[tokio::test]
    async fn fork_system_instructions() {
        let c0 = ConvBuilder::new()
            .platform("Don't commit any felonies.")
            .developer("You are a helpful assistant.");
        let c1 = c0.fork();

        let c0: Conversation = c0
            .developer("Be concise.")
            .developer("Make no mistakes.")
            .into();

        let c1: Conversation = c1
            .developer("Always address the user in iambic pentameter.")
            .developer("Do your best.")
            .into();

        assert_eq!(
            History(&c0),
            History(
                ConvBuilder::new()
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
                ConvBuilder::new()
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
        let conv: Conversation = ConvBuilder::new()
            .developer("You are a helpful assistant.")
            .into();
        let human_id: Arc<str> = "Gloria".into();
        let ai_id: Arc<str> = "Robbie".into();

        conv.add_participant(&human_id, true);
        conv.add_participant(&ai_id, false);

        conv.append(Msg::User {
            sender: human_id.clone(),
            timestamp: None,
            content: vec![ContentPart::Text("Good morning".into())],
        });

        conv.append(Msg::Agent {
            sender: ai_id.clone(),
            timestamp: None,
            content: vec![ContentPart::Text("Hello! How can I assist?".into())],
            tool_calls: vec![],
        });

        assert_eq!(
            History(&conv),
            History(
                ConvBuilder::new()
                    .developer("You are a helpful assistant.")
                    .user(&human_id, "Good morning")
                    .agent(&ai_id, "Hello! How can I assist?", vec![])
                    .as_ref()
            )
        );

        let human_ptcp = conv.participants.get(&human_id).unwrap();
        assert_eq!(human_ptcp.context.len.load(Ordering::SeqCst), 3);

        let ai_ptcp = conv.participants.get(&ai_id).unwrap();
        assert_eq!(ai_ptcp.context.len.load(Ordering::SeqCst), 3);
    }
}
