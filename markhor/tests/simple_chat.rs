//! Full `Simple` chats driven by scripted doubles in place of a real UI and
//! model. Nothing here talks to a live API.

use std::{
    collections::VecDeque,
    sync::{Arc, Mutex},
    time::Duration,
};

use async_trait::async_trait;
use markhor::chat::Simple;
use markhor_core::{
    chat::{
        ChatError, ChatModel, ChatOptions, ChatResponse, ChatUi, Msg, MsgEvent, ReasoningEvent,
        ToolCallEvent, WorkerEvent, common::ContentPart,
    },
    dependencies::Session,
    extension::Extension,
};
use tokio::sync::watch;

const WAIT: Duration = Duration::from_secs(5);

/// Answers prompts from a script, counts prompts, and records every message
/// the chat shows it.
#[derive(Clone)]
struct ScriptedUi {
    answers: Arc<Mutex<VecDeque<String>>>,
    prompts: Arc<watch::Sender<usize>>,
    shown: Arc<Mutex<Vec<Msg>>>,
}

impl ScriptedUi {
    fn new(answers: &[&str]) -> Self {
        Self {
            answers: Arc::new(Mutex::new(answers.iter().map(|a| a.to_string()).collect())),
            prompts: Arc::new(watch::channel(0).0),
            shown: Default::default(),
        }
    }

    async fn wait_for_prompts(&self, count: usize) {
        let mut rx = self.prompts.subscribe();
        tokio::time::timeout(WAIT, rx.wait_for(|n| *n >= count))
            .await
            .expect("timed out waiting for prompts")
            .expect("prompt counter dropped");
    }

    fn shown(&self) -> Vec<Msg> {
        self.shown.lock().unwrap().clone()
    }
}

#[async_trait]
impl ChatUi for ScriptedUi {
    async fn prompt_with_commands(&self, _commands: Vec<String>) -> String {
        self.prompts.send_modify(|n| *n += 1);
        let next = self.answers.lock().unwrap().pop_front();
        match next {
            Some(answer) => answer,
            // Block rather than panic inside a spawned task, where the panic
            // would go unnoticed and the test would only time out.
            None => std::future::pending().await,
        }
    }

    async fn select(&self, _options: &Vec<String>) -> Option<usize> {
        unimplemented!("selection is not exercised by these tests")
    }

    fn handle_message(&self, event: &MsgEvent) {
        if let MsgEvent::Full(_, msg) = event {
            self.shown.lock().unwrap().push(msg.clone());
        }
    }

    fn handle_reasoning(&self, _event: &ReasoningEvent) {}

    fn handle_tool_call(&self, _event: &ToolCallEvent) {}

    fn handle_worker(&self, _event: &WorkerEvent) {}
}

/// Replies from a script (`Ok` for a reply, `Err` for a failure) and records
/// every request it receives.
#[derive(Clone)]
struct ScriptedModel {
    script: Arc<Mutex<VecDeque<Result<String, String>>>>,
    requests: Arc<Mutex<Vec<Vec<Msg>>>>,
}

impl ScriptedModel {
    fn new(script: &[Result<&str, &str>]) -> Self {
        let script = script
            .iter()
            .map(|r| r.map(String::from).map_err(String::from))
            .collect();
        Self {
            script: Arc::new(Mutex::new(script)),
            requests: Default::default(),
        }
    }

    fn requests(&self) -> Vec<Vec<Msg>> {
        self.requests.lock().unwrap().clone()
    }
}

#[async_trait]
impl ChatModel for ScriptedModel {
    fn model_name(&self) -> &str {
        "scripted"
    }

    async fn generate(
        &self,
        messages: &[Msg],
        _options: &ChatOptions,
    ) -> Result<ChatResponse, ChatError> {
        self.requests.lock().unwrap().push(messages.to_vec());
        let next = self.script.lock().unwrap().pop_front();
        match next {
            Some(Ok(text)) => Ok(ChatResponse {
                content: vec![ContentPart::Text(text)],
                tool_calls: vec![],
                usage: None,
                finish_reason: None,
                model_id: None,
            }),
            Some(Err(text)) => Err(ChatError::Authentication(text)),
            None => std::future::pending().await,
        }
    }
}

#[derive(Clone)]
struct ScriptedExtension {
    ui: ScriptedUi,
    model: ScriptedModel,
}

impl Extension for ScriptedExtension {
    fn uri(&self) -> &str {
        "markhor://test-simple-chat"
    }

    fn name(&self) -> &str {
        "Scripted simple chat"
    }

    fn description(&self) -> &str {
        "Scripted chat UI and model for integration tests"
    }

    fn chat_models(&self) -> Vec<Box<dyn ChatModel>> {
        vec![Box::new(self.model.clone())]
    }

    fn chat_uis(&self) -> Vec<Box<dyn ChatUi>> {
        vec![Box::new(self.ui.clone())]
    }
}

/// Starts a `Simple` chat that uses the given doubles.
async fn start_chat(ui: &ScriptedUi, model: &ScriptedModel) -> Simple {
    let mut session = Session::new();
    let ext = ScriptedExtension {
        ui: ui.clone(),
        model: model.clone(),
    };
    session
        .add_extension::<_, ScriptedExtension>(ext, Default::default())
        .await
        .unwrap();
    Simple::new(session).unwrap()
}

fn text_of(content: &[ContentPart]) -> String {
    content
        .iter()
        .filter_map(|part| match part {
            ContentPart::Text(text) => Some(text.as_str()),
            _ => None,
        })
        .collect()
}

/// The text of every user and agent message, in order.
fn transcript(messages: &[Msg]) -> String {
    messages
        .iter()
        .filter_map(|msg| match msg {
            Msg::User { content, .. } | Msg::Agent { content, .. } => Some(text_of(content)),
            _ => None,
        })
        .collect()
}

#[tokio::test]
async fn simple_chat_round_trip() {
    let ui = ScriptedUi::new(&["hello"]);
    let model = ScriptedModel::new(&[Ok("hi there")]);
    let _chat = start_chat(&ui, &model).await;

    ui.wait_for_prompts(2).await;

    assert!(transcript(&model.requests()[0]).contains("hello"));
    assert!(ui.shown().iter().any(|msg| match msg {
        Msg::Agent { content, .. } => text_of(content) == "hi there",
        _ => false,
    }));
}

#[tokio::test]
async fn simple_chat_two_exchanges() {
    let ui = ScriptedUi::new(&["hello", "again"]);
    let model = ScriptedModel::new(&[Ok("first"), Ok("second")]);
    let _chat = start_chat(&ui, &model).await;

    ui.wait_for_prompts(3).await;

    let sent = transcript(&model.requests()[1]);
    let positions: Vec<usize> = ["hello", "first", "again"]
        .iter()
        .map(|text| {
            sent.find(text)
                .unwrap_or_else(|| panic!("missing {text:?} in {sent:?}"))
        })
        .collect();
    assert!(
        positions.windows(2).all(|w| w[0] < w[1]),
        "transcript out of order: {sent:?}"
    );
}

#[tokio::test]
async fn simple_chat_recovers_from_model_error() {
    let ui = ScriptedUi::new(&["hello", "retry"]);
    let model = ScriptedModel::new(&[Err("boom"), Ok("ok")]);
    let _chat = start_chat(&ui, &model).await;

    ui.wait_for_prompts(3).await;

    assert!(ui.shown().iter().any(|msg| match msg {
        Msg::Event { content, .. } => content.to_string().contains("boom"),
        _ => false,
    }));
    assert_eq!(model.requests().len(), 2);
}
