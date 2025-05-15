use std::{collections::HashMap, sync::{atomic::{AtomicUsize, Ordering}, Arc}};

use async_trait::async_trait;
use markhor_core::{chat::{chat::{ChatApi, ChatOptions, ChatResponse, ChatStream, ContentPart, Message, ModelInfo}, ChatError}, extension::Extension};


const SONNET_18: [&str; 14] = [
    "Shall I compare thee to a summer’s day?",
    "Thou art more lovely and more temperate.",
    "Rough winds do shake the darling buds of May,",
    "And summer’s lease hath all too short a date.",
    "Sometime too hot the eye of heaven shines,",
    "And often is his gold complexion dimmed;",
    "And every fair from fair sometime declines,",
    "By chance or nature’s changing course untrimmed.",
    "But thy eternal summer shall not fade,",
    "Nor lose possession of that fair thou ow’st,",
    "Nor shall Death brag thou wand’rest in his shade,",
    "When in eternal lines to time thou grow’st.",
    "So long as men can breathe or eyes can see,",
    "So long lives this, and this gives life to thee."];


struct ShakespeareChatModel {
    idx: AtomicUsize,
}

impl ShakespeareChatModel {
    fn new() -> Self {
        Self { idx: AtomicUsize::new(0) }
    }
}

#[async_trait]
impl ChatApi for ShakespeareChatModel {
    async fn list_models(&self) -> Result<Vec<ModelInfo>, ChatError> {
        Ok(vec![ModelInfo {
            id: "shakespeare".to_string(),
            description: Some("Chat with Shakespeare".to_string()),
            context_window: None,
            max_output_tokens: None,
        }])
    }

    async fn generate(&self, messages: &[Message], options: &ChatOptions) -> Result<ChatResponse, ChatError> {
        if self.idx.load(Ordering::SeqCst) >= SONNET_18.len() {
            return Err(ChatError::RateLimited);
        }
        let idx = self.idx.fetch_add(1, Ordering::SeqCst);
        Ok(ChatResponse {
            content: vec![ContentPart::Text(SONNET_18[idx].to_string())],
            tool_calls: vec![],
            usage: None,
            finish_reason: None,
            model_id: Some("shakespeare".to_string()),
        })
    }

    async fn generate_stream(&self, messages: &[Message], options: &ChatOptions) -> Result<ChatStream, ChatError> {
        unimplemented!()
    }
}


struct ShakespeareChatExtension;

impl ShakespeareChatExtension {
    fn new() -> Self {
        Self
    }
}

impl Extension for ShakespeareChatExtension {
    fn uri(&self) -> &str {
        "shakespeare"
    }
    fn name(&self) -> &str {
        "Shakespeare Chat"
    }
    fn description(&self) -> &str {
        "Chat with Shakespeare"
    }
    fn chat_model(&self) -> Option<Box<dyn ChatApi>> {
        Some(Box::new(ShakespeareChatModel::new()))
    }
}

#[tokio::test]
async fn shakespeare_chat_model() {
    let model = ShakespeareChatModel::new();
    let messages = vec![
        Message::user("Tell me a sonnet"),
        Message::assistant("Sure, here is one:"),
    ];
    let response = ChatApi::generate(&model,&messages, &Default::default()).await.unwrap();
    assert_eq!(response.content.join(""), "Shall I compare thee to a summer’s day?");
    let response = ChatApi::generate(&model, &messages, &Default::default()).await.unwrap();
    assert_eq!(response.content.join(""), "Thou art more lovely and more temperate.");
}
