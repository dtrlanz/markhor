use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;
use std::borrow::Borrow;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ContentPart {
    Text(String),
    Image {
        mime_type: String,
        data: Vec<u8>,
    },
}

impl ContentPart {
    pub fn into_text(self) -> Option<String> {
        match self {
            ContentPart::Text(text) => Some(text),
            _ => None,
        }
    }
}

impl Borrow<str> for ContentPart {
    fn borrow(&self) -> &str {
        match self {
            ContentPart::Text(s) => &*s,
            ContentPart::Image { .. } => "[Image]",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolResult {
    /// An id for matching tool requests to results. 
    /// 
    /// Should uniquely identify a tool call within a list of messages.
    pub call_id: String, 

    /// The name of the function/tool to call.
    pub name: String,

    /// The result of the function call.
    pub content: serde_json::Value,
}

/// Represents a request from the model to call a specific tool.
/// This is typically part of the `ChatResponse`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolCallRequest {
    /// Unique identifier for this specific tool call instance.
    /// Needed to match the `ToolResult` back.
    pub id: String,

    /// The name of the function/tool to call.
    pub name: String,
    
    /// The arguments to pass to the function.
    pub arguments: JsonValue,
}
