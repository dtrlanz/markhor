use std::sync::Arc;

use async_trait::async_trait;
use markhor_core::chat::{
    ChatError, ChatModel, ChatOptions, ChatResponse, FinishReason as ChatFinishReason, Msg,
    common::ContentPart,
};
use reqwest::StatusCode;

use crate::google::{
    API_KEY_HEADER, ClientInner,
    api::{
        BlockReason, ErrorResponse, GenerateContentRequest, GenerateContentResponse, ModelPart,
        ModelPartKind, ModelTurn, SystemInstruction, Turn, UserPart, UserTurn, wire_name,
    },
};

#[derive(Debug)]
pub struct ChatClient {
    pub(super) inner: Arc<ClientInner>,
    pub(super) model_name: String,
}

#[async_trait]
impl ChatModel for ChatClient {
    fn model_name(&self) -> &str {
        &self.model_name
    }

    async fn generate(
        &self,
        messages: &[Msg],
        options: &ChatOptions,
    ) -> Result<ChatResponse, ChatError> {
        let request = build_request(messages, options)?;

        let response = self
            .inner
            .http_client
            .post(
                self.inner
                    .url(&format!("models/{}:generateContent", self.model_name)),
            )
            .header(API_KEY_HEADER, self.inner.api_key.key())
            .json(&request)
            .send()
            .await
            .map_err(network_error)?;

        let status = response.status();
        let body = response.bytes().await.map_err(network_error)?;
        if !status.is_success() {
            return Err(map_error_response(status, &body, &self.model_name));
        }

        let parsed: GenerateContentResponse =
            serde_json::from_slice(&body).map_err(|e| ChatError::Parsing(Box::new(e)))?;
        convert_response(parsed)
    }
}

/// Strips the request URL, which is not secret but is noisy, from network errors.
fn network_error(error: reqwest::Error) -> ChatError {
    ChatError::Network(Box::new(error.without_url()))
}

/// Maps Markhor messages to a Gemini request.
///
/// - `Platform` and `Developer` messages become the system instruction (Gemini has no
///   mid-conversation system role).
/// - First-person `Agent` messages become `model` turns; everything else that carries
///   content (including other agents' messages) becomes a `user` turn.
/// - Consecutive turns with the same role are merged, as Gemini expects alternating roles.
/// - `Event` and `ControlSignal` messages are not meant for the model and are skipped.
fn build_request(
    messages: &[Msg],
    options: &ChatOptions,
) -> Result<GenerateContentRequest, ChatError> {
    let mut system = Vec::new();
    let mut contents: Vec<Turn> = Vec::new();

    for msg in messages {
        match msg {
            Msg::Platform(text) | Msg::Developer(text) => system.push(text.clone()),
            Msg::User { content, .. } | Msg::Direct { content, .. } => {
                push_user_parts(&mut contents, content);
            }
            Msg::Agent {
                sender,
                content,
                tool_calls,
                ..
            } => {
                // TODO(tool support): map `tool_calls` to `FunctionCall` parts (restoring
                // thought signatures) and `Msg::ToolResult` to `FunctionResponse` parts.
                if !tool_calls.is_empty() {
                    return Err(ChatError::NotSupported(
                        "tool calls are not yet supported for Google models".into(),
                    ));
                }
                if sender.is_first_person() {
                    push_model_parts(&mut contents, content);
                } else {
                    push_user_parts(&mut contents, content);
                }
            }
            Msg::ToolResult { .. } => {
                return Err(ChatError::NotSupported(
                    "tool results are not yet supported for Google models".into(),
                ));
            }
            Msg::Event { .. } | Msg::ControlSignal { .. } => {
                tracing::debug!("skipping message not intended for the model");
            }
        }
    }

    if contents.is_empty() {
        return Err(ChatError::InvalidRequest(
            "at least one non-empty user message is required".into(),
        ));
    }

    Ok(GenerateContentRequest {
        contents,
        system_instruction: (!system.is_empty()).then_some(SystemInstruction(system)),
        generation_config: options.into(),
    })
}

fn push_user_parts(contents: &mut Vec<Turn>, content: &[ContentPart]) {
    if content.is_empty() {
        return;
    }
    let parts = content.iter().cloned().map(UserPart::from);
    match contents.last_mut() {
        Some(Turn::User(turn)) => turn.parts.extend(parts),
        _ => contents.push(
            UserTurn {
                parts: parts.collect(),
            }
            .into(),
        ),
    }
}

