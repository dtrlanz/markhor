use std::sync::Arc;

use async_trait::async_trait;
use markhor_core::chat::{ChatError, ChatModel, ChatOptions, ChatResponse, Msg};

use crate::google::ClientInner;

#[derive(Debug)]
pub struct ChatClient {
    pub(super) inner: Arc<ClientInner>,
    pub(super) model_name: String,
}

#[async_trait]
impl ChatModel for ChatClient {
    fn model_name(&self) -> &str {
        &self.model_name
    }

    async fn generate(
        &self,
        messages: &[Msg],
        options: &ChatOptions,
    ) -> Result<ChatResponse, ChatError> {
        todo!()
    }
}
