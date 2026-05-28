use crate::{chat::{chat::ChatModel, prompter::Prompter}, chunking::Chunker, convert::Converter, dependencies::ResolveDependencyError, embedding::EmbeddingModel};

use std::{fmt::Display, ops::{Deref, DerefMut}};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use thiserror::Error;

mod active_extension;
pub use active_extension::{ActiveExtension, ExtensionConfig};

mod comp;
pub use comp::Comp;

#[async_trait]
pub trait Extension: Send + Sync {
    fn uri(&self) -> &str;
    fn name(&self) -> &str;
    fn description(&self) -> &str;
    async fn initialize(&mut self) -> Result<(), InitExtensionError> { Ok(()) }

    fn chat_models(&self) -> Vec<Box<dyn ChatModel>> { vec![] }
    fn embedding_models(&self) -> Vec<Box<dyn EmbeddingModel>> { vec![] }
    fn chunkers(&self) -> Vec<Box<dyn Chunker>> { vec![] }
    fn converters(&self) -> Vec<Box<dyn Converter>> { vec![] }
    fn prompters(&self) -> Vec<Box<dyn Prompter>> { vec![] }
    fn tools(&self) -> Vec<Box<dyn crate::tool::Tool>> { vec![] }
}

#[derive(Debug, Error)]
pub enum InitExtensionError {
    #[error("Failed to initialize extension: {0}")]
    Dependency(#[from] ResolveDependencyError),
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

    #[error("Prompter not available in extension")]
    PrompterNotAvailable,

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
        format!("{} {}{}", sanitize_filename::sanitize(self.extension.uri()),
            self.functionality_type, suffix)
    }
}

impl<T: ?Sized> Deref for F11y<T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        &*self.trait_object
    }
}

impl<T: ?Sized> DerefMut for F11y<T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut *self.trait_object
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
            .map(|s| s.trim_matches('"').to_string())
            .map_err(|_| std::fmt::Error)?
            .fmt(f)
    }
}

