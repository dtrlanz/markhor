use reqwest::StatusCode;
use serde::Deserialize;
use serde_json::Value;

use crate::mistral::Error;

/// Body returned by the API for a non-2xx response.
///
/// Mistral uses several shapes: `{"object": "error", "message": ..., "type": ..., "code": ...}`
/// (where `message` is usually a string but can be a nested value), and FastAPI-style
/// `{"detail": ...}` for validation errors (422), where `detail` is a list of
/// `{"loc": [...], "msg": ...}` items or occasionally a plain string. All fields are therefore
/// optional and loosely typed.
#[derive(Debug, Deserialize)]
pub(crate) struct ErrorResponse {
    #[serde(default)]
    message: Option<Value>,
    #[serde(default)]
    detail: Option<Value>,
    #[serde(default, rename = "type")]
    kind: Option<String>,
}

impl ErrorResponse {
    /// Builds an [`Error`] from a failed response, falling back to the raw body text when it
    /// is not a recognizable error body.
    pub(crate) fn into_error(status: StatusCode, body: &[u8]) -> Error {
        let parsed = serde_json::from_slice::<ErrorResponse>(body).ok();
        let kind = parsed.as_ref().and_then(|e| e.kind.clone());
        let message = parsed
            .and_then(|e| e.message())
            .unwrap_or_else(|| String::from_utf8_lossy(body).into_owned());
        Error::Api {
            status,
            message,
            kind,
        }
    }

    fn message(&self) -> Option<String> {
        self.message
            .as_ref()
            .or(self.detail.as_ref())
            .map(describe)
            .filter(|m| !m.is_empty())
    }
}

/// Renders a `message`/`detail` value as human-readable text.
fn describe(value: &Value) -> String {
    match value {
        Value::String(s) => s.clone(),
        Value::Array(items) => items.iter().map(describe).collect::<Vec<_>>().join("; "),
        Value::Object(map) => {
            if let Some(msg) = map.get("msg").and_then(Value::as_str) {
                match map.get("loc").and_then(Value::as_array) {
                    Some(loc) if !loc.is_empty() => {
                        let loc = loc.iter().map(describe).collect::<Vec<_>>().join(".");
                        format!("{loc}: {msg}")
                    }
                    _ => msg.to_owned(),
                }
            } else if let Some(inner) = map.get("detail").or_else(|| map.get("message")) {
                describe(inner)
            } else {
                value.to_string()
            }
        }
        other => other.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(status: u16, body: &str) -> (String, Option<String>) {
        match ErrorResponse::into_error(StatusCode::from_u16(status).unwrap(), body.as_bytes()) {
            Error::Api { message, kind, .. } => (message, kind),
            other => panic!("unexpected error: {other:?}"),
        }
    }

    #[test]
    fn parses_standard_error_body() {
        let (message, kind) = parse(
            401,
            r#"{"object":"error","message":"Unauthorized","type":"invalid_request_error","param":null,"code":null}"#,
        );
        assert_eq!(message, "Unauthorized");
        assert_eq!(kind.as_deref(), Some("invalid_request_error"));
    }

    #[test]
    fn parses_validation_error_body() {
        let (message, _) = parse(
            422,
            r#"{"detail":[{"loc":["body","document"],"msg":"Field required","type":"missing"}]}"#,
        );
        assert_eq!(message, "body.document: Field required");
    }

    #[test]
    fn falls_back_to_raw_body() {
        let (message, kind) = parse(502, "<html>Bad Gateway</html>");
        assert_eq!(message, "<html>Bad Gateway</html>");
        assert_eq!(kind, None);
    }
}
