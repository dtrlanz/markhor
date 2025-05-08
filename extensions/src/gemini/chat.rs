
use std::sync::Arc;

use base64::Engine;
use markhor_core::chat::ChatError;
use markhor_core::chat::chat::{
    ChatApi, ChatOptions, ChatResponse, ChatStream, ContentPart, FinishReason,
    Message, ModelInfo, ToolCallRequest, ToolChoice, ToolParameterSchema,
    UsageInfo,
};
use async_trait::async_trait;
use markhor_core::extension::Extension;
use reqwest::{Client, StatusCode};
use serde::{Deserialize, Serialize};
use serde_json::json;
use tracing::{debug, error, instrument, trace, warn};
use uuid::Uuid;
use secrecy::ExposeSecret;

use crate::gemini::error::map_response_error;

use super::error::GeminiError;
use super::shared::{GeminiConfig, SharedGeminiClient, EXTENSION_URI};





// ============== Gemini Specific Request/Response Structs ==============
// These structs mirror the Gemini API structure.

#[derive(Serialize, Debug)]
#[serde(rename_all = "camelCase")]
struct GeminiGenerateRequest {
    contents: Vec<GeminiContent>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tools: Option<Vec<GeminiTool>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_config: Option<GeminiToolConfig>,
    #[serde(skip_serializing_if = "Option::is_none")]
    system_instruction: Option<GeminiContent>, // System prompt
    #[serde(skip_serializing_if = "Option::is_none")]
    generation_config: Option<GeminiGenerationConfig>,
    // safety_settings: Option<Vec<GeminiSafetySetting>>, // Add if needed
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
struct GeminiContent {
    role: String, // "user", "model", "function"
    parts: Vec<GeminiPart>,
}

impl From<Message> for GeminiContent {
    fn from(message: Message) -> Self {
        match message {
            Message::System(parts) => {
                // We only handle text system prompts for now
                let combined_text = parts.into_iter()
                    .filter_map(|p| p.into_text())
                    .collect::<Vec<_>>()
                    .join("\n");

                GeminiContent {
                    role: "system".to_string(), // Role is ignored by API but struct needs it
                    parts: vec![GeminiPart::Text{ text: combined_text }]
                }
            }
            Message::User(parts) => {
                let gemini_parts = parts.into_iter().map(|p| p.into()).collect();
                GeminiContent { 
                    role: "user".to_string(), 
                    parts: gemini_parts 
                }
            }
            Message::Assistant { content: parts, tool_calls } => {
                // Convert standard content parts (Text, Image)
                let mut gemini_parts: Vec<_> = parts.into_iter().map(|p| p.into()).collect();

                // Convert requested tool calls into Gemini FunctionCall parts
                for call_request in tool_calls {
                    gemini_parts.push(GeminiPart::FunctionCall {
                        function_call: GeminiFunctionCall {
                            // The 'name' here is the function the assistant *wants* to call
                            name: call_request.name,
                            // 'args' is the structured JSON arguments - directly use the value
                            args: call_request.arguments,
                        }
                    });
                }

                GeminiContent {
                    role: "model".to_string(), // Assistant role maps to "model"
                    parts: gemini_parts,
                }
            }
            Message::Tool(tool_results) => {
                // Each ToolResult needs to be converted into a FunctionResponse part
                let function_response_parts: Vec<GeminiPart> = tool_results.into_iter()
                    .map(|result| {
                        GeminiPart::FunctionResponse {
                            function_response: GeminiFunctionResponse {
                                name: result.name,
                                response: result.content,
                            }
                        }
                    }).collect();

                GeminiContent {
                    role: "function".to_string(), // Role for providing tool results back (Todo: verify)
                    parts: function_response_parts,
                }
            }
        }        
    }
}

#[derive(Serialize, Deserialize, Debug, Clone)]
//#[serde(rename_all = "camelCase")] // No longer needed, but harmless
#[serde(untagged)] // Allows parts to be text OR function call OR function response etc.
enum GeminiPart {
    // Todo: consider using tuple instead of struct members
    Text {
        text: String,
    },
    InlineData {
        inline_data: GeminiBlob,
    },
    FunctionCall {
        #[serde(rename = "functionCall")]
        function_call: GeminiFunctionCall,
    },
    FunctionResponse {
        #[serde(rename = "functionResponse")]
        function_response: GeminiFunctionResponse,
    },
    // FileData{ file_data: GeminiFileData } // For file uploads if needed
}

impl From<ContentPart> for GeminiPart {
    fn from(part: ContentPart) -> Self {
        match part {
            ContentPart::Text(text) => {
                GeminiPart::Text { text }
            }
            ContentPart::Image { mime_type, data } => {
                // Base64 encode data for inlineData
                let encoded_data = base64::engine::general_purpose::STANDARD.encode(data);
                GeminiPart::InlineData {
                    inline_data: GeminiBlob {
                        mime_type: mime_type,
                        data: encoded_data,
                    }
                }
            }
        }
    }
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
struct GeminiBlob {
    mime_type: String,
    data: String, // Base64 encoded
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
struct GeminiFunctionCall {
    name: String,
    args: serde_json::Value, // Gemini provides args as a JSON object
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
struct GeminiFunctionResponse {
    name: String,
    response: serde_json::Value, // Response often structured
}

#[derive(Serialize, Debug)]
#[serde(rename_all = "camelCase")]
struct GeminiTool {
    function_declarations: Vec<GeminiFunctionDeclaration>,
    // Can add code_execution declaration if needed
}

#[derive(Serialize, Debug)]
#[serde(rename_all = "camelCase")]
struct GeminiFunctionDeclaration {
    name: String,
    description: String,
    parameters: ToolParameterSchema, // Our schema matches Gemini's closely
}

#[derive(Serialize, Debug)]
#[serde(rename_all = "camelCase")]
struct GeminiToolConfig {
    //mode: GeminiToolChoiceMode,
    #[serde(skip_serializing_if = "Option::is_none")]
    function_calling_config: Option<GeminiFunctionCallingConfig>,
}

// #[derive(Serialize, Debug)]
// #[serde(rename_all = "SCREAMING_SNAKE_CASE")]
// enum GeminiToolChoiceMode {
//     Auto,
//     Any,  // Corresponds loosely to our 'Required'
//     None,
//     Function, // Used when specific function(s) are required
// }

#[derive(Serialize, Debug)]
#[serde(rename_all = "camelCase")]
struct GeminiFunctionCallingConfig {
    mode: GeminiFunctionCallingMode, // Typically 'ANY' when used with FUNCTION mode above
    #[serde(skip_serializing_if = "Option::is_none")]
    allowed_function_names: Option<Vec<String>>, // Specify the function name for ToolChoice::Tool
}

#[derive(Serialize, Debug)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
enum GeminiFunctionCallingMode {
    ModeUnspecified,
    Auto,
    Any,
    None,
}

impl From<ToolChoice> for GeminiFunctionCallingMode {
    fn from(choice: ToolChoice) -> Self {
        match choice {
            ToolChoice::Auto => GeminiFunctionCallingMode::Auto,
            ToolChoice::Required => GeminiFunctionCallingMode::Any,
            ToolChoice::None => GeminiFunctionCallingMode::None,
            ToolChoice::Tool { name } => {
                warn!("Forcing use of a specific tool is not supported for Gemini (tool: '{}'). Using ANY", name);
                GeminiFunctionCallingMode::Any
            },
        }
    }
}

#[derive(Serialize, Debug, Default)]
#[serde(rename_all = "camelCase")]
struct GeminiGenerationConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    top_p: Option<f32>,
    // top_k: Option<f32>, // Add if needed
    #[serde(skip_serializing_if = "Option::is_none")]
    candidate_count: Option<u32>, // Typically 1 for chat
    #[serde(skip_serializing_if = "Option::is_none")]
    max_output_tokens: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    stop_sequences: Option<Vec<String>>,
    // response_mime_type: Option<String> // e.g., "application/json" for JSON mode
}

// --- Response Structs ---
// 
// https://cloud.google.com/vertex-ai/generative-ai/docs/model-reference/inference#response

#[derive(Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
struct GeminiGenerateResponse {
    candidates: Option<Vec<GeminiCandidate>>,
    // prompt_feedback: Option<GeminiPromptFeedback>, // Add if needed for safety ratings etc.
    #[serde(default)]
    usage_metadata: Option<GeminiUsageMetadata>,
}

impl GeminiGenerateResponse {
    /// Converts Gemini response to Markhor's internal ChatResponse.
    /// Returns GeminiError on failure (e.g., base64 decoding).
    pub fn into_chat_response(self, request_model_id: &str) -> Result<ChatResponse, GeminiError> { // Return GeminiError
        let first_candidate = self.candidates.and_then(|mut c| c.into_iter().next()); // Use into_iter().next()

        let usage = self.usage_metadata.map(Into::into);

        if let Some(cand) = first_candidate {
            let finish_reason = cand.finish_reason.map(Into::into)
                .unwrap_or(FinishReason::Other("Unknown finish reason".to_string()));

            let mut content_parts = Vec::new();
            let mut tool_calls = Vec::new();

            if let Some(content) = cand.content {
                // Expecting role "model" for assistant response
                if content.role == "model" {
                    for part in content.parts {
                        match part {
                            GeminiPart::Text { text } => {
                                content_parts.push(ContentPart::Text(text)); // No clone needed if we consume part
                            }
                            GeminiPart::InlineData { inline_data } => {
                                // Handle potential base64 decoding error
                                let decoded_data = base64::engine::general_purpose::STANDARD.decode(inline_data.data)
                                    .map_err(|e| {
                                        error!("Failed to decode base64 image data from Gemini response: {}", e);
                                        GeminiError::UnexpectedResponse(format!("Failed to decode base64 image data: {}", e))
                                    })?;
                                content_parts.push(ContentPart::Image {
                                    mime_type: inline_data.mime_type,
                                    data: decoded_data,
                                });
                            }
                            GeminiPart::FunctionCall { function_call } => {
                                tool_calls.push(ToolCallRequest {
                                    // Generate unique ID
                                    id: format!("gemini-{}", Uuid::new_v4()),
                                    name: function_call.name, // No clone needed
                                    arguments: function_call.args, // No clone needed
                                });
                            }
                            GeminiPart::FunctionResponse { .. } => {
                                warn!("Unexpected FunctionResponse part in model content.");
                            }
                        }
                    }
                } else {
                    warn!(role = %content.role, "Unexpected role in Gemini candidate content.");
                    // If role is not 'model', treat as empty response? Or error?
                    // Let's treat as empty for now.
                }
            } else {
                debug!("Gemini candidate received with no 'content' field.");
            }

            if content_parts.is_empty() && tool_calls.is_empty() {
                debug!("Received response with no text content or tool calls (Finish Reason: {:?}).", finish_reason);
                // This might be normal (e.g., safety filter, stop sequence).
            }

            Ok(ChatResponse {
                content: content_parts,
                tool_calls,
                usage,
                finish_reason: Some(finish_reason),
                model_id: Some(request_model_id.to_string()),
            })

        } else {
            // No candidate received at all. This is unexpected for a successful API call.
            warn!("Gemini response contained no candidates.");
            // Return an empty response, but maybe this should be an UnexpectedResponse error?
            // Let's return empty for now, consistent with previous code.
            Ok(ChatResponse {
                content: vec![],
                tool_calls: vec![],
                usage, // Usage might still be present even with no candidates
                finish_reason: Some(FinishReason::Other("No candidate received".to_string())),
                model_id: Some(request_model_id.to_string()),
            })
            // Alternative: Error
            // Err(GeminiError::UnexpectedResponse("Gemini response contained no candidates".to_string()))
        }
    }
}


#[derive(Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
struct GeminiCandidate {
    content: Option<GeminiContent>, // Contains the response message
    finish_reason: Option<GeminiFinishReason>,
    // safety_ratings: Option<Vec<GeminiSafetyRating>>, // Add if needed
    // citation_metadata: Option<GeminiCitationMetadata>, // Add if needed
    // ...
}

#[derive(Deserialize, Serialize, Debug, PartialEq, Eq)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
enum GeminiFinishReason {
    Stop,
    MaxTokens,
    Safety,
    Recitation,
    Blocklist,
    ProhibitedContent,
    Spii,
    MalformedFunctionCall,
    Other,
    Unspecified,
}

impl Into<FinishReason> for GeminiFinishReason {
    fn into(self) -> FinishReason {
        match self {
            GeminiFinishReason::Stop => FinishReason::Stop,
            GeminiFinishReason::MaxTokens => FinishReason::Length,
            GeminiFinishReason::Safety => FinishReason::ContentFilter,
            GeminiFinishReason::Recitation => FinishReason::Other("Recitation".to_string()),
            GeminiFinishReason::Blocklist => FinishReason::Other("Blocklist".to_string()),
            GeminiFinishReason::ProhibitedContent => FinishReason::ContentFilter,
            GeminiFinishReason::Spii => FinishReason::ContentFilter,
            GeminiFinishReason::MalformedFunctionCall => FinishReason::Other("MalformedFunctionCall".to_string()),
            GeminiFinishReason::Other => FinishReason::Other("Unknown".to_string()),
            GeminiFinishReason::Unspecified => FinishReason::Unspecified,
        }
    }
}

#[derive(Deserialize, Debug, Default)]
#[serde(rename_all = "camelCase")]
struct GeminiUsageMetadata {
    #[serde(default)]
    prompt_token_count: Option<u32>,
    #[serde(default)]
    candidates_token_count: Option<u32>, // Sum of tokens for all candidates
    #[serde(default)]
    total_token_count: Option<u32>,
}

impl Into<UsageInfo> for GeminiUsageMetadata {
    fn into(self) -> UsageInfo {
        UsageInfo {
            prompt_tokens: self.prompt_token_count,
            completion_tokens: self.candidates_token_count, // Note: Gemini sums *all* candidates if > 1
            total_tokens: self.total_token_count,
        }        
    }
}

#[derive(Deserialize, Debug)]
struct GeminiErrorResponse {
    error: GeminiErrorDetail,
}

impl GeminiErrorResponse {
    fn into_api_error(self, response_status: StatusCode) -> ChatError {
        let msg = format!("{} (Status: {}, Code: {})", self.error.message, self.error.status, self.error.code);
        match response_status.as_u16() {
            400 => ChatError::InvalidRequest(msg),
            401 | 403 => ChatError::Authentication(msg),
            404 => ChatError::ModelNotFound(msg), // Or potentially other 404 reasons
            429 => ChatError::RateLimited,
            500..=599 => ChatError::Api { 
                status: Some(response_status.as_u16()),
                message: msg,
                source: None,
            },
            _ => ChatError::Api { 
                status: Some(response_status.as_u16()), 
                message: msg,
                source: None,
            },
        }
    }
}

#[derive(Deserialize, Debug)]
struct GeminiErrorDetail {
    code: u16,
    message: String,
    status: String, // e.g. "INVALID_ARGUMENT"
}

// --- Model Info Structs ---
#[derive(Deserialize, Debug)]
struct GeminiListModelsResponse {
    models: Vec<GeminiModelInfo>,
}

#[derive(Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
struct GeminiModelInfo {
    name: String, // Format: "models/{model_id}"
    // version: String,
    display_name: Option<String>,
    description: Option<String>,
    input_token_limit: Option<u32>,
    output_token_limit: Option<u32>,
    // supported_generation_methods: Vec<String>,
    // temperature: Option<f32>,
    // top_p: Option<f32>,
    // top_k: Option<f32>,
}


// ============== Gemini Client Implementation ==============

const DEFAULT_GEMINI_API_BASE: &str = "https://generativelanguage.googleapis.com/v1beta/models";
const DEFAULT_GEMINI_CHAT_MODEL: &str = "gemini-2.0-flash-lite";

#[derive(Debug, Clone)]
pub struct GeminiChatClient {
    shared_client: Arc<SharedGeminiClient>,
    default_model_id: String,
    // Add any chat-specific config here if needed later
}

impl GeminiChatClient {
    /// Creates a new Gemini Chat API client with default settings.
    ///
    /// # Arguments
    /// * `api_key`: Your Google AI API key.
    pub fn new(api_key: impl Into<String>) -> Result<Self, GeminiError> {
        Self::new_with_options(api_key, None, None)
    }

