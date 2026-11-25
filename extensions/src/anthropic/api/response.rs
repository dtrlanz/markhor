use markhor_core::chat::{FinishReason, UsageInfo};
use serde::{Deserialize, Serialize};

use super::message::AssistantBlock;

/// Body of a successful `POST /v1/messages` response (non-streaming).
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub(crate) struct MessagesResponse {
    pub id: String,
    /// The model that handled the request.
    pub model: String,
    /// Always an assistant message; send it back as history via [`Message::Assistant`].
    ///
    /// [`Message::Assistant`]: super::message::Message::Assistant
    pub content: Vec<AssistantBlock>,
    /// `None` only for streaming responses that haven't finished.
    pub stop_reason: Option<StopReason>,
    /// The stop sequence that ended generation, if `stop_reason` is `StopSequence`.
    #[serde(default)]
    pub stop_sequence: Option<String>,
    pub usage: Usage,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum StopReason {
    /// The model reached a natural stopping point.
    EndTurn,
    MaxTokens,
    StopSequence,
    /// The model wants the application to run one or more tools.
    ToolUse,
    /// A server-tool loop hit its iteration cap; resend the assistant content to continue.
    PauseTurn,
    Refusal,
    ModelContextWindowExceeded,
    #[serde(untagged)]
    Unknown(String),
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Deserialize)]
pub(crate) struct Usage {
    /// Input tokens that were not served from or written to the cache.
    #[serde(default)]
    pub input_tokens: u32,
    #[serde(default)]
    pub output_tokens: u32,
    #[serde(default)]
    pub cache_creation_input_tokens: Option<u32>,
    #[serde(default)]
    pub cache_read_input_tokens: Option<u32>,
    #[serde(default)]
    pub service_tier: Option<ResponseServiceTier>,
}

impl From<StopReason> for FinishReason {
    fn from(reason: StopReason) -> Self {
        match reason {
            StopReason::EndTurn | StopReason::StopSequence => FinishReason::Stop,
            StopReason::MaxTokens | StopReason::ModelContextWindowExceeded => FinishReason::Length,
            StopReason::ToolUse => FinishReason::ToolCalls,
            StopReason::Refusal => FinishReason::ContentFilter,
            StopReason::PauseTurn => FinishReason::Other("pause_turn".into()),
            StopReason::Unknown(other) => FinishReason::Other(other),
        }
    }
}

impl From<Usage> for UsageInfo {
    /// Cached input tokens count towards the prompt, as they do for other providers.
    fn from(usage: Usage) -> Self {
        let prompt = usage
            .input_tokens
            .saturating_add(usage.cache_creation_input_tokens.unwrap_or(0))
            .saturating_add(usage.cache_read_input_tokens.unwrap_or(0));
        UsageInfo {
            prompt_tokens: Some(prompt),
            completion_tokens: Some(usage.output_tokens),
            total_tokens: Some(prompt.saturating_add(usage.output_tokens)),
        }
    }
}

