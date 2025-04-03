
use markhor_core::chat::chat::{
    ApiError, ChatApi, ChatOptions, ChatResponse, ChatStream, ContentPart, FinishReason,
    Message, ModelInfo, ToolCallRequest, ToolChoice, ToolDefinition, ToolParameterSchema, ToolResult,
    UsageInfo,
};
use async_trait::async_trait;
use futures::stream::Stream;
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

#[derive(Serialize, Deserialize, Debug, Clone)]
//#[serde(rename_all = "camelCase")] // No longer needed, but harmless
#[serde(untagged)] // Allows parts to be text OR function call OR function response etc.
enum GeminiPart {
    Text {
        text: String,
    },
    // We will need this if we add multimodal support later
    // InlineData {
    //     inline_data: GeminiBlob,
    // },
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

// #[derive(Serialize, Deserialize, Debug, Clone)]
// #[serde(rename_all = "camelCase")]
// struct GeminiBlob {
//     mime_type: String,
//     data: String, // Base64 encoded
// }

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

#[derive(Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
struct GeminiGenerateResponse {
    candidates: Option<Vec<GeminiCandidate>>,
    // prompt_feedback: Option<GeminiPromptFeedback>, // Add if needed for safety ratings etc.
    #[serde(default)]
    usage_metadata: Option<GeminiUsageMetadata>,
}

#[derive(Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
struct GeminiCandidate {
    content: Option<GeminiContent>, // Contains the response message
    finish_reason: Option<GeminiFinishReason>,
    // safety_ratings: Option<Vec<GeminiSafetyRating>>, // Add if needed
    // citation_metadata: Option<GeminiCitationMetadata>, // Add if needed
    token_count: Option<u32>, // Sometimes provided per candidate
}

#[derive(Deserialize, Debug, PartialEq, Eq)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
enum GeminiFinishReason {
    Stop,
    MaxTokens,
    Safety,
    Recitation,
    FunctionCall, // Our trigger for ToolCalls
    Other,        // Catch-all
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

#[derive(Deserialize, Debug)]
struct GeminiErrorResponse {
    error: GeminiErrorDetail,
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
const DEFAULT_GEMINI_MODEL: &str = "gemini-1.5-flash-latest"; // A reasonable default

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
    async fn map_gemini_error(err_resp: reqwest::Response) -> ApiError {
        let status = err_resp.status();
        let error_text_result = err_resp.text().await; // Consume body to attempt parsing

        match error_text_result {
            Ok(error_text) => {
                trace!(status = %status, error_body = %error_text, "Gemini API error response body");
                 // Try parsing Gemini's specific error format
                 match serde_json::from_str::<GeminiErrorResponse>(&error_text) {
                    Ok(gemini_error) => {
                         let msg = format!("{} (Status: {}, Code: {})", gemini_error.error.message, gemini_error.error.status, gemini_error.error.code);
                         match status.as_u16() {
                             400 => ApiError::InvalidRequest(msg),
                             401 | 403 => ApiError::Authentication(msg),
                             404 => ApiError::ModelNotFound(msg), // Or potentially other 404 reasons
                             429 => ApiError::RateLimited,
                             500..=599 => ApiError::Api { status: status.as_u16(), message: msg },
                             _ => ApiError::Api { status: status.as_u16(), message: msg },
                         }
                    }
                    Err(parse_err) => {
                         // Couldn't parse the specific error format, return generic API error
                         warn!(parse_error = %parse_err, body = %error_text, "Failed to parse Gemini error response JSON");
                         ApiError::Api { status: status.as_u16(), message: error_text }
                    }
                }
            },
            Err(text_err) => {
                 // Failed even to read the error body text
                 error!(status = %status, text_error = %text_err, "Failed to read Gemini error response body text");
                 ApiError::Api { status: status.as_u16(), message: format!("Failed to read error response body: {}", text_err)}
            }
        }

    }