    /// Creates a new Gemini Chat API client with custom options.
    ///
    /// # Arguments
    /// * `api_key`: Your Google AI API key.
    /// * `default_model_id`: Optional default model ID to use if not specified in `ChatOptions`.
    /// * `client_override`: Optional custom `reqwest::Client` to use.
    pub fn new_with_options(
        api_key: impl Into<String>,
        default_model_id: Option<String>,
        client_override: Option<Client>,
    ) -> Result<Self, GeminiError> {
        let config = GeminiConfig::new(api_key)?; // Create base config
        let shared_client = SharedGeminiClient::new(config, client_override)?;
        Self::new_with_shared_client(Arc::new(shared_client), default_model_id)
    }

    /// Creates a new Gemini Chat API client with a pre-built client configuration.
    #[instrument(name = "gemini_chat_client_from_config", skip(shared_client))]
    pub(crate) fn new_with_shared_client(
        shared_client: Arc<SharedGeminiClient>,
        default_model_id: Option<String>,
    ) -> Result<Self, GeminiError> {
        
        let model_id = default_model_id.unwrap_or_else(|| DEFAULT_GEMINI_CHAT_MODEL.to_string());
        debug!(default_model_id = %model_id, "GeminiChatClient created.");
        Ok(Self {
            shared_client,
            default_model_id: model_id,
        })
    }    

