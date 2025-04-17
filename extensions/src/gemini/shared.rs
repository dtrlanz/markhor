use reqwest::Client;
use secrecy::{ExposeSecret, SecretString}; // Using `secrecy` for the API key
use thiserror::Error;
use tracing::{debug, instrument, trace};
use url::Url; // Use the `url` crate for robust URL handling

use super::error::GeminiError;

// Re-use GeminiError definition from previous step
// use crate::gemini::error::GeminiError;
// Assume GeminiError is accessible

// Renaming the old default base URL constant for clarity
const DEFAULT_GEMINI_GENERATIVE_LANGUAGE_BASE_URL: &str = "https://generativelanguage.googleapis.com";

pub(crate) const EXTENSION_URI: &str = "todo";

/// Configuration for Gemini clients.
#[derive(Clone, Debug)]
pub struct GeminiConfig {
    /// Google AI API Key. Use SecretString for better security practices.
    pub(crate) api_key: SecretString,
    /// Base URL for the Google AI Generative Language API.
    pub(crate) base_url: Url,
    /// Timeout for HTTP requests. Defaults to 60 seconds.
    pub(crate) timeout: std::time::Duration,
    // Add other shared configurations like retry policies if needed later
}

impl GeminiConfig {
    /// Creates a new Gemini configuration.
    ///
    /// # Arguments
    /// * `api_key`: The Google AI API key.
    ///
    /// # Errors
    /// Returns `GeminiError::InvalidConfiguration` if the API key is empty or the
    /// default base URL fails to parse (which shouldn't happen).
    pub fn new(api_key: impl Into<String>) -> Result<Self, GeminiError> {
        let api_key = api_key.into();
        if api_key.is_empty() {
            return Err(GeminiError::InvalidConfiguration("API key cannot be empty".to_string()));
        }

        let base_url = Url::parse(DEFAULT_GEMINI_GENERATIVE_LANGUAGE_BASE_URL)
            .map_err(|e| GeminiError::InvalidConfiguration(
                format!("Internal error: Failed to parse default base URL: {}", e)
            ))?; // This should ideally never fail

        Ok(Self {
            api_key: api_key.into(),
            base_url,
            timeout: std::time::Duration::from_secs(60),
        })
    }

    /// Allows setting a custom base URL.
    #[must_use]
    pub fn base_url(mut self, url: &str) -> Result<Self, GeminiError> {
        self.base_url = Url::parse(url)
            .map_err(|e| GeminiError::InvalidConfiguration(
                format!("Invalid base URL '{}': {}", url, e)
            ))?;
        Ok(self)
    }

    /// Allows setting a custom request timeout.
    #[must_use]
    pub fn timeout(mut self, timeout: std::time::Duration) -> Self {
        self.timeout = timeout;
        self
    }
}

/// Shared component holding the HTTP client and configuration for Gemini API access.
#[derive(Clone, Debug)]
pub(crate) struct SharedGeminiClient {
    config: GeminiConfig,
    http_client: Client,
}

impl SharedGeminiClient {
    /// Creates a new SharedGeminiClient.
    /// Builds a default reqwest client if one is not provided.
    #[instrument(name = "shared_gemini_client_new", skip(config, client_override))]
    pub(crate) fn new(config: GeminiConfig, client_override: Option<Client>) -> Result<Self, GeminiError> {
        let client = match client_override {
            Some(client) => {
                debug!("Using provided HTTP client.");
                client
            },
            None => {
                debug!(timeout=?config.timeout, "Building default HTTP client.");
                Client::builder()
                    .timeout(config.timeout)
                    // Add other default client configurations (proxies, headers?) here if needed
                    .build()
                    .map_err(|e| GeminiError::InvalidConfiguration(
                        format!("Failed to build default HTTP client: {}", e)
                    ))?
            }
        };

        // Log base URL without API key
        debug!(base_url = %config.base_url, "Shared Gemini client initialized.");

        Ok(Self { config, http_client: client })
    }

    /// Provides access to the underlying HTTP client.
    pub(crate) fn http_client(&self) -> &Client {
        &self.http_client
    }

    /// Provides access to the configuration.
    pub(crate) fn config(&self) -> &GeminiConfig {
        &self.config
    }

    // fn build_generate_url(&self, model_id: &str) -> String {
    //     // Example: "https://generativelanguage.googleapis.com/v1beta/models/gemini-pro:generateContent?key=..."
    //     format!("{}/{}:generateContent?key={}", self.base_url, model_id, self.api_key)
    // }

    // fn build_list_models_url(&self) -> String {
    //     // Example: "https://generativelanguage.googleapis.com/v1beta/models?key=..."
    //     format!("{}?key={}", self.base_url, self.api_key)
    // }

    /// Builds a URL for a specific model action (e.g., generateContent, listModels).
    /// Ensures the API key is added correctly as a query parameter for this API style.
    /// (Will need adjustment if we standardize on header-based auth later).
    // pub(crate) fn build_url(&self, path_segments: &[&str], query: Option<&str>) -> Result<Url, GeminiError> {
    //     let mut url = self.config.base_url.clone();

    //     // Append path segments
    //     url.path_segments_mut()
    //         .map_err(|_| GeminiError::InvalidConfiguration("Base URL cannot be a 'cannot-be-a-base' URL.".to_string()))?
    //         .extend(["v1beta"])  // Prepend v1beta
    //         .extend(path_segments);

    //     // Append query string if provided
    //     if let Some(q) = query {
    //         url.set_query(Some(q));
    //     }

    //     // --- Crucially: Add API Key ---
    //     // The Gemini API (generativelanguage) often uses `?key=` query param
    //     // Vertex AI uses header/token auth. Let's stick to query param for now
    //     // based on original code. We can refactor this later if needed.
    //     url.query_pairs_mut()
    //         .append_pair("key", self.config.api_key.expose_secret());

    //     trace!(built_url = %url, "Built Gemini API URL");
    //     Ok(url)
    // }

    pub(crate) fn build_url(&self, relative_path: &str /* e.g., "models/gemini-pro:generateContent" */ ) -> Result<Url, GeminiError> {
        let base_path = format!("v1beta/{}", relative_path); // Prepend v1beta
        let mut url = self.config.base_url.clone();
    
        url.path_segments_mut()
            .map_err(|_| GeminiError::InvalidConfiguration("Base URL cannot be a 'cannot-be-a-base' URL.".to_string()))?
            .extend(base_path.split('/')); // Split and extend
    
        trace!(built_url = %url, "Built Gemini API URL (without auth)");
        Ok(url)
    }

    // Potential future helpers:
    // - pub(crate) async fn send_request(...) -> Result<reqwest::Response, GeminiError>
    // - pub(crate) fn add_auth_header(...) -> reqwest::RequestBuilder
}