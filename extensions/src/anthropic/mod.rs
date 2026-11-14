//! Anthropic (Claude) extension.
//!
//! Supports non-streaming text chat. Tool calls are not supported yet.

use std::sync::Arc;

use async_trait::async_trait;
use markhor_core::{
    chat::ChatModel,
    dependencies::{ApiKey, Provide},
    extension::{Extension, InitExtensionError},
};

// Parts are unused until tool support lands.
#[allow(dead_code, unused_imports)]
mod api;
mod chat;

pub use chat::ChatClient;

use api::{
    API_KEY_HEADER, API_VERSION, DEFAULT_BASE_URL, ErrorResponse, ListModelsResponse,
    VERSION_HEADER,
};

const EXTENSION_URI: &str = "https://github.com/dtrlanz/markhor/tree/main/extensions/src/anthropic";
const MODELS_PAGE_SIZE: u32 = 1000;

/// Used when `ChatOptions::max_tokens` is unset, since the API requires a limit.
const DEFAULT_MAX_TOKENS: u32 = 4096;

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
    #[provide(each, filter = |k| k.provider_is("anthropic"))]
    api_key: ApiKey,
    // shared across client instances
    #[provide(default)]
    config: Arc<ClientConfig>,
    #[provide(default)]
    http_client: Arc<reqwest::Client>,
}

impl ClientInner {
    fn url(&self, path: &str) -> String {
        let base = self
            .config
            .api_base_url
            .as_deref()
            .unwrap_or(DEFAULT_BASE_URL);
        format!("{}/{}", base.trim_end_matches('/'), path)
    }

    fn get(&self, path: &str) -> reqwest::RequestBuilder {
        self.authorize(self.http_client.get(self.url(path)))
    }

    fn post(&self, path: &str) -> reqwest::RequestBuilder {
        self.authorize(self.http_client.post(self.url(path)))
    }

    fn authorize(&self, request: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        request
            .header(API_KEY_HEADER, self.api_key.key())
            .header(VERSION_HEADER, API_VERSION)
    }

