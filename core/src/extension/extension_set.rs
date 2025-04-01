use std::{ops::Deref, process::ExitStatus, sync::Arc};

use crate::chat::{ChatError, Message, ChatModel, DynChatModel};

use super::Extension;



pub struct ExtensionSet {
    extensions: Vec<Arc<dyn Extension>>,
}

impl ExtensionSet {
    pub fn new() -> Self {
        Self { extensions: vec![] }
    }

    pub fn add(&mut self, extension: Arc<dyn Extension>) {
        self.extensions.push(extension);
    }

    pub fn chat_model(&self) -> Option<ChatModelRef> {
        for extension in &self.extensions {
            if let Some(model) = extension.chat_model() {
                return Some(ChatModelRef {
                    extension: extension.as_ref(),
                    target: model,
                });
            }
        }
        None
    }
}

impl<T: IntoIterator<Item = Arc<E>>, E: Extension + 'static> From<T> for ExtensionSet {
    fn from(extensions: T) -> Self {
        let mut vec: Vec<Arc<dyn Extension>> = Vec::<_>::new();
        for e in extensions {
            vec.push(e);
        };
        Self { extensions: vec }
    }
}

pub struct ChatModelRef<'a> {
    extension: &'a dyn Extension,
    target: &'a crate::chat::DynChatModel<'a>,
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

// impl<'a> ChatModel for ChatModelRef<'a> {
//     async fn generate(&self, messages: Vec<ChatMessage>) -> Result<String, ChatError> {
//         self.functionality.generate(messages).await
//     }
// }

impl<'a> Deref for ChatModelRef<'a> {
    type Target = DynChatModel<'a>;

    fn deref(&self) -> &Self::Target {
        self.target
    }
}