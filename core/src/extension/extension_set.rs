use std::sync::Arc;

use crate::chat::{ChatError, ChatMessage, ChatModel};

use super::Extension;



pub struct ExtensionSet {
    extensions: Vec<Arc<dyn Extension>>,
}

impl ExtensionSet {
    pub fn new(extensions: Vec<Arc<dyn Extension>>) -> Self {
        Self { extensions }
    }

    pub fn chat_model(&self) -> Option<ChatModelRef> {
        for extension in &self.extensions {
            if let Some(model) = extension.chat_model() {
                return Some(ChatModelRef {
                    extension: extension.as_ref(),
                    functionality: model,
                });
            }
        }
        None
    }
}

pub struct ChatModelRef<'a> {
    extension: &'a dyn Extension,
    functionality: &'a crate::chat::DynChatModel<'a>,
}

impl<'a> ChatModelRef<'a> {
    pub fn uri(&self) -> &str {
        self.extension.uri()
    }
    pub fn name(&self) -> &str {
        self.extension.name()
    }
    pub fn description(&self) -> &str {
        self.extension.description()
    }
}

impl<'a> ChatModel for ChatModelRef<'a> {
    async fn generate(&self, messages: Vec<ChatMessage>) -> Result<String, ChatError> {
        self.functionality.generate(messages).await
    }
}
