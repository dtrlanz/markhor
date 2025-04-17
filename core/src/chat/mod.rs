pub mod chat;
pub mod error;

pub use error::ChatError;


// Implementation Notes:

//     Model Adapters:
//         Create adapter structs for each supported chat model (e.g., OpenAIChatModel, LocalChatModel).
//         Implement the ChatModel trait for each adapter struct.
//         Use appropriate API clients or libraries to interact with the underlying models.

//     Context Management:
//         The ChatModel trait itself does not handle context management.
//         Context management will be handled at a higher level (e.g., in the chat session or workflow logic).

//     Streaming (Future):
//         Consider adding a generate_stream method to the ChatModel trait for streaming capabilities.
//         Use asynchronous streams (e.g., tokio::stream::Stream) to represent streaming responses.

use thiserror::Error;
use std::{collections::HashMap, fmt};
use serde::{Deserialize, Serialize};
use async_trait::async_trait;

use crate::extension::Functionality;

#[async_trait]
pub trait ChatModel: Functionality + Send + Sync {
    // We have temporariliy ended up with two overlapping methods in this trait because it made
    // sense to return more than just a string result (e.g., usage data) but I didn't want to
    // update existing tests immediately.
    // Todo: Resolve/consolidate when the API is a bit more settled.

    async fn generate(&self, messages: &Vec<Message>) -> Result<String, ChatError> {
        self.chat(messages, None, None).await.map(|completion| {
            completion.message.content
        })
    }


    async fn chat(
        &self,
        messages: &[Message],
        model: Option<&str>,
        config: Option<HashMap<String, serde_json::Value>>,
    ) -> Result<Completion, ChatError>;
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct Message {
    pub role: MessageRole,
    pub content: String,
}

impl Message {
    pub fn developer<T: Into<String>>(content: T) -> Self {
        Self {
            role: MessageRole::Developer,
            content: content.into(),
        }
    }
    pub fn user<T: Into<String>>(content: T) -> Self {
        Self {
            role: MessageRole::User,
            content: content.into(),
        }
    }
    pub fn assistant<T: Into<String>>(content: T) -> Self {
        Self {
            role: MessageRole::Assistant,
            content: content.into(),
        }
    }
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MessageRole {
    Developer,
    User,
    Assistant,
}

// Represents the final successful output of a chat operation
#[derive(Debug, Clone)]
pub struct Completion {
    pub message: Message,
    pub usage: Option<UsageData>,
    // Add other metadata like finish reason, model name, etc. if needed
}

#[derive(Deserialize, Debug, Clone, Default)]
pub struct UsageData {
    pub prompt_token_count: Option<u32>,
    pub candidates_token_count: Option<u32>,
    pub total_token_count: Option<u32>,
    // Add other usage fields if needed
}


// #[derive(Debug, Error)]
// pub enum ChatError {
//     #[error("Invalid message format")]
//     InvalidMessageFormat,

//     #[error("Model error: {0}")]
//     ModelError(String),

//     #[error("Error while using plugin: {0}")]
//     PluginError(Box<dyn std::error::Error + Send + Sync>),

//     #[error("Other error: {0}")]
//     Other(Box<dyn std::error::Error + Send + Sync>),
// }