    // TO BE REMOVED/REPLACED
    /// Maps Gemini API errors (parsed from JSON or status codes) to our ChatError enum.
    async fn map_gemini_error(err_resp: reqwest::Response) -> ChatError {
        let status = err_resp.status();
        let error_text_result = err_resp.text().await; // Consume body to attempt parsing

        match error_text_result {
            Ok(error_text) => {
                trace!(status = %status, error_body = %error_text, "Gemini API error response body");
                // Try parsing Gemini's specific error format
                match serde_json::from_str::<GeminiErrorResponse>(&error_text) {
                    Ok(gemini_error) => {
                        gemini_error.into_api_error(status)
                    }
                    Err(parse_err) => {
                        // Couldn't parse the specific error format, return generic API error
                        warn!(parse_error = %parse_err, body = %error_text, "Failed to parse Gemini error response JSON");
                        ChatError::Api { 
                            status: Some(status.as_u16()), 
                            message: error_text, 
                            source: Some(Box::new(parse_err)),
                        }
                    }
                }
            },
            Err(text_err) => {
                // Failed even to read the error body text
                error!(status = %status, text_error = %text_err, "Failed to read Gemini error response body text");
                ChatError::Api { 
                    status: Some(status.as_u16()), 
                    message: format!("Failed to read error response body: {}", text_err),
                    source: Some(Box::new(text_err)),
                }
            }
        }
    }

