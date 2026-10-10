use crate::{chat::{chat::ChatModel, prompter::Prompter}, chunking::Chunker, convert::Converter, dependencies::ResolveDependencyError, embedding::EmbeddingModel, permissions::Permission};

use std::{fmt::Display, ops::{Deref, DerefMut}};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use thiserror::Error;

mod comp;
mod extension_config;

pub use comp::Comp;
pub use extension_config::ExtensionConfig;

#[cfg(test)]
pub(crate) use comp::TryIntoComp;

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

    #[error("Extension lacks required permissions: {}", .0.iter().map(|p| p.name()).collect::<Vec<_>>().join(", "))]
    Permission(Vec<Permission>),
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