    /// Converts our internal Message format to Gemini's Content format.
    fn convert_messages(
        messages: &[Message],
    ) -> Result<(Option<GeminiContent>, Vec<GeminiContent>), ApiError> {
        let mut gemini_contents: Vec<GeminiContent> = Vec::new();
        let mut system_instruction: Option<GeminiContent> = None;

        for message in messages {
            match message {
                Message::System(parts) => {
                    // Gemini uses a dedicated 'system_instruction' field.
                    // We only support one currently. Error if multiple are present?
                    if system_instruction.is_some() {
                        return Err(ApiError::InvalidRequest(
                            "Multiple System messages are not supported by Gemini; use 'system_instruction'.".to_string()
                        ));
                    }
                    // We only handle text system prompts for now
                    let text_parts: Vec<GeminiPart> = parts.iter().filter_map(|part| match part {
                        ContentPart::Text(text) => Some(GeminiPart::Text { text: text.clone() }),
                        ContentPart::Image { .. } => {
                             warn!("Ignoring image part in System message for Gemini.");
                             None
                        }
                    }).collect();

                    if !text_parts.is_empty() {
                        let combined_text = text_parts.into_iter().map(|p| match p {
                            GeminiPart::Text{ text } => text,
                            _ => String::new(), // Should not happen based on filter_map
                        }).collect::<Vec<_>>().join("\n");

                        system_instruction = Some(GeminiContent {
                        role: "system".to_string(), // Role might be ignored by API but struct needs it
                        parts: vec![GeminiPart::Text{ text: combined_text }]
                        });
                    }
                }
                Message::User(parts) => {
                    let gemini_parts = Self::convert_parts(parts)?;
                    if !gemini_parts.is_empty() {
                        gemini_contents.push(GeminiContent { role: "user".to_string(), parts: gemini_parts });
                    }
                }
                Message::Assistant { content, tool_calls } => {
                    // Convert standard content parts (Text, Image)
                    let mut gemini_parts = Self::convert_parts(content)?; // Handles text, ignores images for now

                    // Convert requested tool calls into Gemini FunctionCall parts
                    for call_request in tool_calls {
                        gemini_parts.push(GeminiPart::FunctionCall {
                            function_call: GeminiFunctionCall {
                                // The 'name' here is the function the assistant *wants* to call
                                name: call_request.name.clone(),
                                // 'args' is the structured JSON arguments - directly use/clone the value
                                args: call_request.arguments.clone(), // Clone the JsonValue
                            }
                        });
                    }

                    // Only add the message to history if it contains any parts
                    // (either text content or function calls)
                    if !gemini_parts.is_empty() {
                        gemini_contents.push(GeminiContent {
                            role: "model".to_string(), // Assistant role maps to "model"
                            parts: gemini_parts,
                        });
                    }
                }
                Message::Tool(tool_results) => {
                    // Each ToolResult needs to be converted into a FunctionResponse part
                    let function_response_parts: Vec<GeminiPart> = tool_results.iter().map(|result| {
                         // Attempt to parse the content as JSON, otherwise treat as string
                         let response_value = serde_json::from_str::<serde_json::Value>(&result.content)
                            .unwrap_or_else(|_| json!({ "result": result.content })); // Fallback to simple structure

                        GeminiPart::FunctionResponse {
                            function_response: GeminiFunctionResponse {
                                name: result.name.clone(),
                                response: response_value,
                            }
                        }
                    }).collect();

                    if !function_response_parts.is_empty() {
                        // Add a single 'function' role message containing all tool results for this turn
                        gemini_contents.push(GeminiContent {
                             role: "function".to_string(), // Role for providing tool results back
                             parts: function_response_parts,
                        });
                    }
                }
            }
        }
        Ok((system_instruction, gemini_contents))
    }

