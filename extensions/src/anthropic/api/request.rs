use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use super::{
    is_false,
    message::{CacheControl, Message, TextBlock},
};

/// Body of `POST /v1/messages` (non-streaming).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub(crate) struct MessagesRequest {
    pub model: String,
    /// Required by the API, unlike most other providers.
    pub max_tokens: u32,
    pub messages: Vec<Message>,
    /// System prompt, sent as separate text blocks.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub system: Vec<TextBlock>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub top_p: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub top_k: Option<u32>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub stop_sequences: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thinking: Option<ThinkingConfig>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub tools: Vec<ToolDefinition>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_choice: Option<ToolChoice>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_config: Option<OutputConfig>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub service_tier: Option<ServiceTier>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<Metadata>,
    /// Automatically places a cache breakpoint on the last cacheable block.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cache_control: Option<CacheControl>,
}

impl MessagesRequest {
    pub fn new(model: impl Into<String>, max_tokens: u32, messages: Vec<Message>) -> Self {
        Self {
            model: model.into(),
            max_tokens,
            messages,
            system: Vec::new(),
            temperature: None,
            top_p: None,
            top_k: None,
            stop_sequences: Vec::new(),
            thinking: None,
            tools: Vec::new(),
            tool_choice: None,
            output_config: None,
            service_tier: None,
            metadata: None,
            cache_control: None,
        }
    }
}

/// Extended thinking configuration. Which variants are accepted depends on the model.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub(crate) enum ThinkingConfig {
    /// Manual extended thinking with a token budget (at least 1024, below `max_tokens`).
    Enabled {
        budget_tokens: u32,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        display: Option<ThinkingDisplay>,
    },
    Disabled,
    /// The model decides whether and how much to think.
    Adaptive {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        display: Option<ThinkingDisplay>,
    },
    /// Thinking is off, except for short progress updates between tool calls.
    BetweenTools,
}

/// How thinking content is returned. `Omitted` still returns a signature for multi-turn
/// continuity.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ThinkingDisplay {
    Summarized,
    Omitted,
}

/// Definition of a client-side tool.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct ToolDefinition {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub input_schema: InputSchema,
    /// Guarantees schema validation on tool names and inputs.
    #[serde(default, skip_serializing_if = "is_false")]
    pub strict: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_control: Option<CacheControl>,
}

/// JSON schema for a tool's input, which must be an object schema.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub(crate) enum InputSchema {
    Object {
        #[serde(default, skip_serializing_if = "Map::is_empty")]
        properties: Map<String, Value>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        required: Vec<String>,
        /// Any other schema keywords (e.g. `additionalProperties`, `$defs`).
        #[serde(flatten)]
        extra: Map<String, Value>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub(crate) enum ToolChoice {
    /// The model decides whether to use tools.
    Auto {
        #[serde(default, skip_serializing_if = "is_false")]
        disable_parallel_tool_use: bool,
    },
    /// The model must use some tool.
    Any {
        #[serde(default, skip_serializing_if = "is_false")]
        disable_parallel_tool_use: bool,
    },
    /// The model must use the named tool.
    Tool {
        name: String,
        #[serde(default, skip_serializing_if = "is_false")]
        disable_parallel_tool_use: bool,
    },
    None,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub(crate) struct OutputConfig {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effort: Option<Effort>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub format: Option<OutputFormat>,
}

/// How much effort the model puts into its response.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum Effort {
    Low,
    Medium,
    High,
    Xhigh,
    Max,
}

/// Structured output format.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub(crate) enum OutputFormat {
    JsonSchema { schema: Map<String, Value> },
}

