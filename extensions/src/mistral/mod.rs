//! Mistral API client (OCR and Files so far; chat to follow).
//!
//! The client can be used on its own:
//!
//! ```no_run
//! # async fn demo() -> Result<(), markhor_extensions::mistral::Error> {
//! use markhor_core::dependencies::ApiKey;
//! use markhor_extensions::mistral::{Client, DocumentSource, OcrOptions};
//!
//! let client = Client::new(ApiKey::new("<key>".into(), "mistral".into(), "default".into()));
//! let source = DocumentSource::from_path("report.pdf").await?;
//! let output = client.ocr(&source, &OcrOptions::default()).await?;
//! println!("{}", output.markdown());
//! # Ok(())
//! # }
//! ```
//!
//! Integration with Markhor's extension system lives in the `extension` module. It is the only
//! part that depends on Markhor's component traits (beyond [`ApiKey`] and the `Provide` derive,
//! which all provider clients currently share).

use std::sync::Arc;

use markhor_core::dependencies::{ApiKey, Provide};
use reqwest::RequestBuilder;

// The wire types model fields that the public API doesn't expose (yet).
#[allow(dead_code)]
mod api;
mod error;
mod extension;
mod files;
mod ocr;

pub use error::Error;
pub use files::{FileVisibility, Files, UploadOptions, UploadedFile};
pub use ocr::{DocumentSource, OcrImage, OcrOptions, OcrOutput, OcrPage};

use api::DEFAULT_BASE_URL;

const DEFAULT_OCR_MODEL: &str = "mistral-ocr-latest";

#[derive(Debug, Clone, Provide)]
pub struct Client {
    // unique per client instance but shared across components
    #[provide(each)]
    inner: Arc<ClientInner>,
}

#[derive(Debug, Provide)]
pub struct ClientInner {
    // unique per client instance
    #[provide(each, filter = |k| k.provider_is("mistral"))]
    api_key: ApiKey,
    // shared across client instances
    #[provide(default)]
    config: Arc<ClientConfig>,
    #[provide(default)]
    http_client: Arc<reqwest::Client>,
}

impl Client {
    pub fn new(api_key: ApiKey) -> Self {
        Self::with_config(api_key, ClientConfig::default())
    }

    pub fn with_config(api_key: ApiKey, config: ClientConfig) -> Self {
        Self {
            inner: Arc::new(ClientInner {
                api_key,
                config: Arc::new(config),
                http_client: Arc::new(reqwest::Client::new()),
            }),
        }
    }

    /// Operations on files stored with Mistral (uploads, signed URLs, deletion).
    pub fn files(&self) -> Files<'_> {
        Files::new(&self.inner)
    }
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

    fn get(&self, path: &str) -> RequestBuilder {
        self.authorize(self.http_client.get(self.url(path)))
    }

    fn post(&self, path: &str) -> RequestBuilder {
        self.authorize(self.http_client.post(self.url(path)))
    }

    fn delete(&self, path: &str) -> RequestBuilder {
        self.authorize(self.http_client.delete(self.url(path)))
    }

    fn authorize(&self, request: RequestBuilder) -> RequestBuilder {
        request.bearer_auth(self.api_key.key())
    }
}

#[derive(Debug, Clone)]
pub struct ClientConfig {
    pub api_base_url: Option<String>,

    /// The model used for OCR unless [`OcrOptions::model`] overrides it.
    pub ocr_model: String,
}

impl Default for ClientConfig {
    fn default() -> Self {
        Self {
            api_base_url: Some(DEFAULT_BASE_URL.to_string()),
            ocr_model: DEFAULT_OCR_MODEL.to_string(),
        }
    }
}

#[cfg(test)]
pub(crate) fn test_client(server: &wiremock::MockServer) -> Client {
    Client::with_config(
        ApiKey::new("test-key".into(), "mistral".into(), "proj".into()),
        ClientConfig {
            api_base_url: Some(format!("{}/v1", server.uri())),
            ..Default::default()
        },
    )
}