fn push_model_parts(contents: &mut Vec<Turn>, content: &[ContentPart]) {
    if content.is_empty() {
        return;
    }
    let parts = content
        .iter()
        .cloned()
        .map(|part| ModelPart::from(ModelPartKind::from(part)));
    match contents.last_mut() {
        Some(Turn::Model(turn)) => turn.parts.extend(parts),
        _ => contents.push(
            ModelTurn {
                parts: parts.collect(),
            }
            .into(),
        ),
    }
}

fn convert_response(response: GenerateContentResponse) -> Result<ChatResponse, ChatError> {
    let GenerateContentResponse {
        candidates,
        prompt_feedback,
        usage_metadata,
        model_version,
        ..
    } = response;

    if let Some(reason) = prompt_feedback.and_then(|f| f.block_reason) {
        return Err(ChatError::ContentModerated(format!(
            "prompt blocked: {}",
            block_reason_name(&reason)
        )));
    }

    let Some(candidate) = candidates.into_iter().next() else {
        return Err(ChatError::Api {
            status: None,
            message: "response contained no candidates".into(),
            source: None,
        });
    };

    let mut content = Vec::new();
    for part in candidate.content.into_iter().flat_map(|turn| turn.parts) {
        match part.kind {
            ModelPartKind::Thought(_) => {}
            // TODO(tool support): route `FunctionCall` parts to `ChatResponse::tool_calls`.
            kind => match ContentPart::try_from(kind) {
                Ok(part) => content.push(part),
                Err(unsupported) => {
                    tracing::warn!(part = ?unsupported, "ignoring unsupported part in Gemini response")
                }
            },
        }
    }

    let finish_reason = candidate.finish_reason;
    if content.is_empty()
        && finish_reason
            .as_ref()
            .is_some_and(|reason| reason.is_content_filter())
    {
        let detail = candidate.finish_message.unwrap_or_default();
        return Err(ChatError::ContentModerated(if detail.is_empty() {
            "response blocked".into()
        } else {
            format!("response blocked: {detail}")
        }));
    }

    Ok(ChatResponse {
        content,
        tool_calls: Vec::new(),
        usage: usage_metadata.map(Into::into),
        finish_reason: finish_reason.map(ChatFinishReason::from),
        model_id: model_version,
    })
}

fn block_reason_name(reason: &BlockReason) -> String {
    wire_name(reason)
}

fn map_error_response(status: StatusCode, body: &[u8], model: &str) -> ChatError {
    let parsed = serde_json::from_slice::<ErrorResponse>(body);
    let (message, invalid_key) = match &parsed {
        Ok(response) => (
            response.error.message.clone(),
            response.error.is_invalid_api_key(),
        ),
        Err(_) => (String::from_utf8_lossy(body).into_owned(), false),
    };

    if invalid_key || matches!(status, StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN) {
        ChatError::Authentication(message)
    } else if status == StatusCode::NOT_FOUND {
        ChatError::ModelNotFound(format!("{model}: {message}"))
    } else if status == StatusCode::TOO_MANY_REQUESTS {
        ChatError::RateLimited
    } else {
        ChatError::Api {
            status: Some(status.as_u16()),
            message,
            source: parsed.err().map(|e| Box::new(e) as _),
        }
    }
}

#[cfg(test)]
mod tests {
    use markhor_core::{
        chat::MemberId, dependencies::ApiKey, extension::Extension, extension::ModelId,
    };
    use serde_json::json;
    use wiremock::{
        Mock, MockServer, ResponseTemplate,
        matchers::{body_json, header, method, path, query_param},
    };

    use super::*;
    use crate::google::{Client, ClientConfig};

    fn user(text: &str) -> Msg {
        Msg::User {
            sender: MemberId::new("alice", true),
            timestamp: None,
            content: vec![ContentPart::Text(text.into())],
        }
    }

    fn client_for(server: &MockServer) -> ChatClient {
        ChatClient {
            inner: inner_for(server),
            model_name: "gemini-test".into(),
        }
    }

    fn inner_for(server: &MockServer) -> Arc<ClientInner> {
        Arc::new(ClientInner {
            api_key: ApiKey::new("test-key".into(), "google".into(), "proj".into()),
            config: Arc::new(ClientConfig {
                api_base_url: Some(format!("{}/v1beta/", server.uri())),
            }),
            http_client: Arc::new(reqwest::Client::new()),
        })
    }

    mod request {
        use super::*;

