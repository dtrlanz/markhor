use std::sync::Arc;
use thiserror::Error;

use markhor_core::{
    chat::{ChatModel, ChatUi, Group},
    dependencies::{ResolveDependencyError, Session},
    extension::Comp,
};

use crate::{
    agent::{Agent, AgentBuilder, InitAgentError},
    user::{InitUserError, User, UserBuilder},
};

pub struct Simple {
    session: Session,
    group: Arc<Group>,
    user: User,
    assistant: Agent,
}

impl Simple {
    pub fn new(session: Session) -> Result<Self, InitChatError> {
        Simple::with_user_id(session, "User")
    }

    pub fn with_user_id(session: Session, user_id: &str) -> Result<Self, InitChatError> {
        let group = Group::new();
        let ui: Comp<dyn ChatUi> = session.resolve()?;
        let model: Comp<dyn ChatModel> = session.resolve()?;

        let user = UserBuilder::new(&group, ui).id(user_id).spawn()?;
        let assistant = AgentBuilder::new(&group, model).id("Assistant").spawn()?;

        Ok(Self {
            session,
            group,
            user,
            assistant,
        })
    }
}

#[derive(Error, Debug)]
pub enum InitChatError {
    #[error("Error initializing chat: {0}")]
    User(#[from] InitUserError),

    #[error("Error initializing chat: {0}")]
    Agent(#[from] InitAgentError),

    #[error("Dependency unavailable: {0}")]
    Dependency(#[from] ResolveDependencyError),
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_utils::session_with_mocks;
    use markhor_core::chat::{AddMemberError, MemberId};

    fn is_taken(group: &Arc<Group>, id: &str) -> bool {
        matches!(
            group.add_member(MemberId::new(id, false)),
            Err(AddMemberError::AlreadyExists(_))
        )
    }

    #[tokio::test]
    async fn with_user_id_registers_user_and_assistant() {
        let chat = Simple::with_user_id(session_with_mocks().await, "alice").unwrap();
        assert!(is_taken(&chat.group, "alice"));
        assert!(is_taken(&chat.group, "Assistant"));
    }

    #[test]
    fn missing_chat_ui_is_reported_as_dependency_error() {
        let result = Simple::new(Session::new());
        assert!(matches!(result, Err(InitChatError::Dependency(_))));
    }
}
