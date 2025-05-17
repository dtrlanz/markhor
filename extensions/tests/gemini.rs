use markhor_core::chat::chat::{
    ChatApi, ChatOptions, ContentPart, FinishReason,
    Message, ToolChoice, ToolDefinition, ToolParameterSchema, ToolResult,
};
use markhor_core::embedding::{Embedder, EmbeddingUseCase, EmbeddingError};
use serde_json::json;
use tracing::error;

use markhor_extensions::gemini::{GeminiChatClient, GeminiEmbedder};



// Bring trait types into scope
use tokio;
use tracing::info;
use tracing_subscriber; // To see logs during tests
use std::env; // To read API key from environment

// Helper to initialize tracing subscriber
fn setup_tracing() {
    let _ = tracing_subscriber::fmt::try_init();
}

// Helper function to get api key (requires GEMINI_API_KEY env var)
async fn get_api_key() -> String {
    // log current directory
    println!("Current directory: {:?}", std::env::current_dir());

    dotenv::dotenv().ok();
    let api_key = env::var("GEMINI_API_KEY")
        .expect("GEMINI_API_KEY environment variable not set.");

    api_key
}

// Helper function to get client (requires GEMINI_API_KEY env var)
async fn get_chat_client() -> GeminiChatClient {
    GeminiChatClient::new(get_api_key().await).expect("Failed to create GeminiChatClient")
}

#[tokio::test]
#[ignore]
async fn test_gemini_list_models_integration() {
    setup_tracing();
    let client = get_chat_client().await;
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

#[tokio::test]
#[ignore]
async fn test_gemini_generate_simple_integration() {
    setup_tracing();
    let client = get_chat_client().await;
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


#[tokio::test]
#[ignore]
async fn test_gemini_generate_tool_use_integration() {
    setup_tracing();
    let client = get_chat_client().await;

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




#[tokio::test]
async fn integration_test_gemini_embed_success() {
    let api_key = get_api_key().await;

    // Use a known, generally available model
    let model_name = "embedding-001";

    let embedder = match GeminiEmbedder::new(api_key, model_name) {
            Ok(e) => e,
            Err(e) => panic!("Failed to create GeminiEmbedder: {}", e),
    };

    let texts = ["Hello Gemini!", "This is an integration test."];

    let result = embedder.embed(&texts).await;

    match result {
        Ok(embeddings) => {
            assert_eq!(embeddings.len(), texts.len(), "Should return same number of embeddings as inputs");
            let expected_dims = embedder.dimensions();
            assert!(expected_dims.is_some(), "Dimensions should be known for {}", model_name);
            let expected_dims = expected_dims.unwrap(); // Safe after check

            for (i, embedding) in embeddings.iter().enumerate() {
                assert_eq!(embedding.0.len(), expected_dims, "Embedding {} should have correct dimensions", i);
                // Basic sanity check on values (not all zero)
                assert!(embedding.0.iter().any(|&v| v != 0.0), "Embedding {} should not be all zeros", i);
            }
            assert_eq!(embedder.intended_use_case(), EmbeddingUseCase::General, "Default use case should be General");
        }
        Err(e) => {
            panic!("Embedding failed unexpectedly: {}", e);
        }
    }
}

#[tokio::test]
#[ignore]
async fn integration_test_gemini_embed_with_task_type() {
    let api_key = get_api_key().await;

    let model_name = "embedding-001";
    let task_type = "RETRIEVAL_DOCUMENT".to_string();

    let embedder = match GeminiEmbedder::new_with_options(
        api_key, 
        model_name,
        Some(task_type.clone()),
        None,
        None,
    ) {
        Ok(e) => e,
        Err(e) => panic!("Failed to create GeminiEmbedder: {}", e),
    };

    assert_eq!(embedder.intended_use_case(), EmbeddingUseCase::RetrievalDocument, "Use case should be mapped correctly");

    let texts = ["This document is intended for retrieval."];
    let result = embedder.embed(&texts).await;

        match result {
        Ok(embeddings) => {
            assert_eq!(embeddings.len(), texts.len(), "Should return embedding for the input");
            let expected_dims = embedder.dimensions().unwrap_or(0);
            assert_ne!(expected_dims, 0, "Dimensions should be > 0");
            assert_eq!(embeddings[0].0.len(), expected_dims, "Embedding should have correct dimensions");
        }
        Err(e) => {
            // Note: Some task_types might eventually be invalid for certain models,
            // so this could potentially error, but RETRIEVAL_DOCUMENT is generally safe.
            panic!("Embedding failed unexpectedly with task_type '{}': {}", task_type, e);
        }
    }
}

#[tokio::test]
#[ignore]
async fn integration_test_gemini_embed_invalid_api_key() {
    let invalid_api_key = "THIS_IS_NOT_A_VALID_API_KEY";
    let model_name = "embedding-001"; // Use a real model name

    // Creation should succeed, the key isn't validated until the API call
    let embedder = match GeminiEmbedder::new(invalid_api_key, model_name) {
        Ok(e) => e,
        Err(e) => panic!("Failed to create GeminiEmbedder (should succeed): {}", e),
    };

    let texts = ["Testing with an invalid key."];
    let result = embedder.embed(&texts).await;

    assert!(result.is_err(), "Embedding should fail with invalid API key");

    // Check if the error is an InvalidRequest
    let err = result.err().unwrap();
    assert!(
        matches!(err, EmbeddingError::InvalidRequest { .. }),
        "Error should be an ApiError variant, got {:?}", err
    );

    assert!(
        format!("{}", err).contains("API key"), 
        "Error message should include 'API key', got {:?}", err
    );
}

#[tokio::test]
#[ignore]
async fn integration_test_gemini_embed_invalid_model_name() {
    let api_key = get_api_key().await;

    let invalid_model_name = "non-existent-embedding-model-foobar";

    let embedder = match GeminiEmbedder::new(api_key, invalid_model_name) {
        Ok(e) => e,
        Err(e) => panic!("Failed to create GeminiEmbedder: {}", e),
    };

    let texts = ["Testing with an invalid model."];
    let result = embedder.embed(&texts).await;

    assert!(result.is_err(), "Embedding should fail with invalid model name");

    // Expect an error, likely a 404, which translates to ModelNotFound
    let err = result.err().unwrap();
    assert!(
        matches!(err, EmbeddingError::ModelNotFound( .. )),
        "Error should be ModelNotFound variant, got {:?}", err
    );

    // Optional: Check if status code seems like 'not found' or 'invalid argument' if needed
    if let EmbeddingError::Api { status, message, .. } = err {
            println!("Received ApiError status: {:?}, message: {}", status, message);
            // Example check: assert!(status_code == Some(404) || status_code == Some(400));
    }
}

#[tokio::test]
#[ignore]
async fn integration_test_gemini_batch_limit_error() {
    let api_key = get_api_key().await;

    let model_name = "embedding-001";

    let embedder = match GeminiEmbedder::new(api_key, model_name) {
            Ok(e) => e,
            Err(e) => panic!("Failed to create GeminiEmbedder: {}", e),
    };

    // Create a batch larger than the expected limit (100 for embedding-001)
    let large_batch: Vec<&str> = (0..101).map(|_| "text").collect();

    let result = embedder.embed(&large_batch).await;

    assert!(result.is_err(), "Embedding should fail when batch size exceeds limit");

    let err = result.err().unwrap();
    assert!(
        matches!(err, EmbeddingError::BatchTooLarge { limit: Some(100), actual: 101 }),
        "Error should be BatchTooLarge with correct details, got {:?}", err
    );
}

