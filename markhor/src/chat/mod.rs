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

    /// Runs the chat until the user ends input, handing each command to `on_command`.
    ///
    /// Commands are the `(name, args)` pairs the UI recognized. Messages are
    /// answered by the agents and do not reach `on_command`.
    pub async fn run(&self, mut on_command: impl FnMut(&str, &str)) {
        while let Some((name, args)) = self.user.next_command().await {
            on_command(&name, &args);
        }
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
    use crate::test_utils::{ScriptedChatUi, session_with_mocks, session_with_ui};
    use markhor_core::chat::{AddMemberError, MemberId, UserInput};

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

    #[tokio::test]
    async fn run_hands_commands_to_handler_until_end() {
        let scripted = ScriptedChatUi::new(&[]);
        scripted.push_input(UserInput::Command {
            name: "model".into(),
            args: "gemma-4-31b-it".into(),
        });
        scripted.push_input(UserInput::Command {
            name: "help".into(),
            args: String::new(),
        });
        scripted.push_input(UserInput::End);
        let chat = Simple::with_user_id(session_with_ui(&scripted).await, "alice").unwrap();

        let mut seen = Vec::new();
        tokio::time::timeout(
            std::time::Duration::from_secs(5),
            chat.run(|name, args| seen.push((name.to_string(), args.to_string()))),
        )
        .await
        .expect("run did not return after the user ended input");

        let expected = vec![
            ("model".to_string(), "gemma-4-31b-it".to_string()),
            ("help".to_string(), String::new()),
        ];
        assert_eq!(seen, expected);
    }

    #[test]
    fn missing_chat_ui_is_reported_as_dependency_error() {
        let result = Simple::new(Session::new());
        assert!(matches!(result, Err(InitChatError::Dependency(_))));
    }
}