        #[test]
        fn system_messages_become_system_instruction() {
            let request = build_request(
                &[
                    Msg::Platform("be nice".into()),
                    Msg::Developer("be brief".into()),
                    user("hi"),
                ],
                &ChatOptions {
                    temperature: Some(0.5),
                    max_tokens: Some(10),
                },
            )
            .unwrap();

            assert_eq!(
                serde_json::to_value(&request).unwrap(),
                json!({
                    "contents": [{"role": "user", "parts": [{"text": "hi"}]}],
                    "systemInstruction": {"parts": [{"text": "be nice"}, {"text": "be brief"}]},
                    "generationConfig": {"temperature": 0.5, "maxOutputTokens": 10},
                })
            );
        }

        #[test]
        fn consecutive_user_messages_are_merged() {
            let request = build_request(
                &[
                    user("a"),
                    Msg::Event {
                        recipients: Default::default(),
                        timestamp: None,
                        source: "x".into(),
                        content: json!(null),
                    },
                    user("b"),
                ],
                &ChatOptions::default(),
            )
            .unwrap();

            assert_eq!(
                serde_json::to_value(&request.contents).unwrap(),
                json!([{"role": "user", "parts": [{"text": "a"}, {"text": "b"}]}])
            );
        }

        #[test]
        fn other_agents_are_presented_as_user_turns() {
            let request = build_request(
                &[Msg::Agent {
                    sender: MemberId::new("other", false),
                    timestamp: None,
                    content: vec![ContentPart::Text("hello".into())],
                    tool_calls: vec![],
                    model: ModelId::new("m", "p"),
                    reasoning_content: None,
                    metadata: json!(null),
                }],
                &ChatOptions::default(),
            )
            .unwrap();
            assert!(matches!(request.contents[0], Turn::User(_)));
        }

        #[test]
        fn images_are_sent_as_inline_data() {
            let request = build_request(
                &[Msg::User {
                    sender: MemberId::new("alice", true),
                    timestamp: None,
                    content: vec![ContentPart::Image {
                        mime_type: "image/png".into(),
                        data: vec![1, 2, 3],
                    }],
                }],
                &ChatOptions::default(),
            )
            .unwrap();
            assert_eq!(
                serde_json::to_value(&request.contents).unwrap(),
                json!([{"role": "user", "parts": [
                    {"inlineData": {"mimeType": "image/png", "data": "AQID"}}
                ]}])
            );
        }

        #[test]
        fn empty_conversation_is_rejected() {
            let err =
                build_request(&[Msg::Platform("sys".into())], &ChatOptions::default()).unwrap_err();
            assert!(matches!(err, ChatError::InvalidRequest(_)));
        }

        #[test]
        fn tool_results_are_not_supported_yet() {
            let err = build_request(
                &[
                    user("hi"),
                    Msg::ToolResult {
                        recipients: Default::default(),
                        timestamp: None,
                        tool_call_id: "1".into(),
                        content: vec![],
                    },
                ],
                &ChatOptions::default(),
            )
            .unwrap_err();
            assert!(matches!(err, ChatError::NotSupported(_)));
        }

        // `MemberId`s can't be flagged as first-person outside of `markhor_core` yet,
        // so the model-turn path is exercised through the helpers directly.
        #[test]
        fn merging_model_turns() {
            let mut contents = Vec::new();
            push_user_parts(&mut contents, &[ContentPart::Text("q".into())]);
            push_model_parts(&mut contents, &[ContentPart::Text("a".into())]);
            push_model_parts(&mut contents, &[ContentPart::Text("b".into())]);
            push_model_parts(&mut contents, &[]);
            assert_eq!(
                serde_json::to_value(&contents).unwrap(),
                json!([
                    {"role": "user", "parts": [{"text": "q"}]},
                    {"role": "model", "parts": [{"text": "a"}, {"text": "b"}]},
                ])
            );
        }
    }

    mod response {
        use super::*;

        fn parse(value: serde_json::Value) -> Result<ChatResponse, ChatError> {
            convert_response(serde_json::from_value(value).unwrap())
        }

