use markhor_extensions::plugin::python::stdio::plugin::PythonStdioPlugin;
use markhor_core::{chat::ChatModel, extension::Extension};

use std::collections::HashMap;
use tracing_subscriber::FmtSubscriber;
use dotenv;

fn init_tracing() {
    let subscriber = FmtSubscriber::builder()
        .with_max_level(tracing::Level::DEBUG)
        .finish();
    tracing::subscriber::set_global_default(subscriber).unwrap();
}

// Helper function to get API key or skip test
fn get_api_key(test_name: &str) -> Option<String> {
    dotenv::dotenv().ok(); // Load .env file if present

    // Check for GOOGLE_API_KEY in environment variables
    match std::env::var("GOOGLE_API_KEY") {
        Ok(key) if !key.is_empty() => Some(key),
        _ => {
            println!("Skipping integration test {} - GOOGLE_API_KEY environment variable not set.", test_name);
            None // Signal to skip
        }
    }
}


#[tokio::test]
async fn test_stdio_plugin() {
    dotenv::dotenv().ok();
    //init_tracing();

    let api_key = match get_api_key("test_stdio_plugin") {
        Some(key) => key,
        None => return, // Skip test
    };

    let plugin = PythonStdioPlugin::new(
        "(uri)".into(),
        "Gemini via Stdio".into(),
        "Uses Gemini chat completion model via stdio".into(),
        "tests/python-chat-plugin".into(),
        "chat_gemini.py".into(),
        None,
        // Default::default(), 
        HashMap::from([("GOOGLE_API_KEY".into(), api_key)]),
    );
    
    let model = plugin.chat_model().unwrap();
    let result = model.chat(
        &vec![
            markhor_core::chat::Message::user("What is tha capital of France?"),
        ],
        Some("gemini-2.0-flash-lite"),
        None,
    ).await;

    println!("Chat completion result:\n{:?}", result);

    let completion = result.unwrap();
    
    assert_eq!(completion.message.role, markhor_core::chat::MessageRole::Assistant);
    assert!(completion.message.content.contains("Paris"));
}