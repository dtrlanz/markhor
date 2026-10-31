use std::sync::Arc;

use async_trait::async_trait;
use markhor_core::{
    chat::ChatModel,
    dependencies::{ApiKey, Provide},
    extension::{Extension, InitExtensionError},
};

mod api;
mod chat;

pub use chat::ChatClient;

const EXTENSION_URI: &str = "https://github.com/dtrlanz/markhor/tree/main/extensions/src/gemini";
const DEFAULT_GOOGLE_API_BASE_URL: &str = "https://...";

#[derive(Debug, Provide)]
pub struct Client {
    // unique per client instance but shared across models
    #[provide(each)]
    inner: Arc<ClientInner>,
    #[provide(default)]
    model_list: Vec<String>,
}

#[derive(Debug, Provide)]
pub struct ClientInner {
    // unique per client instance
    #[provide(each)]
    api_key: ApiKey,
    // shared across client instances
    #[provide(default)]
    config: Arc<ClientConfig>,
    // consider having this `Provide`d by session to improve connection pooling
    #[provide(default)]
    http_client: Arc<reqwest::Client>,
}

#[async_trait]
impl Extension for Client {
    fn uri(&self) -> &str {
        EXTENSION_URI
    }

    fn name(&self) -> &str {
        "Google Client"
    }

    fn description(&self) -> &str {
        "Client for Google's AI models"
    }

    async fn initialize(&mut self) -> Result<(), InitExtensionError> {
        todo!("load model list")
    }

    fn chat_models(&self) -> Vec<Box<dyn ChatModel>> {
        self.model_list
            .iter()
            .map(|model_name| {
                Box::new(ChatClient {
                    inner: self.inner.clone(),
                    model_name: model_name.clone(),
                }) as Box<dyn ChatModel>
            })
            .collect()
    }
}

#[derive(Debug, Clone)]
pub struct ClientConfig {
    pub api_base_url: Option<String>,
}

impl Default for ClientConfig {
    fn default() -> Self {
        Self {
            api_base_url: Some(DEFAULT_GOOGLE_API_BASE_URL.to_string()),
        }
    }
}
