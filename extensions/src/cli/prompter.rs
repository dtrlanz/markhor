use async_trait::async_trait;
use dialoguer::{Input, theme::ColorfulTheme};

use markhor_core::chat::prompter::{Prompter, PromptError};

pub struct ConsolePrompter;

#[async_trait]
impl Prompter for ConsolePrompter {
    async fn prompt(&self, message: &str) -> Result<String, PromptError> {
        let message_clone = message.to_string();
        let result = tokio::task::spawn_blocking(move || {
            Input::<String>::with_theme(&ColorfulTheme::default())
                .with_prompt(message_clone)
                .allow_empty(true)
                .interact_text()
                .map_err(|e| match e {
                    dialoguer::Error::IO(err) => PromptError::Io(err),
                })
        }).await?;

        if result.as_ref().is_ok_and(|s| s == "") {
            Err(PromptError::Canceled)
        } else {
            result
        }
    }
}