/// Whether to use priority capacity when available.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ServiceTier {
    Auto,
    StandardOnly,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub(crate) struct Metadata {
    /// Opaque external identifier of the end user (no names or email addresses).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user_id: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::anthropic::api::message::UserBlock;
    use serde_json::json;

    fn user(text: &str) -> Message {
        Message::User {
            content: vec![UserBlock::Text(TextBlock::new(text))],
        }
    }

    #[test]
    fn minimal_request_has_only_required_fields() {
        let request = MessagesRequest::new("claude-haiku-4-5", 64, vec![user("Hello")]);
        assert_eq!(
            serde_json::to_value(&request).unwrap(),
            json!({
                "model": "claude-haiku-4-5",
                "max_tokens": 64,
                "messages": [{"role": "user", "content": [{"type": "text", "text": "Hello"}]}],
            })
        );
    }

    #[test]
    fn full_request_serializes() {
        let mut request = MessagesRequest::new("claude-sonnet-4-6", 4096, vec![user("Hi")]);
        request.system = vec![TextBlock {
            text: "Be brief.".into(),
            cache_control: Some(CacheControl::Ephemeral { ttl: None }),
        }];
        request.temperature = Some(0.5);
        request.stop_sequences = vec!["END".into()];
        request.thinking = Some(ThinkingConfig::Enabled {
            budget_tokens: 2048,
            display: Some(ThinkingDisplay::Omitted),
        });
        request.tool_choice = Some(ToolChoice::Tool {
            name: "get_weather".into(),
            disable_parallel_tool_use: true,
        });
        request.tools = vec![ToolDefinition {
            name: "get_weather".into(),
            description: Some("Gets the weather".into()),
            input_schema: InputSchema::Object {
                properties: json!({"city": {"type": "string"}})
                    .as_object()
                    .unwrap()
                    .clone(),
                required: vec!["city".into()],
                extra: Map::new(),
            },
            strict: false,
            cache_control: None,
        }];
        request.output_config = Some(OutputConfig {
            effort: Some(Effort::Xhigh),
            format: None,
        });
        request.service_tier = Some(ServiceTier::StandardOnly);
        request.metadata = Some(Metadata {
            user_id: Some("u-1".into()),
        });

        assert_eq!(
            serde_json::to_value(&request).unwrap(),
            json!({
                "model": "claude-sonnet-4-6",
                "max_tokens": 4096,
                "messages": [{"role": "user", "content": [{"type": "text", "text": "Hi"}]}],
                "system": [{"text": "Be brief.", "cache_control": {"type": "ephemeral"}}],
                "temperature": 0.5,
                "stop_sequences": ["END"],
                "thinking": {"type": "enabled", "budget_tokens": 2048, "display": "omitted"},
                "tools": [{
                    "name": "get_weather",
                    "description": "Gets the weather",
                    "input_schema": {
                        "type": "object",
                        "properties": {"city": {"type": "string"}},
                        "required": ["city"],
                    },
                }],
                "tool_choice": {"type": "tool", "name": "get_weather", "disable_parallel_tool_use": true},
                "output_config": {"effort": "xhigh"},
                "service_tier": "standard_only",
                "metadata": {"user_id": "u-1"},
            })
        );
    }

    #[test]
    fn thinking_config_variants() {
        let cases = [
            (
                ThinkingConfig::Enabled {
                    budget_tokens: 1024,
                    display: None,
                },
                json!({"type": "enabled", "budget_tokens": 1024}),
            ),
            (ThinkingConfig::Disabled, json!({"type": "disabled"})),
            (
                ThinkingConfig::Adaptive {
                    display: Some(ThinkingDisplay::Summarized),
                },
                json!({"type": "adaptive", "display": "summarized"}),
            ),
            (
                ThinkingConfig::BetweenTools,
                json!({"type": "between_tools"}),
            ),
        ];
        for (config, expected) in cases {
            assert_eq!(serde_json::to_value(config).unwrap(), expected);
            assert_eq!(
                serde_json::from_value::<ThinkingConfig>(expected).unwrap(),
                config
            );
        }
    }

    #[test]
    fn tool_choice_variants() {
        let cases = [
            (
                ToolChoice::Auto {
                    disable_parallel_tool_use: false,
                },
                json!({"type": "auto"}),
            ),
            (
                ToolChoice::Any {
                    disable_parallel_tool_use: true,
                },
                json!({"type": "any", "disable_parallel_tool_use": true}),
            ),
            (
                ToolChoice::Tool {
                    name: "t".into(),
                    disable_parallel_tool_use: false,
                },
                json!({"type": "tool", "name": "t"}),
            ),
            (ToolChoice::None, json!({"type": "none"})),
        ];
        for (choice, expected) in cases {
            assert_eq!(serde_json::to_value(&choice).unwrap(), expected);
            assert_eq!(
                serde_json::from_value::<ToolChoice>(expected).unwrap(),
                choice
            );
        }
    }

    #[test]
    fn input_schema_keeps_extra_keywords() {
        let raw = json!({
            "type": "object",
            "properties": {"a": {"type": "integer"}},
            "required": ["a"],
            "additionalProperties": false,
        });
        let schema: InputSchema = serde_json::from_value(raw.clone()).unwrap();
        let InputSchema::Object { extra, .. } = &schema;
        assert_eq!(extra["additionalProperties"], json!(false));
        assert_eq!(serde_json::to_value(&schema).unwrap(), raw);

        let empty = InputSchema::Object {
            properties: Map::new(),
            required: vec![],
            extra: Map::new(),
        };
        assert_eq!(
            serde_json::to_value(&empty).unwrap(),
            json!({"type": "object"})
        );
    }

    #[test]
    fn output_config_serializes() {
        let config = OutputConfig {
            effort: Some(Effort::Low),
            format: Some(OutputFormat::JsonSchema {
                schema: json!({"type": "object"}).as_object().unwrap().clone(),
            }),
        };
        assert_eq!(
            serde_json::to_value(&config).unwrap(),
            json!({
                "effort": "low",
                "format": {"type": "json_schema", "schema": {"type": "object"}},
            })
        );
    }
}
