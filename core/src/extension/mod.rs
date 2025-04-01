mod extension_set;

pub use extension_set::ExtensionSet;

pub trait Extension {
    fn uri(&self) -> &str;
    fn name(&self) -> &str;
    fn description(&self) -> &str;

    fn chat_model(&self) -> Option<&crate::chat::DynChatModel> { None }
    fn embedding_model(&self) -> Option<&dyn crate::embedding::EmbeddingModel> { None }
    fn chunker(&self) -> Option<&dyn crate::embedding::Chunker> { None }
    fn converter(&self) -> Option<&crate::convert::DynConverter> { None }
    fn tools(&self) -> Vec<&dyn crate::tool::Tool> { vec![] }
}