    /// Lists the ids of all models, newest first.
    async fn list_chat_models(&self) -> Result<Vec<String>, ListModelsError> {
        let mut ids = Vec::new();
        let mut after_id: Option<String> = None;
        loop {
            let mut request = self
                .get("models")
                .query(&[("limit", MODELS_PAGE_SIZE.to_string())]);
            if let Some(cursor) = &after_id {
                request = request.query(&[("after_id", cursor)]);
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
            ids.extend(page.data.into_iter().map(|m| m.id));
            match page.last_id {
                Some(cursor) if page.has_more => after_id = Some(cursor),
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
        "Anthropic Client"
    }

    fn description(&self) -> &str {
        "Client for Anthropic's Claude models"
    }

    async fn initialize(&mut self) -> Result<(), InitExtensionError> {
        // `InitExtensionError` has no variant for network/API failures. Until it does, a
        // failed listing is logged and leaves the extension without chat models.
        match self.inner.list_chat_models().await {
            Ok(models) => self.model_list = self.inner.config.select_models(models),
            Err(error) => tracing::warn!(%error, "failed to list Anthropic chat models"),
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

    /// The `max_tokens` sent when the caller doesn't set one (the API requires it).
    pub default_max_tokens: u32,

    /// If non-empty, only models matching one of these patterns are offered.
    ///
    /// Patterns are exact ids or ids with a single leading or trailing `*`
    /// (e.g., `claude-haiku-*`).
    pub allow_models: Vec<String>,

    /// Models matching any of these patterns are never offered, even if allowed.
    pub deny_models: Vec<String>,
}

impl Default for ClientConfig {
    fn default() -> Self {
        Self {
            api_base_url: Some(DEFAULT_BASE_URL.to_string()),
            default_max_tokens: DEFAULT_MAX_TOKENS,
            allow_models: vec![],
            deny_models: vec![],
        }
    }
}

impl ClientConfig {
    /// Applies the allow/deny lists, preserving the API's order (newest first).
    fn select_models(&self, mut ids: Vec<String>) -> Vec<String> {
        ids.retain(|id| {
            (self.allow_models.is_empty() || self.allow_models.iter().any(|p| model_matches(p, id)))
                && !self.deny_models.iter().any(|p| model_matches(p, id))
        });
        ids
    }
}

/// Matches a model id against a pattern: either an exact id, or an id with a single leading
/// or trailing `*` wildcard (e.g., `claude-haiku-*` or `*-latest`).
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
    use markhor_core::dependencies::ApiKey;
    use serde_json::json;
    use wiremock::{
        Mock, MockServer, ResponseTemplate,
        matchers::{header, method, path, query_param, query_param_is_missing},
    };

    use super::*;

    fn ids(names: &[&str]) -> Vec<String> {
        names.iter().map(|s| s.to_string()).collect()
    }

    fn inner_for(server: &MockServer) -> Arc<ClientInner> {
        Arc::new(ClientInner {
            api_key: ApiKey::new("test-key".into(), "anthropic".into(), "proj".into()),
            config: Arc::new(ClientConfig {
                api_base_url: Some(format!("{}/v1/", server.uri())),
                ..Default::default()
            }),
            http_client: Arc::new(reqwest::Client::new()),
        })
    }

    #[test]
    fn patterns() {
        assert!(model_matches("claude-haiku-4-5", "claude-haiku-4-5"));
        assert!(!model_matches("claude-haiku", "claude-haiku-4-5"));
        assert!(model_matches("claude-haiku-*", "claude-haiku-4-5"));
        assert!(model_matches("*-latest", "claude-haiku-latest"));
        assert!(!model_matches("*-latest", "claude-haiku-4-5"));
    }

    #[test]
    fn default_keeps_everything_in_api_order() {
        let all = ids(&["claude-opus-4-1", "claude-haiku-4-5", "claude-3-haiku"]);
        assert_eq!(ClientConfig::default().select_models(all.clone()), all);
    }

    #[test]
    fn allow_list_restricts() {
        let config = ClientConfig {
            allow_models: vec!["claude-haiku-*".into()],
            ..Default::default()
        };
        let selected = config.select_models(ids(&["claude-opus-4-1", "claude-haiku-4-5"]));
        assert_eq!(selected, ["claude-haiku-4-5"]);
    }

    #[test]
    fn deny_list_wins_over_allow_list() {
        let config = ClientConfig {
            allow_models: vec!["claude-*".into()],
            deny_models: vec!["claude-3-*".into(), "*-opus-4-1".into()],
            ..Default::default()
        };
        let selected = config.select_models(ids(&[
            "claude-3-haiku",
            "claude-opus-4-1",
            "claude-haiku-4-5",
        ]));
        assert_eq!(selected, ["claude-haiku-4-5"]);
    }

    fn model(id: &str) -> serde_json::Value {
        json!({"type": "model", "id": id, "display_name": id, "created_at": "2025-01-01T00:00:00Z"})
    }

    #[tokio::test]
    async fn listing_follows_pagination_and_sends_required_headers() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/v1/models"))
            .and(header("x-api-key", "test-key"))
            .and(header("anthropic-version", "2023-06-01"))
            .and(query_param("limit", "1000"))
            .and(query_param_is_missing("after_id"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "data": [model("claude-a"), model("claude-b")],
                "has_more": true, "first_id": "claude-a", "last_id": "claude-b",
            })))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/v1/models"))
            .and(query_param("after_id", "claude-b"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "data": [model("claude-c")],
                "has_more": false, "first_id": "claude-c", "last_id": "claude-c",
            })))
            .expect(1)
            .mount(&server)
            .await;

        let listed = inner_for(&server).list_chat_models().await.unwrap();
        assert_eq!(listed, ["claude-a", "claude-b", "claude-c"]);
    }

    #[tokio::test]
    async fn initialize_applies_filters() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/v1/models"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "data": [model("claude-opus-4-1"), model("claude-haiku-4-5")],
                "has_more": false, "first_id": "claude-opus-4-1", "last_id": "claude-haiku-4-5",
            })))
            .mount(&server)
            .await;

        let inner = Arc::new(ClientInner {
            api_key: ApiKey::new("test-key".into(), "anthropic".into(), "proj".into()),
            config: Arc::new(ClientConfig {
                api_base_url: Some(server.uri() + "/v1"),
                allow_models: vec!["claude-haiku-*".into()],
                ..Default::default()
            }),
            http_client: Arc::new(reqwest::Client::new()),
        });
        let mut client = Client {
            inner,
            model_list: vec![],
        };
        client.initialize().await.unwrap();

        let names: Vec<_> = client
            .chat_models()
            .iter()
            .map(|m| m.model_name().to_string())
            .collect();
        assert_eq!(names, ["claude-haiku-4-5"]);
    }

    #[tokio::test]
    async fn listing_failure_leaves_no_models() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/v1/models"))
            .respond_with(ResponseTemplate::new(401).set_body_json(json!({
                "type": "error",
                "error": {"type": "authentication_error", "message": "invalid x-api-key"},
                "request_id": "req_1",
            })))
            .mount(&server)
            .await;

        let mut client = Client {
            inner: inner_for(&server),
            model_list: vec![],
        };
        client.initialize().await.unwrap();
        assert!(client.chat_models().is_empty());

        let error = client.inner.list_chat_models().await.unwrap_err();
        assert!(error.to_string().contains("invalid x-api-key"), "{error}");
    }
}