    /// Converts ContentParts to GeminiParts (currently only Text).
    fn convert_parts(parts: &[ContentPart]) -> Result<Vec<GeminiPart>, ApiError> {
        let mut gemini_parts = Vec::new();
        for part in parts {
            match part {
                ContentPart::Text(text) => {
                    gemini_parts.push(GeminiPart::Text { text: text.clone() });
                }
                ContentPart::Image { mime_type, data } => {
                    // Base64 encode data for inlineData
                    // let encoded_data = base64::engine::general_purpose::STANDARD.encode(data);
                    // gemini_parts.push(GeminiPart::InlineData {
                    //     inline_data: GeminiBlob {
                    //         mime_type: mime_type.clone(),
                    //         data: encoded_data,
                    //     }
                    // });
                    warn!("Image parts are not yet supported for Gemini and will be ignored.");
                    // Return error if strict handling is needed:
                    // return Err(ApiError::NotSupported("Image content is not yet supported for Gemini.".to_string()));
                }
            }
        }
        Ok(gemini_parts)
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

     /// Converts Gemini response to our ChatResponse.
    fn convert_response(
        gemini_resp: GeminiGenerateResponse,
        request_model_id: &str,
    ) -> Result<ChatResponse, ApiError> {
        let candidate = gemini_resp.candidates.and_then(|mut c| c.pop()); // Take the first candidate if available

        let mut content_parts = Vec::new();
        let mut tool_calls = Vec::new();
        let mut finish_reason = FinishReason::Other("No candidate received".to_string());
        let mut assistant_role_present = false;

        if let Some(cand) = &candidate {
             finish_reason = match cand.finish_reason {
                 Some(GeminiFinishReason::Stop) => FinishReason::Stop,
                 Some(GeminiFinishReason::MaxTokens) => FinishReason::Length,
                 Some(GeminiFinishReason::FunctionCall) => FinishReason::ToolCalls,
                 Some(GeminiFinishReason::Safety) => FinishReason::ContentFilter,
                 Some(GeminiFinishReason::Recitation) => FinishReason::Other("Recitation".to_string()),
                 Some(GeminiFinishReason::Other) | None => FinishReason::Other("Unknown".to_string()),
             };

            if let Some(content) = &cand.content {
                 // Expecting role "model" for assistant response
                 if content.role == "model" {
                     assistant_role_present = true;
                     for part in &content.parts {
                         match part {
                             GeminiPart::Text { text } => {
                                 content_parts.push(ContentPart::Text(text.clone()));
                             }
                             GeminiPart::FunctionCall { function_call } => {
                                // Need to serialize args back to string for our ToolCallRequest
                                let arguments_str = match serde_json::to_string(&function_call.args) {
                                    Ok(s) => s,
                                    Err(e) => {
                                         warn!(error = %e, args = ?function_call.args, "Failed to serialize function call arguments to string");
                                         // Provide a fallback or error? Let's provide empty JSON string.
                                         "{}".to_string()
                                    }
                                };
                                tool_calls.push(ToolCallRequest {
                                    // Gemini doesn't provide a unique ID per call in the response.
                                    // Generate one: name + random suffix might be best.
                                    // Using name + index within this response turn for simplicity now.
                                    // Using UUID is better for uniqueness.
                                    id: format!("gemini-{}", Uuid::new_v4()),
                                    name: function_call.name.clone(),
                                    arguments: function_call.args.clone(),
                                });
                             }
                             GeminiPart::FunctionResponse { .. } => {
                                 // This shouldn't happen in the 'model' response content
                                 warn!("Unexpected FunctionResponse part in model content.");
                             }
                            //  GeminiPart::InlineData { .. } => {
                            //      warn!("InlineData parts are not yet supported for Gemini response.");
                            //      // Handle if needed later
                            //  }
                         }
                     }
                 } else {
                      warn!(role = %content.role, "Unexpected role in Gemini candidate content.");
                 }
            }
        } else if finish_reason == FinishReason::ToolCalls {
             // Sometimes, if only tool calls are made, the candidate might have the calls
             // but not typical 'model' content. We might need further checks or adjustments
             // based on real API behavior. The current loop handles calls regardless of role.
             debug!("Candidate finished with FunctionCall but no 'model' role content found (may be expected).");
        } else if content_parts.is_empty() && tool_calls.is_empty() {
            // Handle cases where no content AND no tool calls were returned.
            // This might be due to safety filters, or just an empty response.
            debug!("Received response with no text content or tool calls.");
             // If finish reason was SAFETY, map it properly
             if let Some(cand) = &candidate {
                  if cand.finish_reason == Some(GeminiFinishReason::Safety) {
                       finish_reason = FinishReason::ContentFilter;
                  }
             }
        }


        // If we have tool calls, the finish reason *must* be ToolCalls
        if !tool_calls.is_empty() {
            finish_reason = FinishReason::ToolCalls;
        }

        let usage = gemini_resp.usage_metadata.map(|m| UsageInfo {
             prompt_tokens: m.prompt_token_count,
             completion_tokens: m.candidates_token_count, // Note: Gemini sums *all* candidates if > 1
             total_tokens: m.total_token_count,
        });


        Ok(ChatResponse {
            content: content_parts,
            tool_calls,
            usage,
            finish_reason: Some(finish_reason),
            model_id: Some(request_model_id.to_string()), // Return the model used for the request
        })
    }

}


#[async_trait]
impl ChatApi for GeminiClient {
    #[instrument(skip(self))]
    async fn list_models(&self) -> Result<Vec<ModelInfo>, ApiError> {
        let url = self.build_list_models_url();
        debug!(%url, "Requesting Gemini models list");

        let response = self.http_client.get(&url)
            .send()
            .await
            .map_err(|e| ApiError::Network(Box::new(e)))?;

        if !response.status().is_success() {
             error!(status = %response.status(), "Failed to list models");
             return Err(Self::map_gemini_error(response).await);
        }

        let raw_body = response.text().await.map_err(|e| ApiError::Network(Box::new(e)))?;
        trace!(body = %raw_body, "Received model list response body");

        let list_response: GeminiListModelsResponse = serde_json::from_str(&raw_body)
             .map_err(|e| ApiError::Parsing(Box::new(e)))?;


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
    ) -> Result<ChatResponse, ApiError> {
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
                 ApiError::Network(Box::new(e))
             })?;

