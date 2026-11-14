use std::sync::Arc;

use async_trait::async_trait;
use markhor_core::chat::{
    ChatError, ChatModel, ChatOptions, ChatResponse, Msg, common::ContentPart,
};
use reqwest::StatusCode;

use crate::anthropic::{
    ClientInner,
    api::{
        AssistantBlock, ErrorKind, ErrorResponse, Message, MessagesRequest, MessagesResponse,
        StopReason, TextBlock, UserBlock,
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
        let mut request = build_request(
            &self.model_name,
            self.inner.config.default_max_tokens,
            messages,
            options,
        )?;

        match self.send(&request).await {
            // Claude 4.7 and later reject sampling parameters outright, and the API offers no
            // way to ask which models do. Retry without it rather than failing the request.
            Err(ChatError::InvalidRequest(message))
                if request.temperature.is_some() && mentions_temperature(&message) =>
            {
                tracing::warn!(
                    model = %self.model_name,
                    "model rejected `temperature`; retrying without it"
                );
                request.temperature = None;
                self.send(&request).await
            }
            result => result,
        }
    }
}

impl ChatClient {
    async fn send(&self, request: &MessagesRequest) -> Result<ChatResponse, ChatError> {
        let response = self
            .inner
            .post("messages")
            .json(request)
            .send()
            .await
            .map_err(network_error)?;

        let status = response.status();
        let body = response.bytes().await.map_err(network_error)?;
        if !status.is_success() {
            return Err(map_error_response(status, &body, &self.model_name));
        }

        let parsed: MessagesResponse =
            serde_json::from_slice(&body).map_err(|e| ChatError::Parsing(Box::new(e)))?;
        convert_response(parsed)
    }
}

fn mentions_temperature(message: &str) -> bool {
    message.to_ascii_lowercase().contains("temperature")
}

/// Strips the request URL, which is not secret but is noisy, from network errors.
fn network_error(error: reqwest::Error) -> ChatError {
    ChatError::Network(Box::new(error.without_url()))
}

/// Maps Markhor messages to an Anthropic Messages request.
///
/// - `Platform` and `Developer` messages become the top-level `system` blocks (the API has no
///   mid-conversation system role).
/// - First-person `Agent` messages become `assistant` messages; everything else that carries
///   content (including other agents' messages) becomes a `user` message.
/// - Consecutive messages with the same role are merged, as the API expects alternating roles.
/// - Blank text is dropped, since the API rejects it.
/// - `Event` and `ControlSignal` messages are not meant for the model and are skipped.
fn build_request(
    model: &str,
    default_max_tokens: u32,
    messages: &[Msg],
    options: &ChatOptions,
) -> Result<MessagesRequest, ChatError> {
    let mut system = Vec::new();
    let mut history: Vec<Message> = Vec::new();

    for msg in messages {
        match msg {
            Msg::Platform(text) | Msg::Developer(text) => {
                if !text.trim().is_empty() {
                    system.push(TextBlock::new(text.clone()).into());
                }
            }
            Msg::User { content, .. } | Msg::Direct { content, .. } => {
                push_user_blocks(&mut history, content)?;
            }
            Msg::Agent {
                sender,
                content,
                tool_calls,
                ..
            } => {
                // TODO(tool support): map `tool_calls` to `tool_use` blocks and `Msg::ToolResult`
                // to `tool_result` blocks. Thinking and redacted thinking blocks (with their
                // signatures) must then be echoed back unmodified and in their original order,
                // which needs the provider-specific metadata planned for `Msg::Agent`.
                if !tool_calls.is_empty() {
                    return Err(ChatError::NotSupported(
                        "tool calls are not yet supported for Anthropic models".into(),
                    ));
                }
                if sender.is_first_person() {
                    push_assistant_blocks(&mut history, content);
                } else {
                    push_user_blocks(&mut history, content)?;
                }
            }
            Msg::ToolResult { .. } => {
                return Err(ChatError::NotSupported(
                    "tool results are not yet supported for Anthropic models".into(),
                ));
            }
            Msg::Event { .. } | Msg::ControlSignal { .. } => {
                tracing::debug!("skipping message not intended for the model");
            }
        }
    }

    if !history.iter().any(|m| matches!(m, Message::User { .. })) {
        return Err(ChatError::InvalidRequest(
            "at least one non-empty user message is required".into(),
        ));
    }

    let mut request = MessagesRequest::new(
        model,
        options.max_tokens.unwrap_or(default_max_tokens),
        history,
    );
    request.system = system;
    request.temperature = options.temperature;
    Ok(request)
}

