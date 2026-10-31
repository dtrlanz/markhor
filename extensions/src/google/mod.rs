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
            Ok(models) => self.model_list = self.inner.config.select_models(models),
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

    /// If non-empty, only models matching one of these patterns are offered.
    ///
    /// The API lists models that can't actually be used (e.g., retired ones that return 404),
    /// so this is the way to restrict the list to known-good models. Patterns are exact ids or
    /// ids with a single leading or trailing `*` (e.g., `gemma-4*`).
    pub allow_models: Vec<String>,

    /// Models matching any of these patterns are never offered, even if allowed.
    pub deny_models: Vec<String>,
}

impl Default for ClientConfig {
    fn default() -> Self {
        Self {
            api_base_url: Some(DEFAULT_GOOGLE_API_BASE_URL.to_string()),
            allow_models: vec![],
            deny_models: vec![],
        }
    }
}

impl ClientConfig {
    /// Applies the allow/deny lists and orders the result so that the best default comes first.
    ///
    /// `*-latest` aliases come first, since Google keeps them pointed at current models.
    /// Otherwise the API's order is preserved.
    fn select_models(&self, mut ids: Vec<String>) -> Vec<String> {
        ids.retain(|id| {
            (self.allow_models.is_empty() || self.allow_models.iter().any(|p| model_matches(p, id)))
                && !self.deny_models.iter().any(|p| model_matches(p, id))
        });
        ids.sort_by_key(|id| !id.ends_with("-latest"));
        ids
    }
}

/// Matches a model id against a pattern: either an exact id, or an id with a single leading
/// or trailing `*` wildcard (e.g., `gemini-2.5-*` or `*-tts`).
fn model_matches(pattern: &str, id: &str) -> bool {
    if let Some(suffix) = pattern.strip_prefix('*') {
        id.ends_with(suffix)
    } else if let Some(prefix) = pattern.strip_suffix('*') {
        id.starts_with(prefix)
    } else {
        pattern == id
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ids(names: &[&str]) -> Vec<String> {
        names.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn patterns() {
        assert!(model_matches("gemma-4-31b-it", "gemma-4-31b-it"));
        assert!(!model_matches("gemma-4", "gemma-4-31b-it"));
        assert!(model_matches("gemma-4*", "gemma-4-31b-it"));
        assert!(model_matches("*-tts", "gemini-2.5-flash-preview-tts"));
        assert!(!model_matches("*-tts", "gemini-2.5-flash"));
    }

    #[test]
    fn default_keeps_everything_with_latest_aliases_first() {
        let selected = ClientConfig::default().select_models(ids(&[
            "gemini-2.5-flash",
            "gemini-flash-latest",
            "gemma-4-31b-it",
            "gemini-pro-latest",
        ]));
        assert_eq!(
            selected,
            ["gemini-flash-latest", "gemini-pro-latest", "gemini-2.5-flash", "gemma-4-31b-it"]
        );
    }

    #[test]
    fn allow_list_restricts() {
        let config = ClientConfig {
            allow_models: vec!["gemma-4*".into()],
            ..Default::default()
        };
        let selected =
            config.select_models(ids(&["gemini-2.5-flash", "gemma-4-31b-it", "gemma-4-26b-a4b-it"]));
        assert_eq!(selected, ["gemma-4-31b-it", "gemma-4-26b-a4b-it"]);
    }

    #[test]
    fn deny_list_wins_over_allow_list() {
        let config = ClientConfig {
            allow_models: vec!["gemini-*".into()],
            deny_models: vec!["gemini-2.5-*".into(), "*-tts".into()],
            ..Default::default()
        };
        let selected = config.select_models(ids(&[
            "gemini-2.5-flash",
            "gemini-3-flash-preview",
            "gemini-3-flash-tts",
            "gemma-4-31b-it",
        ]));
        assert_eq!(selected, ["gemini-3-flash-preview"]);
    }
}
