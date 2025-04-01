use std::sync::{atomic::{AtomicUsize, Ordering}, Arc};

use markhor_core::{chat::{ChatError, ChatModel, DynChatModel, Message}, extension::{Extension, ExtensionSet}};


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

impl ChatModel for ShakespeareChatModel {
    async fn generate(&self, _messages: &Vec<Message>) -> Result<String, ChatError> {
        if self.idx.load(Ordering::SeqCst) >= SONNET_18.len() {
            return Err(ChatError::ModelError(String::from("Out of lines")));
        }
        let idx = self.idx.fetch_add(1, Ordering::SeqCst);
        Ok(String::from(SONNET_18[idx]))
    }
}

struct ShakespeareChatExtension {
    model: ShakespeareChatModel,
}

impl ShakespeareChatExtension {
    fn new() -> Self {
        Self { model: ShakespeareChatModel::new() }
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
    fn chat_model(&self) -> Option<&DynChatModel> {
        Some(DynChatModel::from_ref(&self.model))
    }
}

#[tokio::test]
async fn shakespeare_chat_model() {
    let model = ShakespeareChatModel::new();
    let messages = vec![
        Message::user("Tell me a sonnet"),
        Message::assistant("Sure, here is one:"),
    ];
    let response = model.generate(&messages).await.unwrap();
    assert_eq!(response, "Shall I compare thee to a summer’s day?");
    let response = model.generate(&messages).await.unwrap();
    assert_eq!(response, "Thou art more lovely and more temperate.");
}

#[tokio::test]
async fn shakespeare_extension() {
    let extension = ShakespeareChatExtension::new();
    let model = extension.chat_model().unwrap();
    let messages = vec![
        Message::user("Tell me a sonnet"),
    ];
    let response = model.generate(&messages).await.unwrap();
    assert_eq!(response, "Shall I compare thee to a summer’s day?");
}

#[tokio::test]
async fn shakespeare_extension_set() {
    let extension = ShakespeareChatExtension::new();
    let extension_set = ExtensionSet::from(vec![Arc::new(extension)]);
    let model = extension_set.chat_model().unwrap();
    let messages = vec![
        Message::user("Tell me a sonnet"),
    ];
    let response = model.generate(&messages).await.unwrap();
    assert_eq!(response, "Shall I compare thee to a summer’s day?");
}
