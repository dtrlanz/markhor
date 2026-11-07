//! Test doubles for chat components, registered through a real `Session` so
//! that `Comp<dyn ChatModel>` and `Comp<dyn ChatUi>` can be resolved as in
//! production. Nothing here talks to a live API.

use std::{
    collections::VecDeque,
    sync::{Arc, Mutex},
    time::Duration,
};

use async_trait::async_trait;
use markhor_core::{
    chat::{
        ChatError, ChatModel, ChatOptions, ChatResponse, ChatUi, Msg, MsgEvent, ReasoningEvent,
        ToolCallEvent, WorkerEvent,
    },
    dependencies::Session,
    extension::Extension,
};
use tokio::sync::watch;

const WAIT: Duration = Duration::from_secs(5);
const POLL: Duration = Duration::from_millis(5);

/// Polls `check` until it returns `Some`, panicking after a timeout.
pub(crate) async fn eventually<T>(mut check: impl FnMut() -> Option<T>) -> T {
    tokio::time::timeout(WAIT, async {
        loop {
            if let Some(value) = check() {
                return value;
            }
            tokio::time::sleep(POLL).await;
        }
    })
    .await
    .expect("condition not met before timeout")
}

#[derive(Clone, Default)]
pub(crate) struct MockExtension;

impl Extension for MockExtension {
    fn uri(&self) -> &str {
        "markhor://test-chat"
    }

    fn name(&self) -> &str {
        "Test chat"
    }

    fn description(&self) -> &str {
        "Chat model and chat UI doubles for unit tests"
    }

    fn chat_models(&self) -> Vec<Box<dyn ChatModel>> {
        vec![Box::new(MockChatModel)]
    }

    fn chat_uis(&self) -> Vec<Box<dyn ChatUi>> {
        vec![Box::new(MockChatUi)]
    }
}

/// Returns a session with [`MockExtension`] registered.
pub(crate) async fn session_with_mocks() -> Session {
    let mut session = Session::new();
    session
        .add_extension::<_, MockExtension>(MockExtension, Default::default())
        .await
        .unwrap();
    session
}

struct MockChatModel;

#[async_trait]
impl ChatModel for MockChatModel {
    fn model_name(&self) -> &str {
        "mock"
    }

    async fn generate(
        &self,
        _messages: &[Msg],
        _options: &ChatOptions,
    ) -> Result<ChatResponse, ChatError> {
        unimplemented!("generation is not exercised by these tests")
    }
}

struct MockChatUi;

#[async_trait]
impl ChatUi for MockChatUi {
    async fn prompt_with_commands(&self, _commands: Vec<String>) -> String {
        unimplemented!("prompting is not exercised by these tests")
    }

    async fn select(&self, _options: &Vec<String>) -> Option<usize> {
        unimplemented!("selection is not exercised by these tests")
    }

    fn handle_message(&self, _event: &MsgEvent) {}

    fn handle_reasoning(&self, _event: &ReasoningEvent) {}

    fn handle_tool_call(&self, _event: &ToolCallEvent) {}

    fn handle_worker(&self, _event: &WorkerEvent) {}
}

/// A `ChatUi` that answers prompts from a fixed script and records what it is
/// shown. Clones share state, so a test keeps one handle while the session
/// holds another.
#[derive(Clone)]
pub(crate) struct ScriptedChatUi {
    answers: Arc<Mutex<VecDeque<String>>>,
    prompts: Arc<watch::Sender<usize>>,
    messages: Arc<Mutex<Vec<Msg>>>,
}

impl ScriptedChatUi {
    pub(crate) fn new(answers: &[&str]) -> Self {
        Self {
            answers: Arc::new(Mutex::new(answers.iter().map(|a| a.to_string()).collect())),
            prompts: Arc::new(watch::channel(0).0),
            messages: Default::default(),
        }
    }

    pub(crate) fn prompt_count(&self) -> usize {
        *self.prompts.borrow()
    }

    pub(crate) async fn wait_for_prompts(&self, count: usize) {
        let mut rx = self.prompts.subscribe();
        tokio::time::timeout(WAIT, rx.wait_for(|n| *n >= count))
            .await
            .expect("timed out waiting for prompts")
            .expect("prompt counter dropped");
    }

    pub(crate) fn recorded_messages(&self) -> Vec<Msg> {
        self.messages.lock().unwrap().clone()
    }
}

#[async_trait]
impl ChatUi for ScriptedChatUi {
    async fn prompt_with_commands(&self, _commands: Vec<String>) -> String {
        self.prompts.send_modify(|n| *n += 1);
        let next = self.answers.lock().unwrap().pop_front();
        match next {
            Some(answer) => answer,
            // An exhausted script blocks rather than panicking inside the
            // spawned responder task, where a panic would go unnoticed.
            None => std::future::pending().await,
        }
    }

    async fn select(&self, _options: &Vec<String>) -> Option<usize> {
        unimplemented!("selection is not exercised by these tests")
    }

    fn handle_message(&self, event: &MsgEvent) {
        if let MsgEvent::Full(_, msg) = event {
            self.messages.lock().unwrap().push(msg.clone());
        }
    }

    fn handle_reasoning(&self, _event: &ReasoningEvent) {}

    fn handle_tool_call(&self, _event: &ToolCallEvent) {}

    fn handle_worker(&self, _event: &WorkerEvent) {}
}

#[derive(Clone)]
struct ScriptedExtension(ScriptedChatUi);

impl Extension for ScriptedExtension {
    fn uri(&self) -> &str {
        "markhor://test-scripted-chat"
    }

    fn name(&self) -> &str {
        "Scripted chat"
    }

    fn description(&self) -> &str {
        "Scripted chat UI for unit tests"
    }

    fn chat_uis(&self) -> Vec<Box<dyn ChatUi>> {
        vec![Box::new(self.0.clone())]
    }
}

/// Returns a session with `ui` registered as the chat UI.
pub(crate) async fn session_with_ui(ui: &ScriptedChatUi) -> Session {
    let mut session = Session::new();
    session
        .add_extension::<_, ScriptedExtension>(ScriptedExtension(ui.clone()), Default::default())
        .await
        .unwrap();
    session
}
