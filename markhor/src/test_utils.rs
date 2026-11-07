//! Test doubles for chat components, registered through a real `Session` so
//! that `Comp<dyn ChatModel>` and `Comp<dyn ChatUi>` can be resolved as in
//! production. Nothing here talks to a live API.

use async_trait::async_trait;
use markhor_core::{
    chat::{
        ChatError, ChatModel, ChatOptions, ChatResponse, ChatUi, Msg, MsgEvent, ReasoningEvent,
        ToolCallEvent, WorkerEvent,
    },
    dependencies::Session,
    extension::Extension,
};

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
