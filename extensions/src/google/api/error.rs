use serde::{Deserialize, Serialize};

/// Body returned by the API for any non-2xx response.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub(crate) struct ErrorResponse {
    pub error: ApiError,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub(crate) struct ApiError {
    /// HTTP status code.
    pub code: Option<u16>,
    #[serde(default)]
    pub message: String,
    pub status: Option<ErrorStatus>,
    #[serde(default)]
    pub details: Vec<ErrorDetail>,
}

impl ApiError {
    /// Google reports a bad API key as `400 INVALID_ARGUMENT` rather than `401`.
    pub fn is_invalid_api_key(&self) -> bool {
        self.details.iter().any(|detail| {
            matches!(detail, ErrorDetail::ErrorInfo { reason } if reason == "API_KEY_INVALID")
        })
    }
}

/// Structured error details, discriminated by the `@type` field.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(tag = "@type")]
pub(crate) enum ErrorDetail {
    #[serde(rename = "type.googleapis.com/google.rpc.ErrorInfo")]
    ErrorInfo {
        #[serde(default)]
        reason: String,
    },
    #[serde(other)]
    Other,
}

/// The canonical gRPC status name (`google.rpc.Code`) accompanying an error.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub(crate) enum ErrorStatus {
    InvalidArgument,
    FailedPrecondition,
    PermissionDenied,
    NotFound,
    ResourceExhausted,
    Unauthenticated,
    Cancelled,
    Internal,
    Unavailable,
    DeadlineExceeded,
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
            "error": {
                "code": 400,
                "message": "API key not valid. Please pass a valid API key.",
                "status": "INVALID_ARGUMENT",
                "details": [{"@type": "type.googleapis.com/google.rpc.ErrorInfo"}]
            }
        }))
        .unwrap();
        assert_eq!(response.error.code, Some(400));
        assert_eq!(response.error.status, Some(ErrorStatus::InvalidArgument));
    }

    #[test]
    fn invalid_api_key_is_detected_from_details() {
        let response: ErrorResponse = serde_json::from_value(json!({
            "error": {
                "code": 400,
                "message": "API key not valid. Please pass a valid API key.",
                "status": "INVALID_ARGUMENT",
                "details": [
                    {"@type": "type.googleapis.com/google.rpc.Help", "links": []},
                    {
                        "@type": "type.googleapis.com/google.rpc.ErrorInfo",
                        "reason": "API_KEY_INVALID",
                        "domain": "googleapis.com"
                    }
                ]
            }
        }))
        .unwrap();
        assert_eq!(response.error.details[0], ErrorDetail::Other);
        assert!(response.error.is_invalid_api_key());
    }

    #[test]
    fn unknown_status_is_preserved() {
        let status: ErrorStatus = serde_json::from_value(json!("SOMETHING_ELSE")).unwrap();
        assert_eq!(status, ErrorStatus::Unknown("SOMETHING_ELSE".into()));
    }
}
