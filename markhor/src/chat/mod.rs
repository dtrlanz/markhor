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
        Self::with_commands(session, user_id, Vec::new())
    }

    /// Like [`Simple::with_user_id`], but offers `commands` to the UI.
    pub fn with_commands(
        session: Session,
        user_id: &str,
        commands: Vec<String>,
    ) -> Result<Self, InitChatError> {
        let group = Group::new();
        let ui: Comp<dyn ChatUi> = session.resolve()?;
        let model: Comp<dyn ChatModel> = session.resolve()?;

        let user = UserBuilder::new(&group, ui)
            .id(user_id)
            .commands(commands)
            .spawn()?;
        let assistant = AgentBuilder::new(&group, model).id("Assistant").spawn()?;

        Ok(Self {
            session,
            group,
            user,
            assistant,
        })
    }

    /// Runs the chat until the user ends input or `on_command` returns `false`.
    ///
    /// Commands are the `(name, args)` pairs the UI recognized. Messages are
    /// answered by the agents and do not reach `on_command`.
    pub async fn run(&self, mut on_command: impl FnMut(&str, &str) -> bool) {
        while let Some((name, args)) = self.user.next_command().await {
            if !on_command(&name, &args) {
                break;
            }
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
        // NB: Passing commands not really necessary for test to pass because
        // the scripted UI already returns commands. Normally `Simple` would
        // just pass the list on to the UI, which would then recognize them
        // from user input. But we're not covering `Simple`'s implementation
        // details, so just constructing as you would normally.
        let commands = vec!["model".to_string(), "help".to_string()];
        let chat =
            Simple::with_commands(session_with_ui(&scripted).await, "alice", commands).unwrap();

        let mut seen = Vec::new();
        tokio::time::timeout(
            std::time::Duration::from_secs(5),
            chat.run(|name, args| {
                seen.push((name.to_string(), args.to_string()));
                true
            }),
        )
        .await
        .expect("run did not return after the user ended input");

        let expected = vec![
            ("model".to_string(), "gemma-4-31b-it".to_string()),
            ("help".to_string(), String::new()),
        ];
        assert_eq!(seen, expected);
        assert_eq!(scripted.last_commands(), ["model", "help"]);
    }

    #[tokio::test]
    async fn run_stops_when_handler_returns_false() {
        let scripted = ScriptedChatUi::new(&[]);
        scripted.push_input(UserInput::Command {
            name: "exit".into(),
            args: String::new(),
        });
        scripted.push_input(UserInput::Command {
            name: "help".into(),
            args: String::new(),
        });
        let chat = Simple::with_commands(
            session_with_ui(&scripted).await,
            "alice",
            vec!["exit".into(), "help".into()],
        )
        .unwrap();

        let mut seen = Vec::new();
        tokio::time::timeout(
            std::time::Duration::from_secs(5),
            chat.run(|name, _| {
                seen.push(name.to_string());
                name != "exit"
            }),
        )
        .await
        .expect("run did not stop when the handler returned false");

        assert_eq!(seen, ["exit"]);
    }

    #[test]
    fn missing_chat_ui_is_reported_as_dependency_error() {
        let result = Simple::new(Session::new());
        assert!(matches!(result, Err(InitChatError::Dependency(_))));
    }
}
