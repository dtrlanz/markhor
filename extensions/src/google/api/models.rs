use serde::{Deserialize, Serialize};

/// Response of `GET /models`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ListModelsResponse {
    #[serde(default)]
    pub models: Vec<Model>,
    /// Pass as `pageToken` to fetch the next page; absent on the last page.
    pub next_page_token: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Model {
    /// Resource name, e.g. `models/gemini-2.0-flash`.
    pub name: String,
    pub display_name: Option<String>,
    pub description: Option<String>,
    pub input_token_limit: Option<u32>,
    pub output_token_limit: Option<u32>,
    #[serde(default)]
    pub supported_generation_methods: Vec<GenerationMethod>,
}

impl Model {
    /// The model's identifier without the `models/` resource prefix.
    pub fn id(&self) -> &str {
        self.name.strip_prefix("models/").unwrap_or(&self.name)
    }

    pub fn supports(&self, method: &GenerationMethod) -> bool {
        self.supported_generation_methods.contains(method)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) enum GenerationMethod {
    GenerateContent,
    CountTokens,
    EmbedContent,
    BatchEmbedContents,
    #[serde(untagged)]
    Unknown(String),
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn list_models_deserializes() {
        let response: ListModelsResponse = serde_json::from_value(json!({
            "models": [
                {
                    "name": "models/gemini-2.0-flash",
                    "version": "2.0",
                    "displayName": "Gemini 2.0 Flash",
                    "inputTokenLimit": 1048576,
                    "outputTokenLimit": 8192,
                    "supportedGenerationMethods": ["generateContent", "countTokens", "bidiGenerateContent"],
                    "temperature": 1,
                },
                {
                    "name": "models/text-embedding-004",
                    "supportedGenerationMethods": ["embedContent"]
                }
            ],
            "nextPageToken": "tok"
        }))
        .unwrap();

        assert_eq!(response.next_page_token.as_deref(), Some("tok"));
        let chat_models: Vec<_> = response
            .models
            .iter()
            .filter(|m| m.supports(&GenerationMethod::GenerateContent))
            .map(Model::id)
            .collect();
        assert_eq!(chat_models, ["gemini-2.0-flash"]);
        assert!(
            response.models[0]
                .supported_generation_methods
                .contains(&GenerationMethod::Unknown("bidiGenerateContent".into()))
        );
    }
}