        #[test]
        fn converts_text_usage_and_finish_reason() {
            let response = parse(json!({
                "candidates": [{
                    "content": {"role": "model", "parts": [
                        {"text": "thinking...", "thought": true},
                        {"text": "Hello"},
                        {"text": " world", "thoughtSignature": "sig"},
                    ]},
                    "finishReason": "STOP",
                }],
                "usageMetadata": {
                    "promptTokenCount": 3,
                    "candidatesTokenCount": 2,
                    "thoughtsTokenCount": 4,
                    "totalTokenCount": 9,
                },
                "modelVersion": "gemini-test-001",
            }))
            .unwrap();

            assert_eq!(
                response.content,
                vec![
                    ContentPart::Text("Hello".into()),
                    ContentPart::Text(" world".into())
                ]
            );
            assert_eq!(response.finish_reason, Some(ChatFinishReason::Stop));
            assert_eq!(response.model_id.as_deref(), Some("gemini-test-001"));
            let usage = response.usage.unwrap();
            assert_eq!(usage.prompt_tokens, Some(3));
            assert_eq!(usage.completion_tokens, Some(6));
            assert_eq!(usage.total_tokens, Some(9));
        }

        #[test]
        fn max_tokens_without_content_is_not_an_error() {
            let response = parse(json!({
                "candidates": [{"finishReason": "MAX_TOKENS"}],
            }))
            .unwrap();
            assert!(response.content.is_empty());
            assert_eq!(response.finish_reason, Some(ChatFinishReason::Length));
        }

        #[test]
        fn blocked_prompt_is_content_moderated() {
            let err = parse(json!({"promptFeedback": {"blockReason": "SAFETY"}})).unwrap_err();
            assert!(matches!(err, ChatError::ContentModerated(m) if m.contains("SAFETY")));
        }

        #[test]
        fn filtered_response_without_content_is_content_moderated() {
            let err = parse(json!({"candidates": [{"finishReason": "SAFETY"}]})).unwrap_err();
            assert!(matches!(err, ChatError::ContentModerated(_)));
        }

        #[test]
        fn no_candidates_is_an_api_error() {
            let err = parse(json!({})).unwrap_err();
            assert!(matches!(err, ChatError::Api { .. }));
        }

        #[test]
        fn function_calls_are_not_surfaced_yet() {
            let response = parse(json!({
                "candidates": [{
                    "content": {"role": "model", "parts": [
                        {"functionCall": {"name": "f", "args": {}}},
                        {"text": "ok"},
                    ]},
                    "finishReason": "STOP",
                }],
            }))
            .unwrap();
            assert_eq!(response.content, vec![ContentPart::Text("ok".into())]);
        }
    }

    mod errors {
        use super::*;

        fn map(status: u16, body: serde_json::Value) -> ChatError {
            map_error_response(
                StatusCode::from_u16(status).unwrap(),
                body.to_string().as_bytes(),
                "m",
            )
        }

        #[test]
        fn maps_statuses() {
            let body = json!({"error": {"code": 0, "message": "nope", "status": "X"}});
            assert!(matches!(map(401, body.clone()), ChatError::Authentication(m) if m == "nope"));
            assert!(matches!(
                map(403, body.clone()),
                ChatError::Authentication(_)
            ));
            assert!(
                matches!(map(404, body.clone()), ChatError::ModelNotFound(m) if m == "m: nope")
            );
            assert!(matches!(map(429, body.clone()), ChatError::RateLimited));
            assert!(matches!(
                map(400, body.clone()),
                ChatError::Api { status: Some(400), message, .. } if message == "nope"
            ));
        }

        #[test]
        fn bad_key_is_authentication_failure() {
            let err = map(
                400,
                json!({"error": {
                    "code": 400,
                    "message": "API key not valid.",
                    "status": "INVALID_ARGUMENT",
                    "details": [{
                        "@type": "type.googleapis.com/google.rpc.ErrorInfo",
                        "reason": "API_KEY_INVALID",
                    }],
                }}),
            );
            assert!(matches!(err, ChatError::Authentication(_)));
        }

        #[test]
        fn non_json_body_is_preserved() {
            let err = map_error_response(StatusCode::BAD_GATEWAY, b"upstream down", "m");
            assert!(matches!(
                err,
                ChatError::Api { status: Some(502), message, source: Some(_) }
                    if message == "upstream down"
            ));
        }
    }

    mod http {
        use super::*;

