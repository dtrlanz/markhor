use std::sync::Arc;

use markhor_core::{chat::chat::ChatApi, embedding::Embedder, extension::Extension};

mod chat;
mod embed;
mod shared;
mod error;

const EXTENSION_URI: &str = "https://github.com/dtrlanz/markhor/tree/main/extensions/src/gemini";

pub use chat::GeminiChatClient;
pub use embed::GeminiEmbedder;
pub use error::GeminiError;
use reqwest::Client;
use shared::{GeminiConfig, SharedGeminiClient};

pub struct GeminiClientExtension {
    shared_client: Arc<SharedGeminiClient>,
}

impl GeminiClientExtension {
    pub fn new(api_key: impl Into<String>) -> Result<Self, GeminiError> {
        Self::new_with_options(api_key, None, None)
    }

    pub fn new_with_options(
        api_key: impl Into<String>,
        api_base_url: Option<String>,
        client_override: Option<Client>,
    ) -> Result<Self, GeminiError> {
        let mut config = GeminiConfig::new(api_key)?;
        if let Some(base_url_str) = api_base_url {
            config = config.base_url(&base_url_str)?;
        }
        let shared_client = SharedGeminiClient::new(config, client_override)?;
        Ok(GeminiClientExtension {
            shared_client: Arc::new(shared_client),
        })
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

    fn chat_model(&self) -> Option<Arc<dyn ChatApi>> {
        let chat = GeminiChatClient::new_with_shared_client(
            self.shared_client.clone(), 
            None
        ).ok()?;
        let arc: Arc<dyn ChatApi> = Arc::new(chat);
        Some(arc)
    }

    fn embedding_model(&self) -> Option<std::sync::Arc<dyn Embedder>> {
        let embedder = GeminiEmbedder::new_with_shared_client(
            self.shared_client.clone(), 
            "embedding-001".into(),
            None,
        ).ok()?;
        let arc: Arc<dyn Embedder> = Arc::new(embedder);
        Some(arc)
    }
}