    /// Converts Markhor's internal message format to Gemini's Content format.
    /// Separates the system prompt.
    fn convert_messages(
        messages: &[Message],
    ) -> Result<(Option<GeminiContent>, Vec<GeminiContent>), GeminiError> { // Return GeminiError
        let mut system_instruction: Option<GeminiContent> = None;
        let mut gemini_contents: Vec<GeminiContent> = Vec::with_capacity(messages.len());
        let mut system_message_found = false;

        for message in messages {
            match message {
                Message::System(parts) => {
                    if system_message_found {
                        // Found a second system message
                        return Err(GeminiError::InvalidInput(
                            "Multiple System messages are not supported by Gemini; use 'system_instruction'.".to_string()
                        ));
                    }
                    system_message_found = true;
                    // We only handle text system prompts for now
                    let combined_text = parts.iter()
                        .filter_map(|part| part.clone().into_text())
                        .collect::<Vec<_>>()
                        .join("\n");

                    system_instruction = Some(GeminiContent {
                        // Role is ignored by API for system_instruction, but struct needs it.
                        // Use "user" as per Gemini examples for system_instruction content.
                        role: "user".to_string(),
                        parts: vec![GeminiPart::Text { text: combined_text }],
                    });
                    // Do not add system message to the main contents list
                }
                _ => {
                    // Convert other message types
                    gemini_contents.push(GeminiContent::from(message.clone()));
                }
            }
        }
        Ok((system_instruction, gemini_contents))
    }