fn is_blank(part: &ContentPart) -> bool {
    matches!(part, ContentPart::Text(text) if text.trim().is_empty())
}

fn push_user_blocks(history: &mut Vec<Message>, content: &[ContentPart]) -> Result<(), ChatError> {
    let blocks = content
        .iter()
        .filter(|part| !is_blank(part))
        .cloned()
        .map(UserBlock::try_from)
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| ChatError::InvalidRequest(e.to_string()))?;
    if blocks.is_empty() {
        return Ok(());
    }
    match history.last_mut() {
        Some(Message::User { content }) => content.extend(blocks),
        _ => history.push(Message::User { content: blocks }),
    }
    Ok(())
}

fn push_assistant_blocks(history: &mut Vec<Message>, content: &[ContentPart]) {
    let blocks: Vec<_> = content
        .iter()
        .filter_map(|part| match part {
            ContentPart::Text(text) if !text.trim().is_empty() => {
                Some(AssistantBlock::Text(text.clone().into()))
            }
            ContentPart::Text(_) => None,
            ContentPart::Image { .. } => {
                tracing::warn!("dropping image in assistant message; the API doesn't accept it");
                None
            }
        })
        .collect();
    if blocks.is_empty() {
        return;
    }
    match history.last_mut() {
        Some(Message::Assistant { content }) => content.extend(blocks),
        _ => history.push(Message::Assistant { content: blocks }),
    }
}

fn convert_response(response: MessagesResponse) -> Result<ChatResponse, ChatError> {
    let MessagesResponse {
        model,
        content,
        stop_reason,
        usage,
        ..
    } = response;

    let mut parts = Vec::new();
    for block in content {
        match block {
            AssistantBlock::Text(TextBlock { text, .. }) => parts.push(ContentPart::Text(text)),
            AssistantBlock::Thinking(_) | AssistantBlock::RedactedThinking(_) => {}
            // TODO(tool support): route `ToolUse` blocks to `ChatResponse::tool_calls`.
            other => {
                tracing::warn!(block = ?other, "ignoring unsupported block in Claude response")
            }
        }
    }

    if parts.is_empty() && stop_reason == Some(StopReason::Refusal) {
        return Err(ChatError::ContentModerated(
            "the model declined to respond".into(),
        ));
    }

    Ok(ChatResponse {
        content: parts,
        tool_calls: Vec::new(),
        usage: Some(usage.into()),
        finish_reason: stop_reason.map(Into::into),
        model_id: Some(model),
    })
}

fn map_error_response(status: StatusCode, body: &[u8], model: &str) -> ChatError {
    let parsed = serde_json::from_slice::<ErrorResponse>(body);
    let (kind, message) = match &parsed {
        Ok(response) => {
            let mut message = response.error.message.clone();
            if let Some(id) = &response.request_id {
                message.push_str(&format!(" (request_id: {id})"));
            }
            (response.error.kind.clone(), message)
        }
        Err(_) => (
            kind_from_status(status),
            String::from_utf8_lossy(body).into_owned(),
        ),
    };

    match kind {
        ErrorKind::Authentication | ErrorKind::Permission => ChatError::Authentication(message),
        ErrorKind::NotFound => ChatError::ModelNotFound(format!("{model}: {message}")),
        ErrorKind::RateLimit => ChatError::RateLimited,
        ErrorKind::InvalidRequest | ErrorKind::RequestTooLarge => {
            ChatError::InvalidRequest(message)
        }
        _ => ChatError::Api {
            status: Some(status.as_u16()),
            message,
            source: parsed.err().map(|e| Box::new(e) as _),
        },
    }
}

/// Used when the body isn't the documented error JSON (e.g., from a proxy).
fn kind_from_status(status: StatusCode) -> ErrorKind {
    match status {
        StatusCode::BAD_REQUEST => ErrorKind::InvalidRequest,
        StatusCode::UNAUTHORIZED => ErrorKind::Authentication,
        StatusCode::FORBIDDEN => ErrorKind::Permission,
        StatusCode::NOT_FOUND => ErrorKind::NotFound,
        StatusCode::PAYLOAD_TOO_LARGE => ErrorKind::RequestTooLarge,
        StatusCode::TOO_MANY_REQUESTS => ErrorKind::RateLimit,
        other => ErrorKind::Unknown(other.as_u16().to_string()),
    }
}

