mod extension_set;

pub use extension_set::ExtensionSet;
use thiserror::Error;

pub trait Extension: Send + Sync {
    fn uri(&self) -> &str;
    fn name(&self) -> &str;
    fn description(&self) -> &str;

    fn chat_model(&self) -> Option<&crate::chat::DynChatModel> { None }
    fn embedding_model(&self) -> Option<&dyn crate::embedding::EmbeddingModel> { None }
    fn chunker(&self) -> Option<&dyn crate::embedding::Chunker> { None }
    fn converter(&self) -> Option<&crate::convert::DynConverter> { None }
    fn tools(&self) -> Vec<&dyn crate::tool::Tool> { vec![] }
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