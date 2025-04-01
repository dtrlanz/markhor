
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
use std::fmt;

#[dynosaur::dynosaur(pub DynChatModel)]
pub trait ChatModel: Send + Sync {
    fn generate(&self, messages: &Vec<Message>) -> impl Future<Output = Result<String, ChatError>> + Send;
}

pub struct Message {
    pub role: ChatMessageRole,
    pub content: String,
}

impl Message {
    pub fn developer<T: Into<String>>(content: T) -> Self {
        Self {
            role: ChatMessageRole::Developer,
            content: content.into(),
        }
    }
    pub fn user<T: Into<String>>(content: T) -> Self {
        Self {
            role: ChatMessageRole::User,
            content: content.into(),
        }
    }
    pub fn assistant<T: Into<String>>(content: T) -> Self {
        Self {
            role: ChatMessageRole::Assistant,
            content: content.into(),
        }
    }
}

pub enum ChatMessageRole {
    Developer,
    User,
    Assistant,
}

#[derive(Debug, Error)]
pub enum ChatError {
    InvalidMessageFormat,
    ModelError(String),
    // Add other error types as needed
}

impl fmt::Display for ChatError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{:?}", self)
    }
}