#[cfg(test)]
mod tests {
    use markhor_core::{
        chat::{MemberId, common::ToolCallRequest},
        dependencies::ApiKey,
        extension::ModelId,
    };
    use serde_json::json;
    use wiremock::{
        Mock, MockServer, ResponseTemplate,
        matchers::{body_json, header, method, path},
    };

    use super::*;
    use crate::anthropic::ClientConfig;

    fn user(text: &str) -> Msg {
        Msg::User {
            sender: MemberId::new("alice", true),
            timestamp: None,
            content: vec![ContentPart::Text(text.into())],
        }
    }

    fn agent(text: &str, tool_calls: Vec<ToolCallRequest>) -> Msg {
        Msg::Agent {
            sender: MemberId::new("claude", false),
            timestamp: None,
            content: vec![ContentPart::Text(text.into())],
            tool_calls,
            model: ModelId::new("m", "p"),
            reasoning_content: None,
            metadata: json!(null),
        }
    }

    fn build(messages: &[Msg], options: &ChatOptions) -> Result<serde_json::Value, ChatError> {
        build_request("claude-test", 4096, messages, options)
            .map(|r| serde_json::to_value(r).unwrap())
    }

    fn client_for(server: &MockServer) -> ChatClient {
        ChatClient {
            inner: Arc::new(ClientInner {
                api_key: ApiKey::new("test-key".into(), "anthropic".into(), "proj".into()),
                config: Arc::new(ClientConfig {
                    api_base_url: Some(format!("{}/v1/", server.uri())),
                    ..Default::default()
                }),
                http_client: Arc::new(reqwest::Client::new()),
            }),
            model_name: "claude-test".into(),
        }
    }

    mod request {
        use super::*;

        #[test]
        fn system_messages_become_system_blocks() {
            let request = build(
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
                request,
                json!({
                    "model": "claude-test",
                    "max_tokens": 10,
                    "temperature": 0.5,
                    "system": [{"type": "text", "text": "be nice"}, {"type": "text", "text": "be brief"}],
                    "messages": [{"role": "user", "content": [{"type": "text", "text": "hi"}]}],
                })
            );
        }

        #[test]
        fn max_tokens_defaults_when_unset() {
            let request = build(&[user("hi")], &ChatOptions::default()).unwrap();
            assert_eq!(request["max_tokens"], 4096);
        }

        #[test]
        fn same_role_messages_are_merged_and_blank_text_dropped() {
            let request = build(
                &[
                    user("a"),
                    Msg::Event {
                        recipients: Default::default(),
                        timestamp: None,
                        source: "x".into(),
                        content: json!(null),
                    },
                    user("  "),
                    user("b"),
                    // Not first-person, so it speaks as a user too.
                    agent("c", vec![]),
                ],
                &ChatOptions::default(),
            )
            .unwrap();
            assert_eq!(
                request["messages"],
                json!([{"role": "user", "content": [
                    {"type": "text", "text": "a"},
                    {"type": "text", "text": "b"},
                    {"type": "text", "text": "c"},
                ]}])
            );
        }

        // `MemberId`s can't be flagged as first-person outside of `markhor_core` yet,
        // so the assistant path is exercised through the helpers directly.
        #[test]
        fn merging_assistant_messages() {
            let text = |s: &str| [ContentPart::Text(s.into())];
            let mut history = Vec::new();
            push_user_blocks(&mut history, &text("q")).unwrap();
            push_assistant_blocks(&mut history, &text("a"));
            push_assistant_blocks(&mut history, &text("  "));
            push_assistant_blocks(&mut history, &text("b"));
            push_user_blocks(&mut history, &text("r")).unwrap();
            assert_eq!(
                serde_json::to_value(&history).unwrap(),
                json!([
                    {"role": "user", "content": [{"type": "text", "text": "q"}]},
                    {"role": "assistant", "content": [
                        {"type": "text", "text": "a"}, {"type": "text", "text": "b"}]},
                    {"role": "user", "content": [{"type": "text", "text": "r"}]},
                ])
            );
        }

