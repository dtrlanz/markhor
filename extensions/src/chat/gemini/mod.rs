
use base64::Engine;
use markhor_core::chat::ChatError;
use markhor_core::chat::chat::{
    ChatApi, ChatOptions, ChatResponse, ChatStream, ContentPart, FinishReason,
    Message, ModelInfo, ToolCallRequest, ToolChoice, ToolParameterSchema,
    UsageInfo,
};
use async_trait::async_trait;
use markhor_core::extension::{Extension, Functionality};
use reqwest::StatusCode;
use serde::{Deserialize, Serialize};
use serde_json::json;
use tracing::{debug, error, instrument, trace, warn};
use uuid::Uuid;

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
    pub fn into_chat_response(self, request_model_id: &str) -> Result<ChatResponse, ChatError> {
        // Take the first candidate if available
        let first_candidate = self.candidates.and_then(|mut c| c.pop());

        // let mut content_parts = Vec::new();
        // let mut tool_calls = Vec::new();
        // let mut finish_reason = FinishReason::Other("No candidate received".to_string());
        // let mut assistant_role_present = false;

        if let Some(cand) = first_candidate {
            let mut content_parts = Vec::new();
            let mut tool_calls = Vec::new();
            let finish_reason = cand.finish_reason.map(Into::into)
                .unwrap_or(FinishReason::Other("Unknown".to_string()));

            if let Some(content) = cand.content {
                // Expecting role "model" for assistant response
                if content.role == "model" {
                    for part in content.parts {
                        match part {
                            GeminiPart::Text { text } => {
                                content_parts.push(ContentPart::Text(text.clone()));
                            }
                            GeminiPart::InlineData { inline_data } => {
                                content_parts.push(ContentPart::Image {
                                    mime_type: inline_data.mime_type,
                                    data: base64::engine::general_purpose::STANDARD.decode(inline_data.data).unwrap(),
                                })
                            }
                            GeminiPart::FunctionCall { function_call } => {
                                tool_calls.push(ToolCallRequest {
                                    // Gemini doesn't provide a unique ID per call in the response.
                                    // Generate one: name + random suffix might be best.
                                    // Using name + UUID for now.
                                    id: format!("gemini-{}", Uuid::new_v4()),
                                    name: function_call.name.clone(),
                                    arguments: function_call.args.clone(),
                                });
                            }
                            GeminiPart::FunctionResponse { .. } => {
                                // This shouldn't happen in the 'model' response content
                                warn!("Unexpected FunctionResponse part in model content.");
                            }
                        }
                    }
                } else {
                    warn!(role = %content.role, "Unexpected role in Gemini candidate content.");
                }
            }

            if content_parts.is_empty() && tool_calls.is_empty() {
                // Note cases where no content AND no tool calls were returned.
                // This might be due to safety filters, or just an empty response.
                debug!("Received response with no text content or tool calls.");
            } 

            Ok(ChatResponse {
                content: content_parts,
                tool_calls,
                usage: self.usage_metadata.map(Into::into),
                finish_reason: Some(finish_reason),
                model_id: Some(request_model_id.to_string()), // Return the model used for the request
            })
        } else {
            Ok(ChatResponse {
                content: vec![],
                tool_calls: vec![],
                usage: self.usage_metadata.map(Into::into),
                finish_reason: Some(FinishReason::Other("No candidate received".to_string())),
                model_id: Some(request_model_id.to_string()), // Return the model used for the request
            })
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
            500..=599 => ChatError::Api { status: Some(response_status.as_u16()), message: msg, source: None },
            _ => ChatError::Api { status: Some(response_status.as_u16()), message: msg, source: None },
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
const DEFAULT_GEMINI_MODEL: &str = "gemini-2.0-flash-lite"; //"gemini-1.5-flash-latest"; // A reasonable default

#[derive(Debug, Clone)]
pub struct GeminiClient {
    api_key: String,
    base_url: String,
    http_client: reqwest::Client,
}

impl GeminiClient {
    pub fn new(api_key: String, http_client: reqwest::Client) -> Self {
        Self::new_with_base_url(api_key, DEFAULT_GEMINI_API_BASE.to_string(), http_client)
    }

    pub fn new_with_base_url(
        api_key: String,
        base_url: String,
        http_client: reqwest::Client,
    ) -> Self {
        GeminiClient {
            api_key,
            base_url,
            http_client,
        }
    }

    fn build_generate_url(&self, model_id: &str) -> String {
        // Example: "https://generativelanguage.googleapis.com/v1beta/models/gemini-pro:generateContent?key=..."
        format!("{}/{}:generateContent?key={}", self.base_url, model_id, self.api_key)
    }

    fn build_list_models_url(&self) -> String {
        // Example: "https://generativelanguage.googleapis.com/v1beta/models?key=..."
        format!("{}?key={}", self.base_url, self.api_key)
    }

    /// Maps Gemini API errors (parsed from JSON or status codes) to our ApiError enum.
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
                        ChatError::Api { status: Some(status.as_u16()), message: error_text, source: None }
                    }
                }
            },
            Err(text_err) => {
                // Failed even to read the error body text
                error!(status = %status, text_error = %text_err, "Failed to read Gemini error response body text");
                ChatError::Api { status: Some(status.as_u16()), message: format!("Failed to read error response body: {}", text_err), source: None }
            }
        }
    }

    /// Converts Markhor's internal message format to Gemini's Content format.
    fn convert_messages(
        messages: &[Message],
    ) -> Result<(Option<GeminiContent>, Vec<GeminiContent>), ChatError> {
        let mut system_instruction: Result<Option<GeminiContent>, ChatError> = Ok(None);
        let gemini_contents: Vec<GeminiContent> = messages.iter()
            .filter(|&m| match m {
                Message::System(_) => {
                    // Gemini uses a dedicated 'system_instruction' field.
                    // We only support one currently. Error if multiple are present.
                    if !system_instruction.as_ref().is_ok_and(|v| v.is_none()) {
                        system_instruction = Err(ChatError::InvalidRequest(
                            "Multiple System messages are not supported by Gemini; use 'system_instruction'.".to_string()
                        ));
                        return false;
                    }
                    // Use first system message as system instruction
                    system_instruction = Ok(Some(m.clone().into()));
                    // Do not include system message in contents
                    false
                }
                _ => true
            })
            .map(|m| m.clone().into())
            .collect();

        system_instruction.map(|sys| (sys, gemini_contents))
    }

    /// Converts ChatOptions tool settings to Gemini format.
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

        let tool_config = match options.tool_choice {
            Some(ToolChoice::Auto) | None => { // Default is AUTO if tools are provided, NONE otherwise
                 if tools.is_some() {
                    Some(GeminiToolConfig { function_calling_config: None })
                 } else {
                    // Explicitly setting NONE might be safer if no tools are given
                    Some(GeminiToolConfig { function_calling_config: None })
                 }
            },
            Some(ToolChoice::None) => Some(GeminiToolConfig { function_calling_config: None }),
             // Gemini 'ANY' corresponds to requiring *some* tool call
            Some(ToolChoice::Required) => Some(GeminiToolConfig { function_calling_config: None }),
            Some(ToolChoice::Tool { ref name }) => Some(GeminiToolConfig {
                function_calling_config: Some(GeminiFunctionCallingConfig {
                    mode: GeminiFunctionCallingMode::Any, // Use ANY when specifying function names
                    allowed_function_names: Some(vec![name.clone()]),
                }),
            }),
        };

        (tools, tool_config)
    }
}


