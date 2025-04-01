use std::{ops::Deref, sync::Arc};

use crate::chat::DynChatModel;

use super::{Extension, UseExtensionError};



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

    pub fn chat_model(&self) -> Result<ChatModelRef, UseExtensionError> {
        for extension in &self.extensions {
            if let Some(model) = extension.chat_model() {
                return Ok(ChatModelRef {
                    extension: extension.as_ref(),
                    target: model,
                });
            }
        }
        Err(UseExtensionError::ChatModelNotAvailable)
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

impl<'a> Deref for ChatModelRef<'a> {
    type Target = DynChatModel<'a>;

    fn deref(&self) -> &Self::Target {
        self.target
    }
}