     /// Converts ChatOptions tool settings to Gemini format. (Error type not needed here)
    fn convert_tools(options: &ChatOptions) -> (Option<Vec<GeminiTool>>, Option<GeminiToolConfig>) {
         let tools = options.tools.as_ref().map(|defs| {
            vec![GeminiTool {
                function_declarations: defs.iter().map(|def| GeminiFunctionDeclaration {
                    name: def.name.clone(),
                    description: def.description.clone(),
                    parameters: def.parameters.clone(), // Assuming ToolParameterSchema is compatible
                }).collect(),
            }]
        });

        // Revised Tool Choice mapping based on Gemini API docs for function calling config:
        // Mode AUTO: Model decides whether to call functions. Default if tools provided.
        // Mode ANY: Model *must* call one of the provided functions. Corresponds to our Required.
        // Mode NONE: Model will not call any functions. Corresponds to our None.
        // Mode FUNCTION (deprecated/internal?): Not directly exposed like this. Specifying allowed_function_names implies ANY mode.
        let tool_config = match options.tool_choice.as_ref() {
            None | Some(ToolChoice::Auto) => {
                 if tools.is_some() {
                    // If tools are present, default to AUTO mode. No specific config needed for AUTO.
                    None // Omitting tool_config defaults to AUTO if tools are present
                 } else {
                     // No tools and no choice means no function calling.
                     Some(GeminiToolConfig { function_calling_config: Some(GeminiFunctionCallingConfig { mode: GeminiFunctionCallingMode::None, allowed_function_names: None })})
                 }
            },
            Some(ToolChoice::None) => Some(GeminiToolConfig { function_calling_config: Some(GeminiFunctionCallingConfig { mode: GeminiFunctionCallingMode::None, allowed_function_names: None }) }),
            Some(ToolChoice::Required) => {
                if tools.is_none() || tools.as_ref().map_or(true, |t| t.is_empty()) {
                    // Cannot require a tool if none are defined. Log warning? Or error?
                    // Let's proceed but log a warning, API will likely error out.
                    warn!("ToolChoice::Required specified but no tools were provided.");
                    None // Fallback to AUTO-like behavior (or potentially error?)
                } else {
                    Some(GeminiToolConfig { function_calling_config: Some(GeminiFunctionCallingConfig { mode: GeminiFunctionCallingMode::Any, allowed_function_names: None }) })
                }
            },
            Some(ToolChoice::Tool { name }) => Some(GeminiToolConfig {
                function_calling_config: Some(GeminiFunctionCallingConfig {
                    // Specifying allowed names requires ANY mode according to docs
                    mode: GeminiFunctionCallingMode::Any,
                    allowed_function_names: Some(vec![name.clone()]),
                }),
            }),
        };

        (tools, tool_config)
    }
}

#[async_trait]
impl ChatApi for GeminiChatClient {
    #[instrument(skip(self), fields(client = self.shared_client.config().base_url.as_str()))]
    async fn list_models(&self) -> Result<Vec<ModelInfo>, ChatError> {
        // Inner function or block to handle internal GeminiError easily
        async {
            // 1. Build URL using shared client
            // The path segment for listing models in Generative Language API is just "models"
            let url = self.shared_client.build_url("models")?; // build_url adds API key
            debug!(%url, "Requesting Gemini models list");

            // 2. Get HTTP client and send request
            let response = self.shared_client.http_client()
                .get(url)
                .header("x-goog-api-key", self.shared_client.config().api_key.expose_secret()) // API Key in header
                .send()
                .await
                .map_err(GeminiError::Network)?; // Convert reqwest error to GeminiError::Network

            // 3. Check response status
            if !response.status().is_success() {
                let status = response.status();
                error!(%status, "Failed to list models from Gemini API");
                // 4. Map error response using shared helper
                return Err(map_response_error(response).await);
            }

            // 5. Process successful response
            let status = response.status();
            debug!(%status, "Received successful response for model list");
            let raw_body = response.text()
                .await
                .map_err(|e| {
                    // Failed to read body even on success status - likely network issue during read
                    error!(error = %e, "Failed to read successful response body for model list");
                    GeminiError::Network(e)
                })?;
            trace!(body = %raw_body, "Received model list response body");

            // 6. Parse JSON response
            let list_response: GeminiListModelsResponse = serde_json::from_str(&raw_body)
                .map_err(|e| { 
                    error!(parse_error = %e, raw_body = %raw_body, "Failed to parse Gemini model list JSON");
                    GeminiError::ResponseParsing {
                        context: "Parsing model list".to_string(),
                        source: e,
                    }
                })?;

            // 7. Convert to public ModelInfo struct
            let models = list_response.models.into_iter()
                // Filter for models compatible with generateContent (chat) if possible/needed
                // .filter(|m| m.supported_generation_methods.contains(&"generateContent".to_string()))
                .filter_map(|m| {
                    // Extract model ID from "models/model-id" or "tunedModels/model-id"
                    let model_id = m.name.split('/').last();
                    match model_id {
                        Some(id) if !id.is_empty() => Some(ModelInfo {
                            id: id.to_string(),
                            description: m.description.clone().or(m.display_name.clone()), // Prefer description
                            context_window: m.input_token_limit,
                            max_output_tokens: m.output_token_limit,
                            // Add other relevant fields if available and needed
                        }),
                        _ => {
                            warn!(raw_name = %m.name, "Could not parse model ID from Gemini model name");
                            None // Skip models with unexpected name format
                        }
                    }
                })
                .collect::<Vec<_>>();

            debug!(count = models.len(), "Successfully parsed models list");
            Ok(models)
        }
        .await // Execute the inner async block
        .map_err(Into::into) // Convert GeminiError into ChatError at the boundary
    }