#[async_trait]
impl ChatApi for GeminiClient {
    #[instrument(skip(self))]
    async fn list_models(&self) -> Result<Vec<ModelInfo>, ChatError> {
        let url = self.build_list_models_url();
        debug!(%url, "Requesting Gemini models list");

        let response = self.http_client.get(&url)
            .send()
            .await
            .map_err(|e| ChatError::Network(Box::new(e)))?;

        if !response.status().is_success() {
            error!(status = %response.status(), "Failed to list models");
            return Err(Self::map_gemini_error(response).await);
        }

        let raw_body = response.text().await.map_err(|e| ChatError::Network(Box::new(e)))?;
        trace!(body = %raw_body, "Received model list response body");

        let list_response: GeminiListModelsResponse = serde_json::from_str(&raw_body)
            .map_err(|e| ChatError::Parsing(Box::new(e)))?;


        let models = list_response.models.into_iter()
            .filter(|m| m.name.starts_with("models/")) // Ensure correct format
            .map(|m| ModelInfo {
            // Extract the actual ID from "models/model-id"
            id: m.name.split('/').last().unwrap_or(&m.name).to_string(),
            description: m.description.clone().or(m.display_name.clone()), // Prefer description
            context_window: m.input_token_limit,
            max_output_tokens: m.output_token_limit,
            })
            .collect();

        Ok(models)
    }


