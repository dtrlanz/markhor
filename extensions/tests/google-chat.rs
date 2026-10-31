//! Integration tests for the Google chat client against the real Gemini API.
//!
//! Requires a `.env` file (or environment) containing
//! `GOOGLE_API_KEY=<key>` and `GOOGLE_API_KEY_PERMISSIONS=<comma-separated permissions>`,
//! e.g. `public`. Ignored by default to avoid network calls on every `cargo test`.
//! Run with: `cargo test -p markhor_extensions --test google-chat -- --ignored`

use markhor_core::{
    chat::{ChatError, ChatModel, ChatOptions, MemberId, Msg, common::ContentPart},
    dependencies::Session,
    extension::{Comp, ExtensionConfig},
};
use markhor_extensions::google::Client;

mod common;

async fn session_with_google() -> Session {
    dotenv::dotenv().ok();

    let mut session = Session::new();
    session
        .add_api_keys_from_env()
        .expect("API key environment variables must be valid");
    session
        .add_extension::<(), Client>((), ExtensionConfig::default())
        .await
        .expect("failed to add Google extension");
    session
}

#[tokio::test]
#[ignore = "requires GOOGLE_API_KEY and network access"]
async fn model_list_is_initialized() {
    let session = session_with_google().await;

    let models: Vec<Comp<dyn ChatModel>> = session.resolve().unwrap();
    assert!(!models.is_empty(), "expected at least one chat model");

    let names: Vec<&str> = models.iter().map(|m| m.model_name()).collect();
    assert!(
        names.iter().any(|n| n.starts_with("gemini-")),
        "expected a Gemini model, got: {names:?}"
    );
    assert!(
        names.iter().all(|n| !n.starts_with("models/")),
        "model names should not carry the `models/` prefix: {names:?}"
    );
}

#[tokio::test]
#[ignore = "requires GOOGLE_API_KEY and network access"]
async fn resolved_chat_model_can_generate() {
    let session = session_with_google().await;

    // `*-latest` aliases are sorted first, so a single `Comp` resolves to one of them rather
    // than a retired model (e.g. `gemini-2.5-flash` is listed but returns 404).
    let first: Comp<dyn ChatModel> = session.resolve().unwrap();
    println!("first listed model: {}", first.model_name());
    assert!(first.model_name().ends_with("-latest"));

    // Use a Gemma 4 model, which has more generous rate limits for development testing.
    let models: Vec<Comp<dyn ChatModel>> = session.resolve().unwrap();
    let model = models
        .into_iter()
        .find(|m| m.model_name().starts_with("gemma-4"))
        .expect("a Gemma 4 model should be listed");
    println!("using model: {}", model.model_name());

    let messages = [
        Msg::Platform("Answer with a single word.".into()),
        Msg::User {
            sender: MemberId::new("tester", true),
            timestamp: None,
            content: vec![ContentPart::Text("What is the capital of France?".into())],
        },
    ];

    // Gemma endpoints are flaky (intermittent 500s and dropped connections), so retry those.
    let mut attempt = 1;
    let response = loop {
        match model.generate(&messages, &ChatOptions::default()).await {
            Err(ChatError::Api {
                status: Some(500..=599),
                ..
            })
            | Err(ChatError::Network(_))
                if attempt < 4 =>
            {
                println!("attempt {attempt} hit a transient error; retrying");
                attempt += 1;
            }
            result => break result.unwrap(),
        }
    };

    let text: String = response
        .content
        .into_iter()
        .filter_map(ContentPart::into_text)
        .collect();
    assert!(text.contains("Paris"), "unexpected answer: {text:?}");
}