    #[instrument(skip(self, messages, options), fields(model = options.model_id.as_deref().unwrap_or(&self.default_model_id)))]
    async fn generate(
        &self,
        messages: &[Message],
        options: &ChatOptions,
    ) -> Result<ChatResponse, ChatError> {
        // Inner async block returning Result<..., GeminiError>
        async {
            // 1. Determine Model ID and Build URL
            let model_id = options
                .model_id
                .as_deref()
                .unwrap_or(&self.default_model_id);
            let path_segment = format!("models/{}:generateContent", model_id);
            let url = self.shared_client.build_url(&path_segment)?; // Adds API key
            debug!(%url, %model_id, "Sending generate request to Gemini");

            // 2. Convert Inputs (Messages and Tools)
            let (system_instruction, gemini_contents) = Self::convert_messages(messages)?;
            let (tools, tool_config) = Self::convert_tools(options);

            // 3. Construct Generation Config
            let generation_config = GeminiGenerationConfig {
                temperature: options.temperature,
                top_p: options.top_p,
                max_output_tokens: options.max_tokens,
                stop_sequences: options.stop_sequences.clone(),
                candidate_count: Some(1), // Usually want just one candidate for chat
                // response_mime_type: options.response_format // Map if/when needed
                ..Default::default()
            };

            // 4. Construct Request Body
            let request_body = GeminiGenerateRequest {
                contents: gemini_contents,
                tools,
                tool_config,
                system_instruction,
                generation_config: Some(generation_config).filter(|c| {
                    // Only include config if it's not default/empty (optimization)
                    c.temperature.is_some() || c.top_p.is_some() || c.max_output_tokens.is_some() || c.stop_sequences.is_some()
                }),
                // safety_settings: None, // Add if needed
            };

            // 5. Serialize Request Body
            let request_json = serde_json::to_string(&request_body)
                .map_err(|e| {
                    error!(error = %e, "Failed to serialize Gemini generate request body");
                    GeminiError::RequestSerialization(e)
                })?;
            trace!(body = %request_json, "Constructed Gemini request body JSON"); // Log JSON, not Debug format

            // 6. Send Request
            let response = self.shared_client.http_client()
                .post(url)
                .header("x-goog-api-key", self.shared_client.config().api_key.expose_secret()) // API Key in header
                .header("Content-Type", "application/json") // Standard header
                // Add User-Agent or other headers if desired
                .body(request_json)
                .send()
                .await
                .map_err(GeminiError::Network)?;

            // 7. Check Response Status
            if !response.status().is_success() {
                let status = response.status();
                error!(%status, "Gemini generate API returned error status");
                // 8. Map Error Response
                return Err(map_response_error(response).await);
            }

            // 9. Process Successful Response
            let status = response.status();
            debug!(%status, "Received successful response for generate request");
             let raw_body = response.text()
                .await
                .map_err(|e| {
                    error!(error = %e, "Failed to read successful response body for generate");
                    GeminiError::Network(e)
                })?;
             trace!(body = %raw_body, "Received Gemini generate response body");

            // 10. Parse JSON Response
            let gemini_response: GeminiGenerateResponse = serde_json::from_str(&raw_body)
                .map_err(|e| {
                    error!(parse_error = %e, raw_body = %raw_body, "Failed to parse Gemini generate response JSON");
                    GeminiError::ResponseParsing {
                        context: "Parsing generate response".to_string(),
                        source: e,
                    }
                })?;

            debug!("Successfully parsed Gemini generate response");

            // 11. Convert to public ChatResponse struct
            gemini_response.into_chat_response(model_id) // This now returns Result<..., GeminiError>
        }
        .await // Execute the inner async block
        .map_err(Into::into) // Convert GeminiError into ChatError at the boundary
    }