        #[test]
        fn other_agents_speak_as_user() {
            let request = build(
                &[user("hi"), agent("hello", vec![])],
                &ChatOptions::default(),
            )
            .unwrap();
            assert_eq!(request["messages"].as_array().unwrap().len(), 1);
        }

        #[test]
        fn images_are_base64_encoded() {
            let request = build(
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
                request["messages"][0]["content"][0],
                json!({"type": "image", "source": {
                    "type": "base64", "media_type": "image/png", "data": "AQID"}})
            );
        }

        #[test]
        fn unsupported_image_type_is_invalid() {
            let error = build(
                &[Msg::User {
                    sender: MemberId::new("alice", true),
                    timestamp: None,
                    content: vec![ContentPart::Image {
                        mime_type: "image/tiff".into(),
                        data: vec![],
                    }],
                }],
                &ChatOptions::default(),
            )
            .unwrap_err();
            assert!(matches!(error, ChatError::InvalidRequest(_)), "{error:?}");
        }

        #[test]
        fn requires_a_user_message() {
            let error = build(
                &[Msg::Platform("x".into()), user("  ")],
                &ChatOptions::default(),
            )
            .unwrap_err();
            assert!(matches!(error, ChatError::InvalidRequest(_)), "{error:?}");
        }

        #[test]
        fn tool_calls_are_not_supported() {
            let error = build(
                &[
                    user("hi"),
                    agent(
                        "",
                        vec![ToolCallRequest {
                            id: "1".into(),
                            name: "f".into(),
                            arguments: json!({}),
                        }],
                    ),
                ],
                &ChatOptions::default(),
            )
            .unwrap_err();
            assert!(matches!(error, ChatError::NotSupported(_)), "{error:?}");
        }
    }

    mod response {
        use super::*;

        fn parse(value: serde_json::Value) -> MessagesResponse {
            serde_json::from_value(value).unwrap()
        }

        #[test]
        fn text_is_kept_and_thinking_dropped() {
            let response = convert_response(parse(json!({
                "id": "msg_1", "model": "claude-test",
                "content": [
                    {"type": "thinking", "thinking": "hmm", "signature": "sig"},
                    {"type": "redacted_thinking", "data": "xyz"},
                    {"type": "text", "text": "Paris"},
                    {"type": "server_tool_use", "id": "x", "name": "web_search", "input": {}},
                ],
                "stop_reason": "max_tokens",
                "usage": {"input_tokens": 7, "output_tokens": 3},
            })))
            .unwrap();
            assert_eq!(response.content, [ContentPart::Text("Paris".into())]);
            assert_eq!(
                response.finish_reason,
                Some(markhor_core::chat::FinishReason::Length)
            );
            assert_eq!(response.usage.unwrap().total_tokens, Some(10));
            assert_eq!(response.model_id.as_deref(), Some("claude-test"));
        }

        #[test]
        fn empty_refusal_is_content_moderated() {
            let error = convert_response(parse(json!({
                "id": "msg_1", "model": "claude-test", "content": [],
                "stop_reason": "refusal",
                "usage": {"input_tokens": 1, "output_tokens": 0},
            })))
            .unwrap_err();
            assert!(matches!(error, ChatError::ContentModerated(_)), "{error:?}");
        }
    }

    mod errors {
        use super::*;

        fn body(kind: &str) -> Vec<u8> {
            json!({"type": "error", "error": {"type": kind, "message": "boom"}, "request_id": "req_9"})
                .to_string()
                .into_bytes()
        }

        fn map(status: u16, kind: &str) -> ChatError {
            map_error_response(StatusCode::from_u16(status).unwrap(), &body(kind), "m")
        }

        #[test]
        fn maps_each_kind() {
            assert!(matches!(
                map(401, "authentication_error"),
                ChatError::Authentication(_)
            ));
            assert!(matches!(
                map(403, "permission_error"),
                ChatError::Authentication(_)
            ));
            assert!(
                matches!(map(404, "not_found_error"), ChatError::ModelNotFound(m) if m.starts_with("m: boom"))
            );
            assert!(matches!(
                map(429, "rate_limit_error"),
                ChatError::RateLimited
            ));
            assert!(
                matches!(map(400, "invalid_request_error"), ChatError::InvalidRequest(m) if m.contains("req_9"))
            );
            assert!(matches!(
                map(413, "request_too_large"),
                ChatError::InvalidRequest(_)
            ));
            for (status, kind) in [
                (402, "billing_error"),
                (500, "api_error"),
                (504, "timeout_error"),
                (529, "overloaded_error"),
                (500, "something_new"),
            ] {
                assert!(
                    matches!(map(status, kind), ChatError::Api { status: Some(s), .. } if s == status),
                    "{kind}"
                );
            }
        }

