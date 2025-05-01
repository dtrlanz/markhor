//mod extension_set;

use std::sync::Arc;

use serde::{Deserialize, Serialize};
//pub use extension_set::ExtensionSet;
use thiserror::Error;

use crate::{chat::chat::ChatApi, chunking::Chunker, convert::Converter, embedding::Embedder};

pub trait Extension: Send + Sync {
    fn uri(&self) -> &str;
    fn name(&self) -> &str;
    fn description(&self) -> &str;

    fn chat_model(&self) -> Option<Arc<dyn ChatApi>> { None }
    fn embedding_model(&self) -> Option<Arc<dyn Embedder>> { None }
    fn chunker(&self) -> Option<Arc<dyn Chunker>> { None }
    fn converter(&self) -> Option<Arc<dyn Converter>> { None }
    fn tools(&self) -> Vec<Arc<dyn crate::tool::Tool>> { vec![] }
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

/// Trait for identifying extension functionalities
/// 
/// An extension might have any number of functionalities. For example, a chat model extension
/// might offer several different models; an SDK wrapper might implement different kinds of 
/// functionalities (chat, embedding, conversion).
/// 
/// This trait helps identify and distinguish the various functinalities offered by extensions.
/// 
/// The combination of `extension_uri` and `id` must be unique.
pub trait Functionality {
    /// The unique URI of the extension that this functionality belongs to.
    fn extension_uri(&self) -> &str;

    /// Identifier that is unique among the extension's functionalities.
    fn id(&self) -> &str;

    /// A name that might help the user identify this functionality (e.g., name of chat model).
    /// 
    /// The default implementation returns `self.id()`.
    fn name(&self) -> &str {
        self.id()
    }
}


#[derive(Debug, Clone, Hash, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct FunctionalityId {
    uri: String,
    id: String,
}

impl<T: Functionality + ?Sized> From<&T> for FunctionalityId {
    fn from(f: &T) -> Self {
        FunctionalityId {
            uri: f.extension_uri().to_string(),
            id: f.id().to_string(),
        }
    }
}

impl Into<String> for FunctionalityId {
    fn into(self) -> String {
        // Space is an acceptable separator because it is not valid in URIs
        format!("{} {}", self.uri, self.id)
    }
}

impl TryFrom<String> for FunctionalityId {
    type Error = String;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        let parts: Vec<&str> = value.split(' ').collect();
        if parts.len() != 2 {
            return Err(value);
        }
        Ok(FunctionalityId {
            uri: parts[0].to_string(),
            id: parts[1].to_string(),
        })
    }
}