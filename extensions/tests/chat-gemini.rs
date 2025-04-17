use markhor_core::chat::ChatError;
use markhor_core::chat::chat::{
    ChatApi, ChatOptions, ChatResponse, ChatStream, ContentPart, FinishReason,
    Message, ModelInfo, ToolCallRequest, ToolChoice, ToolDefinition, ToolParameterSchema, ToolResult,
    UsageInfo,
};
use async_trait::async_trait;
use futures::stream::Stream;
use serde::{Deserialize, Serialize};
use serde_json::json;
use tracing::{debug, error, instrument, trace, warn};
use uuid::Uuid;

use markhor_extensions::chat::gemini::{create_default_http_client, GeminiClient};



// Bring trait types into scope
use tokio;
use tracing::info;
use tracing_subscriber; // To see logs during tests
use std::env; // To read API key from environment

// Helper to initialize tracing subscriber
fn setup_tracing() {
    let _ = tracing_subscriber::fmt::try_init();
}

// Helper function to get client (requires GEMINI_API_KEY env var)
async fn get_test_client() -> Option<GeminiClient> {
    // log current directory
    println!("Current directory: {:?}", std::env::current_dir());
    

    dotenv::dotenv().ok();
    let api_key = env::var("GEMINI_API_KEY")
        .expect("GEMINI_API_KEY environment variable not set.");

    let http_client = create_default_http_client().expect("Failed to create client");
    Some(GeminiClient::new(api_key, http_client))
}

#[tokio::test]
#[ignore]
async fn test_gemini_list_models_integration() {
setup_tracing();
    if let Some(client) = get_test_client().await {
        let result = client.list_models().await;
        if let Err(ref e) = result {
            error!(error=%e, "list_models failed");
        }
        assert!(result.is_ok());
        let models = result.unwrap();
        assert!(!models.is_empty());
        // Check if a common model exists
        assert!(models.iter().any(|m| m.id.contains("gemini")));
        info!("Found {} models.", models.len());
        for model in models.iter().take(5) { // Print first 5
            info!("Model: id={}, description={:?}, context_window={:?}, max_output={:?}",
                model.id, model.description, model.context_window, model.max_output_tokens);
        }
    }
}

#[tokio::test]
#[ignore]
async fn test_gemini_generate_simple_integration() {
    setup_tracing();
    if let Some(client) = get_test_client().await {
        let messages = vec![
            Message::user("Hi there, what's the capital of France?"),
        ];
        let options = ChatOptions {
                model_id: Some("gemini-1.5-flash-latest".to_string()),
                temperature: Some(0.7),
                max_tokens: Some(50),
                ..Default::default()
        };

        let result = client.generate(&messages, &options).await;

        if let Err(ref e) = result {
                error!(error = %e, "generate failed");
        }
        assert!(result.is_ok());

        let response = result.unwrap();
        info!(response = ?response, "Received response");

        assert!(!response.content.is_empty());
        assert!(matches!(response.content[0], ContentPart::Text(_)));
        let text_response = match &response.content[0] {
            ContentPart::Text(t) => t,
            _ => panic!("Expected text response"),
        };
        assert!(text_response.to_lowercase().contains("paris"));
        assert!(response.finish_reason.is_some());
        assert!(response.usage.is_some());
        assert_eq!(response.model_id, Some("gemini-1.5-flash-latest".to_string()));
    }
}


