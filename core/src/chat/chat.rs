use async_trait::async_trait;
use futures::stream::Stream;
use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;
use std::pin::Pin;

use super::ApiError;


#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ContentPart {
    Text(String),
    Image {
        mime_type: String,
        data: Vec<u8>,
    },
}

impl ContentPart {
    pub fn into_text(self) -> Option<String> {
        match self {
            ContentPart::Text(text) => Some(text),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolResult {
    /// An id for matching tool requests to results. 
    /// 
    /// Should uniquely identify a tool call within a list of messages.
    pub call_id: String, 

    /// The name of the function/tool to call.
    pub name: String,

    /// The result of the function call.
    pub content: serde_json::Value,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)] // Removed Eq due to ToolCallRequest -> JsonValue
pub enum Message {
    System(Vec<ContentPart>),
    User(Vec<ContentPart>),
    Assistant {
        content: Vec<ContentPart>,
        #[serde(default)] // Necessary if Assistant messages might not have tool_calls
        tool_calls: Vec<ToolCallRequest>,
    },
    Tool(Vec<ToolResult>),
}

impl Message {
    pub fn system(text: impl Into<String>) -> Self {
        Message::System(vec![ContentPart::Text(text.into())])
    }
    
    pub fn user(text: impl Into<String>) -> Self {
        Message::User(vec![ContentPart::Text(text.into())])
    }

    pub fn assistant(text: impl Into<String>) -> Self {
        Message::Assistant {
            content: vec![ContentPart::Text(text.into())],
            tool_calls: Vec::new(), // Default to no tool calls
        }
    }

    // Assistant message potentially with content and tool calls
    pub fn assistant_response(content: Vec<ContentPart>, tool_calls: Vec<ToolCallRequest>) -> Self {
        Message::Assistant { content, tool_calls }
    }    

    pub fn tool(results: Vec<ToolResult>) -> Self {
        Message::Tool(results)
    }
}


// ============== Tool Use Structures ==============

/// Describes the parameters a tool accepts, using JSON Schema format.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolParameterSchema {
    #[serde(rename = "type")]
    pub schema_type: String, // Typically "object"
    #[serde(default)]
    pub properties: serde_json::Map<String, JsonValue>,
    #[serde(default)]
    pub required: Vec<String>,
}

/// Defines a tool that the model can call.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolDefinition {
    pub name: String,
    pub description: String,
    pub parameters: ToolParameterSchema,
}

/// Specifies how the model should select tools.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolChoice {
    /// The model decides whether to call tools and which ones.FF
    Auto,
    /// The model will not call any tools.
    None,
    /// The model *must* call one or more tools.
    Required, // Some APIs (like OpenAI) support forcing *any* tool call
    /// The model *must* call the specific tool named.
    Tool { name: String },
}

/// Represents a request from the model to call a specific tool.
/// This is typically part of the `ChatResponse`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolCallRequest {
    /// Unique identifier for this specific tool call instance.
    /// Needed to match the `ToolResult` back.
    pub id: String,

    /// The name of the function/tool to call.
    pub name: String,
    
    /// The arguments to pass to the function.
    pub arguments: JsonValue,

    // type: String, // Usually "function", omitted for simplicity unless needed
}



// ============== Configuration ==============

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ChatOptions {
    pub model_id: Option<String>,
    pub temperature: Option<f32>,
    pub max_tokens: Option<u32>,
    pub top_p: Option<f32>,
    pub stop_sequences: Option<Vec<String>>,

    // --- Tool Use Options ---
    /// List of tools the model may call.
    #[serde(default)]
    pub tools: Option<Vec<ToolDefinition>>,
    /// Controls how the model selects tools (if any are provided).
    #[serde(default)]
    pub tool_choice: Option<ToolChoice>,

    // Add other common options like presence_penalty, frequency_penalty, user_id if desired
}


// ============== Response Structures ==============

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct UsageInfo {
    pub prompt_tokens: Option<u32>,
    pub completion_tokens: Option<u32>,
    pub total_tokens: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FinishReason {
    Stop,
    Length,
    /// The model decided to call one or more tools. Check `tool_calls`.
    ToolCalls,
    ContentFilter,
    Cancelled,
    Other(String),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)] // Eq removed due to ToolCallRequest
pub struct ChatResponse {
    /// The content generated by the assistant.
    /// This might be empty if the response contains only tool calls.
    /// It might contain partial thoughts *before* the tool call request.
    pub content: Vec<ContentPart>,

    /// List of tool calls requested by the model in this turn.
    /// If this is non-empty the `finish_reason` should typically be `ToolCalls`.
    /// The application should execute these calls and send back `Message`s with `Role::Tool`.
    #[serde(default)]
    pub tool_calls: Vec<ToolCallRequest>,

    pub usage: Option<UsageInfo>,
    pub finish_reason: Option<FinishReason>,
    pub model_id: Option<String>,

    // Provider-specific metadata could go here, e.g., using Option<JsonValue>
    // Moderation results could go here
}

// ============== Model Info ==============

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelInfo {
    pub id: String,
    pub description: Option<String>,
    pub context_window: Option<u32>,
    pub max_output_tokens: Option<u32>,
    // Could add capability flags: supports_streaming, supports_vision, supports_tool_use
    // pub supports_tool_use: Option<bool>,
    // pub supports_vision: Option<bool>,
}

// ============== The Trait ==============

pub type ChatStream = Pin<Box<dyn Stream<Item = Result<String, ApiError>> + Send>>;

#[async_trait]
pub trait ChatApi: Send + Sync {
    /// Returns a list of models available through this API provider.
    async fn list_models(&self) -> Result<Vec<ModelInfo>, ApiError>;

    /// Generates a chat response, potentially including text and/or tool call requests.
    ///
    /// This method handles the full request-response cycle, including potential tool calls
    /// requested by the model. If the model returns tool calls, the `ChatResponse` will
    /// contain them in the `tool_calls` field, and the `finish_reason` will likely be `ToolCalls`.
    /// The application must then execute the tools and send the results back in a subsequent
    /// call to `generate` using `Message`s with `Role::Tool`.
    ///
    /// # Arguments
    /// * `messages` - Conversation history, including user prompts, assistant replies,
    ///                system instructions, and potentially `Role::Tool` messages with results.
    /// * `options` - Configuration including model ID, generation parameters, and tool definitions.
    ///
    /// # Returns
    /// A `ChatResponse` which may contain an assistant message, tool call requests, or both,
    /// along with metadata.
    async fn generate(&self, messages: &[Message], options: &ChatOptions) -> Result<ChatResponse, ApiError>;

    /// Generates a chat response as a stream of text deltas.
    ///
    /// **Note:** This method is primarily intended for streaming textual responses.
    /// While implementations *might* stream text leading up to a tool call, the tool call
    /// request itself is typically *not* delivered via this stream. Use the `generate` method
    /// to reliably handle tool calls. If the generation stops due to requesting tool calls,
    /// this stream will likely end, possibly without yielding any specific indicator beyond
    /// the stream completing.
    ///
    /// # Arguments
    /// * `messages` - Conversation history. Tool definitions in `options` might influence
    ///                generation even if calls aren't streamed.
    /// * `options` - Configuration options.
    ///
    /// # Returns
    /// A `Stream` yielding `Result<String, ApiError>` for text deltas.
    async fn generate_stream(&self, messages: &[Message], options: &ChatOptions) -> Result<ChatStream, ApiError>;
}