use serde::{Deserialize, Serialize};

/// Body returned by the API for any non-2xx response.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub(crate) struct ErrorResponse {
    pub error: ApiError,
    /// Identifier to quote when contacting support.
    #[serde(default)]
    pub request_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub(crate) struct ApiError {
    #[serde(rename = "type")]
    pub kind: ErrorKind,
    #[serde(default)]
    pub message: String,
}

/// The `error.type` discriminator, which corresponds to the HTTP status code.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum ErrorKind {
    /// 400 (also used for other unlisted 4xx codes and when a spend limit is reached).
    #[serde(rename = "invalid_request_error")]
    InvalidRequest,
    /// 401
    #[serde(rename = "authentication_error")]
    Authentication,
    /// 402
    #[serde(rename = "billing_error")]
    Billing,
    /// 403
    #[serde(rename = "permission_error")]
    Permission,
    /// 404
    #[serde(rename = "not_found_error")]
    NotFound,
    /// 409
    #[serde(rename = "conflict_error")]
    Conflict,
    /// 413
    #[serde(rename = "request_too_large")]
    RequestTooLarge,
    /// 429
    #[serde(rename = "rate_limit_error")]
    RateLimit,
    /// 500
    #[serde(rename = "api_error")]
    Api,
    /// 504
    #[serde(rename = "timeout_error")]
    Timeout,
    /// 529
    #[serde(rename = "overloaded_error")]
    Overloaded,
    #[serde(untagged)]
    Unknown(String),
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn error_response_deserializes() {
        let response: ErrorResponse = serde_json::from_value(json!({
            "type": "error",
            "error": {
                "type": "not_found_error",
                "message": "model: claude-nope"
            },
            "request_id": "req_011CSHoEeqs5C35K2UUqR7Fy"
        }))
        .unwrap();
        assert_eq!(response.error.kind, ErrorKind::NotFound);
        assert_eq!(response.error.message, "model: claude-nope");
        assert_eq!(
            response.request_id.as_deref(),
            Some("req_011CSHoEeqs5C35K2UUqR7Fy")
        );
    }

    #[test]
    fn request_id_is_optional() {
        let response: ErrorResponse = serde_json::from_value(json!({
            "error": {"type": "overloaded_error", "message": "Overloaded"}
        }))
        .unwrap();
        assert_eq!(response.error.kind, ErrorKind::Overloaded);
        assert_eq!(response.request_id, None);
    }

    #[test]
    fn all_documented_kinds_parse() {
        let cases = [
            ("invalid_request_error", ErrorKind::InvalidRequest),
            ("authentication_error", ErrorKind::Authentication),
            ("billing_error", ErrorKind::Billing),
            ("permission_error", ErrorKind::Permission),
            ("not_found_error", ErrorKind::NotFound),
            ("conflict_error", ErrorKind::Conflict),
            ("request_too_large", ErrorKind::RequestTooLarge),
            ("rate_limit_error", ErrorKind::RateLimit),
            ("api_error", ErrorKind::Api),
            ("timeout_error", ErrorKind::Timeout),
            ("overloaded_error", ErrorKind::Overloaded),
        ];
        for (raw, expected) in cases {
            assert_eq!(
                serde_json::from_value::<ErrorKind>(json!(raw)).unwrap(),
                expected
            );
        }
    }

    #[test]
    fn unknown_kind_is_preserved() {
        let kind: ErrorKind = serde_json::from_value(json!("quota_error")).unwrap();
        assert_eq!(kind, ErrorKind::Unknown("quota_error".into()));
    }
}
