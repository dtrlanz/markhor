
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

pub trait ChatModel {
    async fn generate(&self, messages: Vec<ChatMessage>) -> Result<String, ChatError>;
}

pub struct ChatMessage {
    pub role: ChatMessageRole,
    pub content: String,
}

pub enum ChatMessageRole {
    System,
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
