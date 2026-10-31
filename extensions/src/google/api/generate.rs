use markhor_core::chat::{self, ChatOptions, UsageInfo};
use serde::{Deserialize, Serialize};

use super::{ModelTurn, SystemInstruction, Turn, wire_name};

/// Body of `POST /models/{model}:generateContent`. The model name is part of the URL.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct GenerateContentRequest {
    pub contents: Vec<Turn>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub system_instruction: Option<SystemInstruction>,
    #[serde(skip_serializing_if = "GenerationConfig::is_empty")]
    pub generation_config: GenerationConfig,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct GenerationConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_output_tokens: Option<u32>,
}

impl GenerationConfig {
    pub fn is_empty(&self) -> bool {
        self.temperature.is_none() && self.max_output_tokens.is_none()
    }
}

impl From<&ChatOptions> for GenerationConfig {
    fn from(options: &ChatOptions) -> Self {
        Self {
            temperature: options.temperature,
            max_output_tokens: options.max_tokens,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct GenerateContentResponse {
    /// Empty only if the prompt was blocked (see `prompt_feedback`).
    #[serde(default)]
    pub candidates: Vec<Candidate>,
    pub prompt_feedback: Option<PromptFeedback>,
    pub usage_metadata: Option<UsageMetadata>,
    pub model_version: Option<String>,
    pub response_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Candidate {
    /// May be absent if generation was blocked.
    pub content: Option<ModelTurn>,
    pub finish_reason: Option<FinishReason>,
    pub finish_message: Option<String>,
    pub index: Option<u32>,
    #[serde(default)]
    pub safety_ratings: Vec<SafetyRating>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub(crate) enum FinishReason {
    FinishReasonUnspecified,
    Stop,
    MaxTokens,
    Safety,
    Recitation,
    Language,
    Other,
    Blocklist,
    ProhibitedContent,
    Spii,
    MalformedFunctionCall,
    ImageSafety,
    ImageProhibitedContent,
    ImageOther,
    NoImage,
    ImageRecitation,
    UnexpectedToolCall,
    TooManyToolCalls,
    MissingThoughtSignature,
    MalformedResponse,
    Escalation,
    #[serde(untagged)]
    Unknown(String),
}

impl FinishReason {
    /// Whether generation was stopped because of content policy.
    pub fn is_content_filter(&self) -> bool {
        matches!(
            self,
            FinishReason::Safety
                | FinishReason::Recitation
                | FinishReason::Blocklist
                | FinishReason::ProhibitedContent
                | FinishReason::Spii
                | FinishReason::ImageSafety
                | FinishReason::ImageProhibitedContent
                | FinishReason::ImageRecitation
        )
    }
}

impl From<FinishReason> for chat::FinishReason {
    fn from(reason: FinishReason) -> Self {
        match reason {
            FinishReason::FinishReasonUnspecified => chat::FinishReason::Unspecified,
            FinishReason::Stop => chat::FinishReason::Stop,
            FinishReason::MaxTokens => chat::FinishReason::Length,
            ref r if r.is_content_filter() => chat::FinishReason::ContentFilter,
            FinishReason::Unknown(name) => chat::FinishReason::Other(name),
            other => chat::FinishReason::Other(wire_name(&other)),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PromptFeedback {
    pub block_reason: Option<BlockReason>,
    #[serde(default)]
    pub safety_ratings: Vec<SafetyRating>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub(crate) enum BlockReason {
    BlockReasonUnspecified,
    Safety,
    Other,
    Blocklist,
    ProhibitedContent,
    ImageSafety,
    #[serde(untagged)]
    Unknown(String),
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SafetyRating {
    pub category: HarmCategory,
    pub probability: HarmProbability,
    #[serde(default)]
    pub blocked: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub(crate) enum HarmCategory {
    HarmCategoryUnspecified,
    HarmCategoryHarassment,
    HarmCategoryHateSpeech,
    HarmCategorySexuallyExplicit,
    HarmCategoryDangerousContent,
    HarmCategoryCivicIntegrity,
    #[serde(untagged)]
    Unknown(String),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub(crate) enum HarmProbability {
    HarmProbabilityUnspecified,
    Negligible,
    Low,
    Medium,
    High,
    #[serde(untagged)]
    Unknown(String),
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct UsageMetadata {
    pub prompt_token_count: Option<u32>,
    pub cached_content_token_count: Option<u32>,
    /// Excludes thinking tokens; see `thoughts_token_count`.
    pub candidates_token_count: Option<u32>,
    pub tool_use_prompt_token_count: Option<u32>,
    pub thoughts_token_count: Option<u32>,
    pub total_token_count: Option<u32>,
}

impl From<UsageMetadata> for UsageInfo {
    fn from(usage: UsageMetadata) -> Self {
        // Thinking tokens are billed as output, so count them as completion tokens.
        let completion_tokens = match (usage.candidates_token_count, usage.thoughts_token_count) {
            (None, None) => None,
            (a, b) => Some(a.unwrap_or(0) + b.unwrap_or(0)),
        };
        UsageInfo {
            prompt_tokens: usage.prompt_token_count,
            completion_tokens,
            total_tokens: usage.total_token_count,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::google::api::{ModelPartKind, UserPart, UserTurn};
    use serde_json::json;

    #[test]
    fn request_serializes_camel_case_and_omits_unset() {
        let request = GenerateContentRequest {
            contents: vec![UserTurn {
                parts: vec![UserPart::Text("hi".into())],
            }
            .into()],
            system_instruction: Some(SystemInstruction(vec!["sys".into()])),
            generation_config: GenerationConfig {
                temperature: Some(0.5),
                max_output_tokens: None,
            },
        };
        assert_eq!(
            serde_json::to_value(&request).unwrap(),
            json!({
                "contents": [{"role": "user", "parts": [{"text": "hi"}]}],
                "systemInstruction": {"parts": [{"text": "sys"}]},
                "generationConfig": {"temperature": 0.5},
            })
        );

        let minimal = GenerateContentRequest {
            contents: vec![],
            system_instruction: None,
            generation_config: GenerationConfig::default(),
        };
        assert_eq!(
            serde_json::to_value(&minimal).unwrap(),
            json!({"contents": []})
        );
    }

    #[test]
    fn typical_response_deserializes() {
        let response: GenerateContentResponse = serde_json::from_value(json!({
            "candidates": [{
                "content": {"parts": [{"text": "Hello!"}], "role": "model"},
                "finishReason": "STOP",
                "index": 0,
                "safetyRatings": [{
                    "category": "HARM_CATEGORY_HARASSMENT",
                    "probability": "NEGLIGIBLE"
                }]
            }],
            "usageMetadata": {
                "promptTokenCount": 4,
                "candidatesTokenCount": 2,
                "totalTokenCount": 6,
                "promptTokensDetails": [{"modality": "TEXT", "tokenCount": 4}]
            },
            "modelVersion": "gemini-2.0-flash",
            "responseId": "abc"
        }))
        .unwrap();

        let candidate = &response.candidates[0];
        assert_eq!(candidate.finish_reason, Some(FinishReason::Stop));
        assert_eq!(
            candidate.content.as_ref().unwrap().parts[0].kind,
            ModelPartKind::Text("Hello!".into())
        );
        assert_eq!(
            candidate.safety_ratings[0].category,
            HarmCategory::HarmCategoryHarassment
        );
        assert_eq!(response.model_version.as_deref(), Some("gemini-2.0-flash"));
        assert_eq!(
            UsageInfo::from(response.usage_metadata.unwrap()),
            UsageInfo {
                prompt_tokens: Some(4),
                completion_tokens: Some(2),
                total_tokens: Some(6)
            }
        );
    }

    #[test]
    fn blocked_prompt_deserializes() {
        let response: GenerateContentResponse = serde_json::from_value(json!({
            "promptFeedback": {
                "blockReason": "PROHIBITED_CONTENT",
                "safetyRatings": []
            },
            "usageMetadata": {"promptTokenCount": 3, "totalTokenCount": 3}
        }))
        .unwrap();
        assert!(response.candidates.is_empty());
        assert_eq!(
            response.prompt_feedback.unwrap().block_reason,
            Some(BlockReason::ProhibitedContent)
        );
    }

    #[test]
    fn candidate_without_content_deserializes() {
        let candidate: Candidate = serde_json::from_value(json!({
            "finishReason": "SAFETY",
            "index": 0
        }))
        .unwrap();
        assert!(candidate.content.is_none());
        assert!(candidate.finish_reason.unwrap().is_content_filter());
    }

    #[test]
    fn unknown_enum_values_are_preserved() {
        let reason: FinishReason = serde_json::from_value(json!("BRAND_NEW_REASON")).unwrap();
        assert_eq!(reason, FinishReason::Unknown("BRAND_NEW_REASON".into()));
        assert_eq!(
            chat::FinishReason::from(reason),
            chat::FinishReason::Other("BRAND_NEW_REASON".into())
        );

        let category: HarmCategory = serde_json::from_value(json!("HARM_CATEGORY_NEW")).unwrap();
        assert_eq!(category, HarmCategory::Unknown("HARM_CATEGORY_NEW".into()));
    }

    #[test]
    fn finish_reason_maps_to_core() {
        use chat::FinishReason as Core;
        assert_eq!(Core::from(FinishReason::Stop), Core::Stop);
        assert_eq!(Core::from(FinishReason::MaxTokens), Core::Length);
        assert_eq!(Core::from(FinishReason::Recitation), Core::ContentFilter);
        assert_eq!(
            Core::from(FinishReason::MalformedFunctionCall),
            Core::Other("MALFORMED_FUNCTION_CALL".into())
        );
        assert_eq!(
            Core::from(FinishReason::FinishReasonUnspecified),
            Core::Unspecified
        );
    }

    #[test]
    fn usage_includes_thinking_tokens_in_completion() {
        let usage: UsageMetadata = serde_json::from_value(json!({
            "promptTokenCount": 10,
            "candidatesTokenCount": 5,
            "thoughtsTokenCount": 20,
            "totalTokenCount": 35
        }))
        .unwrap();
        assert_eq!(UsageInfo::from(usage).completion_tokens, Some(25));
        assert_eq!(
            UsageInfo::from(UsageMetadata::default()),
            UsageInfo::default()
        );
    }

    #[test]
    fn generation_config_from_chat_options() {
        let config = GenerationConfig::from(&ChatOptions {
            temperature: Some(1.0),
            max_tokens: Some(100),
        });
        assert_eq!(
            serde_json::to_value(&config).unwrap(),
            json!({"temperature": 1.0, "maxOutputTokens": 100})
        );
        assert!(GenerationConfig::from(&ChatOptions::default()).is_empty());
    }
}