    #[instrument(skip(self, messages, options))]
    async fn generate_stream(
        &self,
        messages: &[Message],
        options: &ChatOptions,
    ) -> Result<ChatStream, ChatError> {
        // TODO: Implement streaming for Gemini
        // This involves:
        // 1. Adding ":streamGenerateContent" to the URL.
        // 2. Sending the same request body.
        // 3. Handling the response as a stream of Server-Sent Events (SSE).
        // 4. Parsing each SSE chunk (which will be JSON, similar to GeminiGenerateResponse but partial/delta).
        // 5. Extracting text deltas from `candidates[0].content.parts[0].text`.
        // 6. Handling potential errors within the stream.
        // 7. Mapping the stream items to `Result<String, ChatError>`.
        warn!("Gemini streaming is not yet implemented.");
        Err(ChatError::NotSupported("Streaming is not yet implemented for the Gemini client.".to_string()))
    }
}


// Helper function to create a reqwest client (useful for examples/tests)
// Consider moving this to a more central place if used by multiple clients
pub fn create_default_http_client() -> Result<reqwest::Client, ChatError> {
    reqwest::Client::builder()
       .timeout(std::time::Duration::from_secs(60)) // Example timeout
       .build()
       .map_err(|e| ChatError::Configuration(format!("Failed to build HTTP client: {}", e)))
}





// #[cfg(test)]
// mod tests {
//     use super::*;
//     use markhor_core::chat::chat::{Message, ToolDefinition, ToolParameterSchema, ToolResult}; // Bring trait types into scope
//     use std::env; // To read API key from environment

//     // Helper to initialize tracing subscriber
//     fn setup_tracing() {
//         //let _ = tracing_subscriber::fmt::try_init();
//     }

//      #[test]
//      fn test_convert_messages_basic() {
//          let messages = vec![
//              Message::system("Be helpful."),
//              Message::user("Hello"),
//              Message::assistant("Hi! How can I help?"),
//          ];
//          let (system_instr, contents) = GeminiClient::convert_messages(&messages).unwrap();

//          assert!(system_instr.is_some());
//          assert_eq!(system_instr.unwrap().parts.len(), 1); // Assuming combined text

//          assert_eq!(contents.len(), 2);
//          assert_eq!(contents[0].role, "user");
//          assert!(matches!(contents[0].parts[0], GeminiPart::Text { ref text } if text == "Hello"));
//          assert_eq!(contents[1].role, "model");
//          assert!(matches!(contents[1].parts[0], GeminiPart::Text { ref text } if text == "Hi! How can I help?"));
//      }

//      #[test]
//      fn test_convert_messages_tool_result() {
//          let messages = vec![
//              Message::user("What's the weather?"),
//              // Simulate assistant asking for tool (cannot represent call well)
//              Message::assistant("Okay, I can check that."),
//               // Provide tool result
//              Message::tool(vec![ToolResult {
//                  call_id: "123".to_string(),
//                  name: "get_weather".to_string(), // Matched by name in Gemini
//                  content: serde_json::json!({
//                      "temp": 25,
//                      "unit": "C",
//                  }),
//              }]),
//          ];
//          let (_system_instr, contents) = GeminiClient::convert_messages(&messages).unwrap();

//          assert_eq!(contents.len(), 3); // user, model, function
//          assert_eq!(contents[0].role, "user");
//          assert_eq!(contents[1].role, "model");
//          assert_eq!(contents[2].role, "function");
//          assert_eq!(contents[2].parts.len(), 1);
//          match &contents[2].parts[0] {
//              GeminiPart::FunctionResponse { function_response } => {
//                  assert_eq!(function_response.name, "get_weather");
//                  assert!(function_response.response.is_object());
//                  assert_eq!(function_response.response["temp"], 25);
//              }
//              _ => panic!("Expected FunctionResponse part"),
//          }
//      }

//       #[test]
//       fn test_convert_tools_config() {
//            let tool_def = ToolDefinition {
//                name: "my_func".to_string(), description: "d".to_string(),
//                parameters: ToolParameterSchema{ schema_type: "object".to_string(), properties: Default::default(), required: vec![] }
//            };

//           // Auto (default when tools present)
//           let options_auto = ChatOptions { tools: Some(vec![tool_def.clone()]), tool_choice: None, ..Default::default() };
//           let (tools, cfg) = GeminiClient::convert_tools(&options_auto);
//           assert!(tools.is_some());
//           assert!(cfg.is_some());
//           //assert!(matches!(cfg.as_ref().unwrap().mode, GeminiToolChoiceMode::Auto));

//           // Explicit Auto
//           let options_explicit_auto = ChatOptions { tools: Some(vec![tool_def.clone()]), tool_choice: Some(ToolChoice::Auto), ..Default::default() };
//           let (_, cfg) = GeminiClient::convert_tools(&options_explicit_auto);
//           assert!(cfg.is_some());
//           //assert!(matches!(cfg.as_ref().unwrap().mode, GeminiToolChoiceMode::Auto));


//           // None
//           let options_none = ChatOptions { tools: Some(vec![tool_def.clone()]), tool_choice: Some(ToolChoice::None), ..Default::default() };
//            let (_, cfg) = GeminiClient::convert_tools(&options_none);
//           assert!(cfg.is_some());
//         //   assert!(matches!(cfg.as_ref().unwrap().mode, GeminiToolChoiceMode::None));

//            // Required (Any)
//            let options_req = ChatOptions { tools: Some(vec![tool_def.clone()]), tool_choice: Some(ToolChoice::Required), ..Default::default() };
//            let (_, cfg) = GeminiClient::convert_tools(&options_req);
//           assert!(cfg.is_some());
//         //   assert!(matches!(cfg.as_ref().unwrap().mode, GeminiToolChoiceMode::Any));

//            // Specific Tool
//            let options_tool = ChatOptions { tools: Some(vec![tool_def.clone()]), tool_choice: Some(ToolChoice::Tool { name: "my_func".to_string() }), ..Default::default() };
//            let (_, cfg) = GeminiClient::convert_tools(&options_tool);
//            assert!(cfg.is_some());
//            let config = cfg.unwrap();
//         //    assert!(matches!(config.mode, GeminiToolChoiceMode::Function));
//            assert!(config.function_calling_config.is_some());
//            let func_cfg = config.function_calling_config.unwrap();
//            assert!(matches!(func_cfg.mode, GeminiFunctionCallingMode::Any));
//            assert_eq!(func_cfg.allowed_function_names, Some(vec!["my_func".to_string()]));
//       }
// }
