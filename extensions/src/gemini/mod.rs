use markhor_core::{chat::chat::ChatApi, embedding::Embedder, extension::Extension};

mod chat;
mod embed;
mod shared;
mod error;

const EXTENSION_URI: &str = "https://github.com/dtrlanz/markhor/tree/main/extensions/src/gemini";

pub use chat::GeminiChatClient;
pub use embed::GeminiEmbedder;
pub use error::GeminiError;

pub struct GeminiClientExtension {
    api_key: String,
    http_client: reqwest::Client,
}

impl GeminiClientExtension {
    pub fn new(api_key: String, http_client: reqwest::Client) -> Self {
        GeminiClientExtension { api_key, http_client }
    }
}

impl Extension for GeminiClientExtension {
    fn uri(&self) -> &str {
        EXTENSION_URI
    }

    fn name(&self) -> &str {
        "Gemini Client Extension"
    }

    fn description(&self) -> &str {
        "Provides a chat client for the Gemini API."
    }

    fn chat_model(&self) -> Option<std::sync::Arc<dyn ChatApi>> {
        // let client = GeminiChatClient::new(self.api_key.clone(), self.http_client.clone());
        // Some(std::sync::Arc::new(client))
        todo!()
    }

    fn embedding_model(&self) -> Option<std::sync::Arc<dyn Embedder>> {
        todo!()
    }
}

