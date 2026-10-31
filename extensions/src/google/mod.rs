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

use api::{ErrorResponse, GenerationMethod, ListModelsResponse};

const EXTENSION_URI: &str = "https://github.com/dtrlanz/markhor/tree/main/extensions/src/google";
const DEFAULT_GOOGLE_API_BASE_URL: &str = "https://generativelanguage.googleapis.com/v1beta";
const MODELS_PAGE_SIZE: u32 = 1000;
const API_KEY_HEADER: &str = "x-goog-api-key";

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
    #[provide(each, filter = |k| k.provider_is("google"))]
    api_key: ApiKey,
    // shared across client instances
    #[provide(default)]
    config: Arc<ClientConfig>,
    // consider having this `Provide`d by session to improve connection pooling
    #[provide(default)]
    http_client: Arc<reqwest::Client>,
}

impl ClientInner {
    fn url(&self, path: &str) -> String {
        let base = self
            .config
            .api_base_url
            .as_deref()
            .unwrap_or(DEFAULT_GOOGLE_API_BASE_URL);
        format!("{}/{}", base.trim_end_matches('/'), path)
    }

    /// Lists the ids of all models that support `generateContent`.
    async fn list_chat_models(&self) -> Result<Vec<String>, ListModelsError> {
        let mut ids = Vec::new();
        let mut page_token: Option<String> = None;
        loop {
            let mut request = self
                .http_client
                .get(self.url("models"))
                .header(API_KEY_HEADER, self.api_key.key())
                .query(&[("pageSize", MODELS_PAGE_SIZE.to_string())]);
            if let Some(token) = &page_token {
                request = request.query(&[("pageToken", token)]);
            }

            let response = request.send().await?;
            let status = response.status();
            let body = response.bytes().await?;
            if !status.is_success() {
                let message = match serde_json::from_slice::<ErrorResponse>(&body) {
                    Ok(err) => err.error.message,
                    Err(_) => String::from_utf8_lossy(&body).into_owned(),
                };
                return Err(ListModelsError::Api { status, message });
            }

            let page: ListModelsResponse = serde_json::from_slice(&body)?;
            ids.extend(
                page.models
                    .iter()
                    .filter(|m| m.supports(&GenerationMethod::GenerateContent))
                    .map(|m| m.id().to_string()),
            );
            match page.next_page_token {
                Some(token) if !token.is_empty() => page_token = Some(token),
                _ => return Ok(ids),
            }
        }
    }
}

#[derive(Debug, thiserror::Error)]
enum ListModelsError {
    #[error("request failed: {0}")]
    Network(#[from] reqwest::Error),
    #[error("invalid response: {0}")]
    Parse(#[from] serde_json::Error),
    #[error("{status}: {message}")]
    Api {
        status: reqwest::StatusCode,
        message: String,
    },
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
        // `InitExtensionError` has no variant for network/API failures. Until it does, a
        // failed listing is logged and leaves the extension without chat models.
        match self.inner.list_chat_models().await {
            Ok(models) => self.model_list = models,
            Err(error) => tracing::warn!(%error, "failed to list Google chat models"),
        }
        Ok(())
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
