mod active_extension;

use std::{fmt::Display, ops::Deref};

pub use active_extension::{ActiveExtension, ExtensionConfig};

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::{chat::{chat::ChatApi, prompter::Prompter}, chunking::Chunker, convert::Converter, embedding::Embedder};

pub trait Extension: Send + Sync {
    fn uri(&self) -> &str;
    fn name(&self) -> &str;
    fn description(&self) -> &str;

    fn chat_model(&self) -> Option<Box<dyn ChatApi>> { None }
    fn embedding_model(&self) -> Option<Box<dyn Embedder>> { None }
    fn chunker(&self) -> Option<Box<dyn Chunker>> { None }
    fn converter(&self) -> Option<Box<dyn Converter>> { None }
    fn prompters(&self) -> Vec<Box<dyn Prompter>> { vec![] }
    fn tools(&self) -> Vec<Box<dyn crate::tool::Tool>> { vec![] }
}

#[derive(Debug, Error)]
pub enum UseExtensionError {
    #[error("Chat model not available in extension")]
    ChatModelNotAvailable,
    
    #[error("Embedding model not available in extension")]
    EmbeddingModelNotAvailable,
    
    #[error("Chunker not available in extension")]
    ChunkerNotAvailable,

    #[error("Converter not available in extension")]
    ConverterNotAvailable,

    #[error("Tool not available in extension")]
    ToolNotAvailable,
}

pub struct F11y<T: ?Sized> {
    trait_object: Box<T>,
    functionality_type: FunctionalityType,
    extension: ActiveExtension,
}

impl<T: ?Sized> F11y<T> {
    pub fn extension(&self) -> &ActiveExtension {
        &self.extension
    }

    pub fn functionality_type(&self) -> FunctionalityType {
        self.functionality_type
    }

    pub fn metadata_id(&self) -> String {
        let suffix = match self.functionality_type {
            _ => "",
        };
        format!("{} {}{}", self.extension.uri(), self.functionality_type, suffix)
    }
}

impl<T: ?Sized> Deref for F11y<T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        &*self.trait_object
    }
}

#[derive(Debug, Clone, Copy, Hash, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum FunctionalityType {
    ChatProvider,
    Embedder,
    Chunker,
    Converter,
    Prompter,
    Tool,
}

impl Display for FunctionalityType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        serde_json::to_string(self)
            .map_err(|_| std::fmt::Error)?
            .fmt(f)
    }
}

