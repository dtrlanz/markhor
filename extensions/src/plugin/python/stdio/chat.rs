use markhor_core::chat::{Message, MessageRole, UsageData};
use serde::{Deserialize, Serialize};
use std::collections::HashMap; // For using arbitrary config/usage fields

// --- Data structures specific to chat functionality ---

// Input structure expected by the *plugin script's* 'chat' handler
#[derive(Serialize, Debug)]
pub(crate) struct ChatParams<'a> {
    pub messages: &'a [ChatMessage],
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<&'a str>,
    #[serde(skip_serializing_if = "HashMap::is_empty")]
    pub config: HashMap<String, serde_json::Value>, // Allow flexible config
}

// Output structure returned in the 'result' field for a successful 'chat' call
#[derive(Deserialize, Debug, Clone)]
pub(crate) struct ChatResult {
    pub response: ChatMessage,
    #[serde(default)] // Handle cases where usage might be missing
    pub usage: UsageData,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct ChatMessage {
    pub role: String, // Typically "user", "model", "system"
    pub content: String,
}

impl From<&Message> for ChatMessage {
    fn from(msg: &Message) -> Self {
        Self {
            role: match msg.role {
                MessageRole::Developer => "system".to_string(),
                MessageRole::User => "user".to_string(),
                MessageRole::Assistant => "model".to_string(),
            },
            content: msg.content.clone(),
        }
    }
}
