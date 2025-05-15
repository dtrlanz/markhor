use async_trait::async_trait;
use thiserror::Error;

use crate::job::AssetSender;

/// Trait for prompting user input.
/// 
/// This trait abstracts the input method for different platforms.
#[async_trait]
pub trait Prompter: Send + Sync {
    /// Prompts the user for input and returns the result as a String.
    async fn prompt(&self, message: &str) -> Result<String, PromptError>;

    /// Assigns an asset sender to the prompter, allowing it to send assets (typically files 
    /// attached by the user).
    fn set_asset_sender(&mut self, sender: Option<AssetSender>) -> Result<(), PromptError> {
        if sender.is_none() {
            return Ok(());
        } else {
            Err(PromptError::FeatureNotSupported(
                "attaching assets".to_string(),
            ))
        }
    }
}


#[derive(Debug, Error)]
pub enum PromptError {
    #[error("Prompt was canceled by the user.")]
    Canceled,

    #[error("Prompt input failed due to IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Prompt failed to execute to completion: {0}")]
    Async(#[from] tokio::task::JoinError),

    #[error("Prompter does not support {0}")]
    FeatureNotSupported(String),
}