#[tokio::test]
#[ignore]
async fn test_gemini_generate_tool_use_integration() {
    setup_tracing();
    if let Some(client) = get_test_client().await {
        let get_weather_tool = ToolDefinition {
            name: "get_current_weather".to_string(),
            description: "Get the current weather in a given location".to_string(),
            parameters: ToolParameterSchema {
                schema_type: "object".to_string(),
                properties: serde_json::from_value(json!({
                    "location": {
                        "type": "string",
                        "description": "The city and state, e.g. San Francisco, CA"
                    },
                    "unit": {
                        "type": "string",
                        "enum": ["celsius", "fahrenheit"]
                    }
                })).unwrap(),
                required: vec!["location".to_string()],
            },
        };

        let mut messages_1 = vec![
            Message::user("What is the weather like in Boston? I prefer temperatures in Celsius."),
        ];
        let options_1 = ChatOptions {
            model_id: Some("gemini-1.5-flash-latest".to_string()), // Or gemini-pro if flash doesn't support tools well
            tools: Some(vec![get_weather_tool.clone()]),
            // ToolChoice::Auto is default if tools are present, or be explicit:
            tool_choice: Some(ToolChoice::Auto),
            ..Default::default()
        };

        // --- First call: Expecting a tool call ---
        info!("Sending initial request expecting tool call...");
        let result_1 = client.generate(&messages_1, &options_1).await;
        if let Err(ref e) = result_1 {
            error!(error = %e, "generate (call 1) failed");
        }
        assert!(result_1.is_ok());
        let response_1 = result_1.unwrap(); // This is the ChatResponse
        info!(response = ?response_1, "Received response 1 (expecting tool call)");

        // Not accurate - value is usually STOP
        //assert!(response_1.finish_reason == Some(FinishReason::ToolCalls));
        assert!(!response_1.tool_calls.is_empty());
        // response_1.content might be empty or contain text like "Okay, I can check..."

        let tool_call_request = response_1.tool_calls.first().unwrap();
        assert_eq!(tool_call_request.name, "get_current_weather");

        // --- Prepare history and tool result for the second call ---

        // 1. Create the Assistant message representing the *first* response
        //    This now correctly includes both content AND the requested tool calls.
        let assistant_message_from_response_1 = Message::assistant_response(
            response_1.content.clone(),   // Pass the content received
            response_1.tool_calls.clone(), // Pass the tool calls requested
        );

        // 2. Simulate executing the tool based on the request
        let tool_call_id = tool_call_request.id.clone(); // Use the ID from the request
        let tool_arguments = &tool_call_request.arguments; // Arguments are already JsonValue
        assert!(tool_arguments.is_object());
        assert!(tool_arguments["location"].to_string().contains("Boston"));
        info!(call_id=%tool_call_id, args=?tool_arguments, "Simulating tool execution");

        // Let's pretend the weather is sunny
        let tool_result_content = json!({
            "temperature": "30",
            "unit": "celsius",
            "description": "Sunny"
        });

        // 3. Create the Tool message with the result
        let tool_result = ToolResult {
            call_id: tool_call_request.id.clone(), // Use the ID from the request
            name: tool_call_request.name.clone(), // Use FUNCTION NAME here for Gemini matching
            content: tool_result_content,
        };
        let tool_result_message = Message::Tool(vec![tool_result]);


        // --- Second call: Provide tool result in proper history context ---

        // 4. Construct the full message history for the second call
        let messages_2 = vec![
            messages_1.pop().unwrap(),                      // Original user prompt
            assistant_message_from_response_1, // Assistant's response (text + tool *call*)
            tool_result_message,               // Tool execution *result*
        ];

        info!(history = ?messages_2, "Sending second request with tool result and full history...");
        let options_2 = ChatOptions {
            model_id: Some("gemini-1.5-flash-latest".to_string()),
            // No specific tools or choice needed here, model should just respond to the result.
            // Although keeping the tool definition might be safer in some edge cases.
            tools: None, // Some(vec![get_weather_tool.clone()]),
            tool_choice: Some(ToolChoice::None), // Don't force/allow tool use now
            ..Default::default()
        };

        // 5. Make the second generate call
        let result_2 = client.generate(&messages_2, &options_1).await;

        if let Err(ref e) = result_2 {
            error!(error = %e, "generate (call 2) failed");
        }
        assert!(result_2.is_ok(), "Second generate call failed");

        let response_2 = result_2.unwrap();
        info!(response = ?response_2, "Received response 2 (expecting final answer)");

        // 6. Assert the final response
        assert!(response_2.finish_reason == Some(FinishReason::Stop) || response_2.finish_reason == Some(FinishReason::Length));
        assert!(response_2.tool_calls.is_empty());
        assert!(!response_2.content.is_empty());
        assert!(matches!(response_2.content[0], ContentPart::Text(_)));

        let text_response = match &response_2.content[0] {
            ContentPart::Text(t) => t,
            _ => panic!("Expected text response"),
        };
        // Check if the final answer incorporates the tool result
        assert!(text_response.contains("30") || text_response.to_lowercase().contains("sunny"));
        assert!(text_response.to_lowercase().contains("boston")); // Should mention location too
        info!("Final text response: {}", text_response);
    }
}