    #[instrument(skip(self, messages, options))]
    async fn generate(
        &self,
        messages: &[Message],
        options: &ChatOptions,
    ) -> Result<ChatResponse, ChatError> {
        let model_id = options
            .model_id
            .as_deref()
            .unwrap_or(DEFAULT_GEMINI_MODEL);

        let url = self.build_generate_url(model_id);
        debug!(%url, %model_id, "Sending generate request to Gemini");

        let (system_instruction, gemini_contents) = Self::convert_messages(messages)?;
        let (tools, tool_config) = Self::convert_tools(options);

        let generation_config = GeminiGenerationConfig {
            temperature: options.temperature,
            top_p: options.top_p,
            max_output_tokens: options.max_tokens,
            stop_sequences: options.stop_sequences.clone(),
            candidate_count: Some(1), // Usually want just one candidate for chat
            ..Default::default()
        };

        let request_body = GeminiGenerateRequest {
            contents: gemini_contents,
            tools,
            tool_config,
            system_instruction,
            generation_config: Some(generation_config),
            // safety_settings: None, // Add if needed
        };

        trace!(body = ?request_body, "Constructed Gemini request body");

        let response = self.http_client.post(&url)
            .json(&request_body)
            .send()
            .await
            .map_err(|e| {
                error!(error = %e, "Network error during generate request");
                ChatError::Network(Box::new(e))
            })?;

        if !response.status().is_success() {
            error!(status = %response.status(), "Gemini API returned error status");
            return Err(Self::map_gemini_error(response).await);
        }

        let raw_body = response.text().await.map_err(|e| {
            error!(error = %e, "Failed to read response body");
            ChatError::Network(Box::new(e))
        })?;
        trace!(body = %raw_body, "Received Gemini generate response body");

        let gemini_response: GeminiGenerateResponse = serde_json::from_str(&raw_body)
            .map_err(|e| {
                error!(serde_error = %e, raw_body = %raw_body, "Failed to parse Gemini response JSON");
                ChatError::Parsing(Box::new(e))
            })?;

        debug!("Successfully parsed Gemini response");
        gemini_response.into_chat_response(model_id)

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
        // 7. Mapping the stream items to `Result<String, ApiError>`.
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


const URI: &str = "https://github.com/dtrlanz/markhor/tree/main/extensions/src/chat/gemini";



impl Functionality for GeminiClient {
    fn extension_uri(&self) -> &str {
        URI
    }

    fn id(&self) -> &str {
        "Gemini chat client"
    }
}



pub struct GeminiClientExtension {
    api_key: String,
    http_client: reqwest::Client,
}

impl GeminiClientExtension {
    pub fn new(api_key: String, http_client: reqwest::Client) -> Self {
        GeminiClientExtension { api_key, http_client }
    }
}

impl Extension for GeminiClientExtension {
    fn uri(&self) -> &str {
        URI
    }

    fn name(&self) -> &str {
        "Gemini Client Extension"
    }

    fn description(&self) -> &str {
        "Provides a chat client for the Gemini API."
    }

    fn chat_model(&self) -> Option<Box<dyn ChatApi>> {
        let client = GeminiClient::new(self.api_key.clone(), self.http_client.clone());
        Some(Box::new(client))
    }
}


#[cfg(test)]
mod tests {
    use super::*;
    use markhor_core::chat::chat::{Message, ToolDefinition, ToolParameterSchema, ToolResult}; // Bring trait types into scope
    use std::env; // To read API key from environment

    // Helper to initialize tracing subscriber
    fn setup_tracing() {
        //let _ = tracing_subscriber::fmt::try_init();
    }

     #[test]
     fn test_convert_messages_basic() {
         let messages = vec![
             Message::system("Be helpful."),
             Message::user("Hello"),
             Message::assistant("Hi! How can I help?"),
         ];
         let (system_instr, contents) = GeminiClient::convert_messages(&messages).unwrap();

         assert!(system_instr.is_some());
         assert_eq!(system_instr.unwrap().parts.len(), 1); // Assuming combined text

         assert_eq!(contents.len(), 2);
         assert_eq!(contents[0].role, "user");
         assert!(matches!(contents[0].parts[0], GeminiPart::Text { ref text } if text == "Hello"));
         assert_eq!(contents[1].role, "model");
         assert!(matches!(contents[1].parts[0], GeminiPart::Text { ref text } if text == "Hi! How can I help?"));
     }

     #[test]
     fn test_convert_messages_tool_result() {
         let messages = vec![
             Message::user("What's the weather?"),
             // Simulate assistant asking for tool (cannot represent call well)
             Message::assistant("Okay, I can check that."),
              // Provide tool result
             Message::tool(vec![ToolResult {
                 call_id: "123".to_string(),
                 name: "get_weather".to_string(), // Matched by name in Gemini
                 content: serde_json::json!({
                     "temp": 25,
                     "unit": "C",
                 }),
             }]),
         ];
         let (_system_instr, contents) = GeminiClient::convert_messages(&messages).unwrap();

         assert_eq!(contents.len(), 3); // user, model, function
         assert_eq!(contents[0].role, "user");
         assert_eq!(contents[1].role, "model");
         assert_eq!(contents[2].role, "function");
         assert_eq!(contents[2].parts.len(), 1);
         match &contents[2].parts[0] {
             GeminiPart::FunctionResponse { function_response } => {
                 assert_eq!(function_response.name, "get_weather");
                 assert!(function_response.response.is_object());
                 assert_eq!(function_response.response["temp"], 25);
             }
             _ => panic!("Expected FunctionResponse part"),
         }
     }

      #[test]
      fn test_convert_tools_config() {
           let tool_def = ToolDefinition {
               name: "my_func".to_string(), description: "d".to_string(),
               parameters: ToolParameterSchema{ schema_type: "object".to_string(), properties: Default::default(), required: vec![] }
           };

          // Auto (default when tools present)
          let options_auto = ChatOptions { tools: Some(vec![tool_def.clone()]), tool_choice: None, ..Default::default() };
          let (tools, cfg) = GeminiClient::convert_tools(&options_auto);
          assert!(tools.is_some());
          assert!(cfg.is_some());
          //assert!(matches!(cfg.as_ref().unwrap().mode, GeminiToolChoiceMode::Auto));

          // Explicit Auto
          let options_explicit_auto = ChatOptions { tools: Some(vec![tool_def.clone()]), tool_choice: Some(ToolChoice::Auto), ..Default::default() };
          let (_, cfg) = GeminiClient::convert_tools(&options_explicit_auto);
          assert!(cfg.is_some());
          //assert!(matches!(cfg.as_ref().unwrap().mode, GeminiToolChoiceMode::Auto));


          // None
          let options_none = ChatOptions { tools: Some(vec![tool_def.clone()]), tool_choice: Some(ToolChoice::None), ..Default::default() };
           let (_, cfg) = GeminiClient::convert_tools(&options_none);
          assert!(cfg.is_some());
        //   assert!(matches!(cfg.as_ref().unwrap().mode, GeminiToolChoiceMode::None));

           // Required (Any)
           let options_req = ChatOptions { tools: Some(vec![tool_def.clone()]), tool_choice: Some(ToolChoice::Required), ..Default::default() };
           let (_, cfg) = GeminiClient::convert_tools(&options_req);
          assert!(cfg.is_some());
        //   assert!(matches!(cfg.as_ref().unwrap().mode, GeminiToolChoiceMode::Any));

           // Specific Tool
           let options_tool = ChatOptions { tools: Some(vec![tool_def.clone()]), tool_choice: Some(ToolChoice::Tool { name: "my_func".to_string() }), ..Default::default() };
           let (_, cfg) = GeminiClient::convert_tools(&options_tool);
           assert!(cfg.is_some());
           let config = cfg.unwrap();
        //    assert!(matches!(config.mode, GeminiToolChoiceMode::Function));
           assert!(config.function_calling_config.is_some());
           let func_cfg = config.function_calling_config.unwrap();
           assert!(matches!(func_cfg.mode, GeminiFunctionCallingMode::Any));
           assert_eq!(func_cfg.allowed_function_names, Some(vec!["my_func".to_string()]));
      }
}
