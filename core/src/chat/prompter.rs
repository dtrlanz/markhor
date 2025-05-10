use async_trait::async_trait;
use thiserror::Error;

/// Trait for prompting user input.
/// 
/// This trait abstracts the input method for different platforms.
#[async_trait]
pub trait Prompter: Send + Sync {
    /// Prompts the user for input and returns the result as a String.
    async fn prompt(&self, message: &str) -> Result<String, PromptError>;
}


#[derive(Debug, Error)]
pub enum PromptError {
    #[error("Prompt input failed due to IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Prompt failed to execute to completion: {0}")]
    Async(#[from] tokio::task::JoinError),
}