/// The tier that actually served the request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ResponseServiceTier {
    Standard,
    Priority,
    Batch,
    #[serde(untagged)]
    Unknown(String),
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::anthropic::api::message::{TextBlock, Thinking};
    use serde_json::json;

    #[test]
    fn stop_reasons_map_to_finish_reasons() {
        let cases = [
            (StopReason::EndTurn, FinishReason::Stop),
            (StopReason::StopSequence, FinishReason::Stop),
            (StopReason::MaxTokens, FinishReason::Length),
            (StopReason::ModelContextWindowExceeded, FinishReason::Length),
            (StopReason::ToolUse, FinishReason::ToolCalls),
            (StopReason::Refusal, FinishReason::ContentFilter),
            (
                StopReason::PauseTurn,
                FinishReason::Other("pause_turn".into()),
            ),
            (
                StopReason::Unknown("new".into()),
                FinishReason::Other("new".into()),
            ),
        ];
        for (stop, finish) in cases {
            assert_eq!(FinishReason::from(stop), finish);
        }
    }

    #[test]
    fn usage_counts_cache_tokens_as_prompt() {
        let usage = Usage {
            input_tokens: 10,
            output_tokens: 5,
            cache_creation_input_tokens: Some(100),
            cache_read_input_tokens: Some(1000),
            service_tier: None,
        };
        assert_eq!(
            UsageInfo::from(usage),
            UsageInfo {
                prompt_tokens: Some(1110),
                completion_tokens: Some(5),
                total_tokens: Some(1115),
            }
        );
    }

    #[test]
    fn text_response_deserializes() {
        let response: MessagesResponse = serde_json::from_value(json!({
            "id": "msg_01",
            "type": "message",
            "role": "assistant",
            "model": "claude-haiku-4-5-20251001",
            "content": [{"type": "text", "text": "Paris"}],
            "stop_reason": "end_turn",
            "stop_sequence": null,
            "usage": {
                "input_tokens": 12,
                "output_tokens": 3,
                "cache_creation_input_tokens": 0,
                "cache_read_input_tokens": 0,
                "service_tier": "standard",
                "inference_geo": "global"
            }
        }))
        .unwrap();
        assert_eq!(response.id, "msg_01");
        assert_eq!(
            response.content,
            vec![AssistantBlock::Text(TextBlock::new("Paris"))]
        );
        assert_eq!(response.stop_reason, Some(StopReason::EndTurn));
        assert_eq!(response.stop_sequence, None);
        assert_eq!(response.usage.input_tokens, 12);
        assert_eq!(response.usage.output_tokens, 3);
        assert_eq!(response.usage.cache_read_input_tokens, Some(0));
        assert_eq!(
            response.usage.service_tier,
            Some(ResponseServiceTier::Standard)
        );
    }

    #[test]
    fn thinking_and_tool_use_response_deserializes() {
        let response: MessagesResponse = serde_json::from_value(json!({
            "id": "msg_02",
            "model": "claude-sonnet-4-6",
            "content": [
                {"type": "thinking", "thinking": "Need the price.", "signature": "EqQB"},
                {"type": "tool_use", "id": "toolu_01", "name": "get_stock_price", "input": {"ticker": "^GSPC"}},
            ],
            "stop_reason": "tool_use",
            "usage": {"input_tokens": 100, "output_tokens": 40}
        }))
        .unwrap();
        assert_eq!(response.stop_reason, Some(StopReason::ToolUse));
        assert_eq!(
            response.content[0],
            AssistantBlock::Thinking(Thinking {
                thinking: "Need the price.".into(),
                signature: "EqQB".into()
            })
        );
        assert!(matches!(&response.content[1], AssistantBlock::ToolUse(t) if t.id == "toolu_01"));
        assert_eq!(response.usage.cache_creation_input_tokens, None);
        assert_eq!(response.usage.service_tier, None);
    }

    #[test]
    fn unknown_values_are_tolerated() {
        let response: MessagesResponse = serde_json::from_value(json!({
            "id": "msg_03",
            "model": "claude-future",
            "content": [{"type": "brand_new_block", "x": 1}],
            "stop_reason": "something_new",
            "usage": {"input_tokens": 1, "output_tokens": 1, "service_tier": "turbo"}
        }))
        .unwrap();
        assert_eq!(
            response.stop_reason,
            Some(StopReason::Unknown("something_new".into()))
        );
        assert_eq!(
            response.content[0],
            AssistantBlock::Opaque(json!({"type": "brand_new_block", "x": 1}))
        );
        assert_eq!(
            response.usage.service_tier,
            Some(ResponseServiceTier::Unknown("turbo".into()))
        );
    }

    #[test]
    fn all_stop_reasons_parse() {
        let cases = [
            ("end_turn", StopReason::EndTurn),
            ("max_tokens", StopReason::MaxTokens),
            ("stop_sequence", StopReason::StopSequence),
            ("tool_use", StopReason::ToolUse),
            ("pause_turn", StopReason::PauseTurn),
            ("refusal", StopReason::Refusal),
            (
                "model_context_window_exceeded",
                StopReason::ModelContextWindowExceeded,
            ),
        ];
        for (raw, expected) in cases {
            assert_eq!(
                serde_json::from_value::<StopReason>(json!(raw)).unwrap(),
                expected
            );
        }
    }
}
