//! Integration tests for the Anthropic chat client against the real Claude API.
//!
//! Requires a `.env` file (or environment) containing
//! `ANTHROPIC_API_KEY=<key>` and `ANTHROPIC_API_KEY_PERMISSIONS=<comma-separated permissions>`,
//! e.g. `public`. Ignored by default to avoid network calls (and spend) on every `cargo test`.
//! Run with: `cargo test -p markhor_extensions --test anthropic-chat -- --ignored`

use markhor_core::{
    chat::{ChatModel, ChatOptions, MemberId, Msg, common::ContentPart},
    dependencies::Session,
    extension::{Comp, ExtensionConfig},
};
use markhor_extensions::anthropic::Client;

async fn session_with_anthropic() -> Session {
    dotenv::dotenv().ok();

    let mut session = Session::new();
    session
        .add_api_keys_from_env()
        .expect("API key environment variables must be valid");
    session
        .add_extension::<(), Client>((), ExtensionConfig::default())
        .await
        .expect("failed to add Anthropic extension");
    session
}

#[tokio::test]
#[ignore = "requires ANTHROPIC_API_KEY and network access"]
async fn model_list_is_initialized() {
    let session = session_with_anthropic().await;

    let models: Vec<Comp<dyn ChatModel>> = session.resolve().unwrap();
    assert!(!models.is_empty(), "expected at least one chat model");

    let names: Vec<&str> = models.iter().map(|m| m.model_name()).collect();
    assert!(
        names.iter().all(|n| n.starts_with("claude-")),
        "expected only Claude models, got: {names:?}"
    );
}

/// Sends a single request without retries, so transient errors stay visible. Uses a Haiku-class
/// model by default to keep cost low; set `ANTHROPIC_MODEL` to a model id prefix to override.
#[tokio::test]
#[ignore = "requires ANTHROPIC_API_KEY and network access"]
async fn resolved_chat_model_can_generate() {
    let session = session_with_anthropic().await;

    let wanted = std::env::var("ANTHROPIC_MODEL").unwrap_or("claude-haiku".into());
    // The API lists models newest first, so this picks the newest match.
    let models: Vec<Comp<dyn ChatModel>> = session.resolve().unwrap();
    let model = models
        .into_iter()
        .find(|m| m.model_name().starts_with(&wanted))
        .unwrap_or_else(|| panic!("a model starting with `{wanted}` should be listed"));
    println!("using model: {}", model.model_name());

    let messages = [
        Msg::Platform("Answer with a single word.".into()),
        Msg::User {
            sender: MemberId::new("tester", true),
            timestamp: None,
            content: vec![ContentPart::Text("What is the capital of France?".into())],
        },
    ];
    let response = model
        .generate(
            &messages,
            &ChatOptions {
                temperature: Some(0.0),
                max_tokens: Some(32),
            },
        )
        .await
        .unwrap_or_else(|e| panic!("{} failed: {e:?}", model.model_name()));

    let text: String = response
        .content
        .into_iter()
        .filter_map(ContentPart::into_text)
        .collect();
    assert!(text.contains("Paris"), "unexpected answer: {text:?}");
    assert!(
        response
            .usage
            .is_some_and(|u| u.completion_tokens > Some(0))
    );
}