        #[tokio::test]
        async fn generate_posts_request_and_parses_response() {
            let server = MockServer::start().await;
            Mock::given(method("POST"))
                .and(path("/v1beta/models/gemini-test:generateContent"))
                .and(header("x-goog-api-key", "test-key"))
                .and(body_json(json!({
                    "contents": [{"role": "user", "parts": [{"text": "hi"}]}],
                })))
                .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                    "candidates": [{
                        "content": {"role": "model", "parts": [{"text": "hello"}]},
                        "finishReason": "STOP",
                    }],
                })))
                .expect(1)
                .mount(&server)
                .await;

            let response = client_for(&server)
                .generate(&[user("hi")], &ChatOptions::default())
                .await
                .unwrap();
            assert_eq!(response.content, vec![ContentPart::Text("hello".into())]);
        }

        #[tokio::test]
        async fn generate_maps_api_errors() {
            let server = MockServer::start().await;
            Mock::given(method("POST"))
                .respond_with(ResponseTemplate::new(429).set_body_json(json!({
                    "error": {"code": 429, "message": "slow down", "status": "RESOURCE_EXHAUSTED"}
                })))
                .mount(&server)
                .await;

            let err = client_for(&server)
                .generate(&[user("hi")], &ChatOptions::default())
                .await
                .unwrap_err();
            assert!(matches!(err, ChatError::RateLimited));
        }

        #[tokio::test]
        async fn generate_reports_network_errors() {
            let client = ChatClient {
                inner: Arc::new(ClientInner {
                    api_key: ApiKey::new("test-key".into(), "google".into(), "proj".into()),
                    config: Arc::new(ClientConfig {
                        api_base_url: Some("http://127.0.0.1:1".into()),
                    }),
                    http_client: Arc::new(reqwest::Client::new()),
                }),
                model_name: "gemini-test".into(),
            };
            let err = client
                .generate(&[user("hi")], &ChatOptions::default())
                .await
                .unwrap_err();
            assert!(matches!(err, ChatError::Network(_)));
        }

        #[tokio::test]
        async fn initialize_lists_chat_models_across_pages() {
            let server = MockServer::start().await;
            Mock::given(method("GET"))
                .and(path("/v1beta/models"))
                .and(header("x-goog-api-key", "test-key"))
                .and(query_param("pageToken", "p2"))
                .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                    "models": [{
                        "name": "models/gemini-b",
                        "supportedGenerationMethods": ["generateContent"],
                    }],
                })))
                .with_priority(1)
                .mount(&server)
                .await;
            Mock::given(method("GET"))
                .and(path("/v1beta/models"))
                .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                    "models": [
                        {"name": "models/gemini-a", "supportedGenerationMethods": ["generateContent"]},
                        {"name": "models/embed", "supportedGenerationMethods": ["embedContent"]},
                    ],
                    "nextPageToken": "p2",
                })))
                .with_priority(5)
                .mount(&server)
                .await;

            let mut client = Client {
                inner: inner_for(&server),
                model_list: vec![],
            };
            client.initialize().await.unwrap();

            let names: Vec<String> = client
                .chat_models()
                .iter()
                .map(|m| m.model_name().to_string())
                .collect();
            assert_eq!(names, ["gemini-a", "gemini-b"]);
        }
    }

    /// Live tests against the real API. Run with
    /// `GOOGLE_API_KEY=... cargo test -p markhor_extensions live -- --ignored`.
    mod live {
        use super::*;

        fn live_inner() -> Arc<ClientInner> {
            let key = std::env::var("GOOGLE_API_KEY").expect("GOOGLE_API_KEY must be set");
            Arc::new(ClientInner {
                api_key: ApiKey::new(key, "google".into(), "live".into()),
                config: Default::default(),
                http_client: Default::default(),
            })
        }

        #[tokio::test]
        #[ignore = "requires GOOGLE_API_KEY and network access"]
        async fn live_generate() {
            dotenv::dotenv().ok();
            let model =
                std::env::var("GOOGLE_MODEL").unwrap_or_else(|_| "gemma-4-26b-a4b-it".into());
            let client = ChatClient {
                inner: live_inner(),
                model_name: model,
            };
            let response = client
                .generate(
                    &[
                        Msg::Platform("Answer in one word.".into()),
                        user("What is the capital of France?"),
                    ],
                    &ChatOptions::default(),
                )
                .await
                .unwrap();
            let text: String = response
                .content
                .into_iter()
                .filter_map(ContentPart::into_text)
                .collect();
            assert!(text.contains("Paris"), "unexpected answer: {text}");
        }

        #[tokio::test]
        #[ignore = "requires GOOGLE_API_KEY and network access"]
        async fn live_initialize() {
            dotenv::dotenv().ok();
            let mut client = Client {
                inner: live_inner(),
                model_list: vec![],
            };
            client.initialize().await.unwrap();
            assert!(!client.chat_models().is_empty());
        }
    }
}
