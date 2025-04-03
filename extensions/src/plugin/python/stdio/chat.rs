use markhor_core::chat::{Message, MessageRole, UsageData};
use serde::{Deserialize, Serialize};
use std::{collections::HashMap, sync::Arc}; // For using arbitrary config/usage fields

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



use markhor_core::chat::{ChatError, ChatModel, Completion};
use crate::plugin::python::stdio::error::PluginError;
use super::wrapper::StdioWrapper;


pub struct PythonStdioChatModel {
    stdio_wrapper: Arc<StdioWrapper>,
}

impl PythonStdioChatModel {
    pub fn new(stdio_manager: &Arc<StdioWrapper>) -> Self {
        Self { 
            stdio_wrapper: stdio_manager.clone(),
        }
    }
}



impl ChatModel for PythonStdioChatModel {
    async fn chat(
        &self,
        messages: &[Message],
        model: Option<&str>,
        config: Option<HashMap<String, serde_json::Value>>,
    ) -> Result<Completion, ChatError> {
        tracing::debug!("Calling chat method on plugin '{}'", self.stdio_wrapper.plugin_name);
        let converted_messages: Vec<ChatMessage> = messages.iter().map(|m| m.into()).collect();
        let params = ChatParams {
            messages: &*converted_messages,
            model,
            config: config.unwrap_or_default(),
        };

        let result: ChatResult = self.stdio_wrapper.run_method("chat", params).await.map_err(|e| e.into())?;
        let result_message = Message {
            role: match result.response.role.as_str() {
                "user" => MessageRole::User,
                "model" => MessageRole::Assistant,
                "system" => MessageRole::Developer,
                _ => Err(PluginError::ResponseInvalid(format!("Unknown role: {}", result.response.role)).into())?,
            },
            content: result.response.content,
        };

        Ok(Completion {
            message: result_message,
            usage: Some(result.usage),
        })
    }
}