        if !response.status().is_success() {
            error!(status = %response.status(), "Gemini API returned error status");
             return Err(Self::map_gemini_error(response).await);
        }

        let raw_body = response.text().await.map_err(|e| {
             error!(error = %e, "Failed to read response body");
             ApiError::Network(Box::new(e))
         })?;
        trace!(body = %raw_body, "Received Gemini generate response body");
        println!("{:?}", raw_body);


        let gemini_response: GeminiGenerateResponse = serde_json::from_str(&raw_body)
            .map_err(|e| {
                error!(serde_error = %e, raw_body = %raw_body, "Failed to parse Gemini response JSON");
                ApiError::Parsing(Box::new(e))
            })?;

        debug!("Successfully parsed Gemini response");
        Self::convert_response(gemini_response, model_id)

    }

    #[instrument(skip(self, messages, options))]
    async fn generate_stream(
        &self,
        messages: &[Message],
        options: &ChatOptions,
    ) -> Result<ChatStream, ApiError> {
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
         Err(ApiError::NotSupported("Streaming is not yet implemented for the Gemini client.".to_string()))
    }
}


// Helper function to create a reqwest client (useful for examples/tests)
// Consider moving this to a more central place if used by multiple clients
pub fn create_default_http_client() -> Result<reqwest::Client, ApiError> {
     reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(60)) // Example timeout
        .build()
        .map_err(|e| ApiError::Configuration(format!("Failed to build HTTP client: {}", e)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use markhor_core::chat::chat::{Message, ToolParameterSchema}; // Bring trait types into scope
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
                 content: "{\"temp\": 25, \"unit\": \"C\"}".to_string(),
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