        #[test]
        fn falls_back_to_status_for_non_json_bodies() {
            let map = |status: u16| {
                map_error_response(StatusCode::from_u16(status).unwrap(), b"<html>", "m")
            };
            assert!(matches!(map(401), ChatError::Authentication(_)));
            assert!(matches!(map(429), ChatError::RateLimited));
            assert!(matches!(
                map(502),
                ChatError::Api {
                    status: Some(502),
                    source: Some(_),
                    ..
                }
            ));
        }
    }

    mod http {
        use super::*;

        fn ok_body() -> serde_json::Value {
            json!({
                "id": "msg_1", "type": "message", "role": "assistant", "model": "claude-test",
                "content": [{"type": "text", "text": "Paris"}],
                "stop_reason": "end_turn", "stop_sequence": null,
                "usage": {"input_tokens": 5, "output_tokens": 2},
            })
        }

        #[tokio::test]
        async fn generate_posts_request_and_parses_response() {
            let server = MockServer::start().await;
            Mock::given(method("POST"))
                .and(path("/v1/messages"))
                .and(header("x-api-key", "test-key"))
                .and(header("anthropic-version", "2023-06-01"))
                .and(body_json(json!({
                    "model": "claude-test",
                    "max_tokens": 4096,
                    "messages": [{"role": "user", "content": [{"type": "text", "text": "Capital of France?"}]}],
                })))
                .respond_with(ResponseTemplate::new(200).set_body_json(ok_body()))
                .expect(1)
                .mount(&server)
                .await;

            let response = client_for(&server)
                .generate(&[user("Capital of France?")], &ChatOptions::default())
                .await
                .unwrap();
            assert_eq!(response.content, [ContentPart::Text("Paris".into())]);
            assert_eq!(
                response.finish_reason,
                Some(markhor_core::chat::FinishReason::Stop)
            );
        }

        #[tokio::test]
        async fn generate_retries_without_rejected_temperature() {
            let server = MockServer::start().await;
            // Mocks are matched in mount order; the first only matches requests with temperature.
            Mock::given(method("POST"))
                .and(wiremock::matchers::body_partial_json(
                    json!({"temperature": 0.0}),
                ))
                .respond_with(ResponseTemplate::new(400).set_body_json(json!({
                    "type": "error",
                    "error": {
                        "type": "invalid_request_error",
                        "message": "`temperature` is not supported for this model",
                    },
                })))
                .expect(1)
                .mount(&server)
                .await;
            Mock::given(method("POST"))
                .respond_with(ResponseTemplate::new(200).set_body_json(ok_body()))
                .expect(1)
                .mount(&server)
                .await;

            let response = client_for(&server)
                .generate(
                    &[user("hi")],
                    &ChatOptions {
                        temperature: Some(0.0),
                        max_tokens: None,
                    },
                )
                .await
                .unwrap();
            assert_eq!(response.content, [ContentPart::Text("Paris".into())]);
        }

        #[tokio::test]
        async fn generate_maps_api_errors() {
            let server = MockServer::start().await;
            Mock::given(method("POST"))
                .respond_with(ResponseTemplate::new(429).set_body_json(json!({
                    "type": "error",
                    "error": {"type": "rate_limit_error", "message": "slow down"},
                })))
                .mount(&server)
                .await;

            let error = client_for(&server)
                .generate(&[user("hi")], &ChatOptions::default())
                .await
                .unwrap_err();
            assert!(matches!(error, ChatError::RateLimited), "{error:?}");
        }

        #[tokio::test]
        async fn generate_reports_malformed_success_bodies() {
            let server = MockServer::start().await;
            Mock::given(method("POST"))
                .respond_with(ResponseTemplate::new(200).set_body_string("not json"))
                .mount(&server)
                .await;

            let error = client_for(&server)
                .generate(&[user("hi")], &ChatOptions::default())
                .await
                .unwrap_err();
            assert!(matches!(error, ChatError::Parsing(_)), "{error:?}");
        }
    }
}
