use markhor_extensions::plugin::python::stdio::wrapper::StdioWrapper;
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


#[tokio::test]
async fn test_stdio_plugin() {
    dotenv::dotenv().ok();
    //init_tracing();

    let api_key = std::env::var("GOOGLE_API_KEY").expect("GOOGLE_API_KEY must be stored in .env");

    let plugin = StdioWrapper::new(
        "python-stdio-chat-plugin-gemini".into(),
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