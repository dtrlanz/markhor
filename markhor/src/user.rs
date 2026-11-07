use std::sync::Arc;

use markhor_core::{
    chat::{AddMemberError, ChatUi, Group, Member, MemberId},
    event::Subscription,
    extension::Comp,
};

use thiserror::Error;
use tokio::{sync::watch, task};

pub struct UserBuilder {
    group: Arc<Group>,
    ui: Comp<dyn ChatUi>,
    id: Option<String>,
    display_name: Option<String>,
}

impl UserBuilder {
    pub fn new(group: &Arc<Group>, ui: Comp<dyn ChatUi>) -> Self {
        Self {
            group: group.clone(),
            ui,
            id: Default::default(),
            display_name: Default::default(),
        }
    }

    pub fn id(mut self, id: &str) -> Self {
        self.id = Some(id.to_string());
        self
    }

    pub fn display_name(mut self, display_name: &str) -> Self {
        self.display_name = Some(display_name.to_string());
        self
    }

    pub fn spawn(self) -> Result<User, InitUserError> {
        let mut sfx = 0;
        let member = loop {
            let sfx_str = if sfx == 0 { "" } else { &sfx.to_string() };
            let id = MemberId::with_display_name(
                self.id
                    .as_ref()
                    .map(String::as_str)
                    .unwrap_or(format!("user{}", sfx_str).as_str()),
                self.display_name
                    .as_ref()
                    .map(String::as_str)
                    .unwrap_or_default(),
                true,
            );
            match self.group.add_member(id) {
                Ok(member) => break member,
                Err(e) if self.id.is_some() => {
                    return Err(e.into());
                }
                _ => {
                    sfx += 1;
                }
            }
        };

        let (tx, rx) = watch::channel(None);
        let id = member.id().clone();
        // TODO: add other events needed consumed by `ChatUi`
        let sub = member.thread().on_message_appended(move |_, idx| {
            tx.send_replace(Some(idx));
        });

        let inner = Arc::new(UserInner {
            member,
            ui: self.ui,
        });
        let cloned = inner.clone();
        let handle = task::spawn(async move {
            cloned.run(rx).await;
        });
        Ok(User {
            inner,
            handle,
            thread_sub: sub,
        })
    }
}

pub struct User {
    inner: Arc<UserInner>,
    handle: tokio::task::JoinHandle<()>,
    thread_sub: Subscription,
}

struct UserInner {
    member: Member,
    ui: Comp<dyn ChatUi>,
}

impl UserInner {
    async fn run(&self, mut rx: watch::Receiver<Option<usize>>) {
        while rx.changed().await.is_ok() {
            let msg_idx = *rx.borrow_and_update();
            if let Some(msg_idx) = msg_idx {
                self.respond(msg_idx).await;
            }
        }
    }

    async fn respond(&self, msg_idx: usize) {
        let context: Vec<_> = self.member.context_until(msg_idx).collect();

        todo!();
        // send to UI, user response, etc.
    }
}

#[derive(Error, Debug)]
pub enum InitUserError {
    #[error("Error adding new member to group: {0}")]
    IdCollision(#[from] AddMemberError),
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_utils::{ScriptedChatUi, eventually, session_with_mocks, session_with_ui};
    use markhor_core::{
        chat::{ChatUi, Msg, common::ContentPart},
        dependencies::Session,
        extension::ModelId,
    };
    use std::time::Duration;

    fn ui(session: &Session) -> Comp<dyn ChatUi> {
        session.resolve().unwrap()
    }

    #[tokio::test]
    async fn default_id_gets_numeric_suffix_and_is_human() {
        let session = session_with_mocks().await;
        let group = Group::new();
        let first = UserBuilder::new(&group, ui(&session)).spawn().unwrap();
        let second = UserBuilder::new(&group, ui(&session)).spawn().unwrap();
        assert_eq!(first.inner.member.id().id(), "user");
        assert_eq!(second.inner.member.id().id(), "user1");
        assert!(first.inner.member.human());
    }

    #[tokio::test]
    async fn explicit_id_collision_is_an_error() {
        let session = session_with_mocks().await;
        let group = Group::new();
        let _first = UserBuilder::new(&group, ui(&session))
            .id("alice")
            .spawn()
            .unwrap();
        let second = UserBuilder::new(&group, ui(&session)).id("alice").spawn();
        assert!(matches!(second, Err(InitUserError::IdCollision(_))));
    }

    #[tokio::test]
    async fn responder_task_stays_alive_while_user_exists() {
        let session = session_with_mocks().await;
        let group = Group::new();
        let user = UserBuilder::new(&group, ui(&session)).spawn().unwrap();
        tokio::task::yield_now().await;
        assert!(!user.handle.is_finished());
    }

    #[tokio::test]
    async fn appending_after_responder_stopped_does_not_panic() {
        let session = session_with_mocks().await;
        let group = Group::new();
        let user = UserBuilder::new(&group, ui(&session)).spawn().unwrap();
        user.handle.abort();
        tokio::task::yield_now().await;
        let msg = Msg::Direct {
            sender: MemberId::new("assistant", false),
            timestamp: None,
            tool_call_id: String::new(),
            recipients: std::iter::once(user.inner.member.id().clone()).collect(),
            content: vec![],
        };
        group.append(msg).unwrap();
    }

    #[tokio::test]
    async fn launch_prompts_for_first_message() {
        let scripted = ScriptedChatUi::new(&["hello"]);
        let session = session_with_ui(&scripted).await;
        let group = Group::new();
        let observer = group.add_member(MemberId::new("observer", false)).unwrap();
        let _user = UserBuilder::new(&group, ui(&session))
            .id("alice")
            .spawn()
            .unwrap();

        scripted.wait_for_prompts(1).await;
        match eventually(|| observer.context().next()).await {
            Msg::User {
                sender, content, ..
            } => {
                assert_eq!(sender.id(), "alice");
                assert_eq!(content, vec![ContentPart::Text("hello".into())]);
            }
            other => panic!("expected a user message, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn own_message_does_not_trigger_another_prompt() {
        let scripted = ScriptedChatUi::new(&["hello", "again"]);
        let session = session_with_ui(&scripted).await;
        let group = Group::new();
        let observer = group.add_member(MemberId::new("observer", false)).unwrap();
        let _user = UserBuilder::new(&group, ui(&session)).spawn().unwrap();

        scripted.wait_for_prompts(1).await;
        // Wait until the user's own message has been appended, then give its
        // responder time to react to it.
        eventually(|| observer.context().next()).await;
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert_eq!(scripted.prompt_count(), 1);
    }

    #[tokio::test]
    async fn agent_message_is_forwarded_then_prompts_again() {
        let scripted = ScriptedChatUi::new(&["hello", "again"]);
        let session = session_with_ui(&scripted).await;
        let group = Group::new();
        let observer = group.add_member(MemberId::new("observer", false)).unwrap();
        let assistant = group.add_member(MemberId::new("assistant", false)).unwrap();
        let _user = UserBuilder::new(&group, ui(&session)).spawn().unwrap();

        scripted.wait_for_prompts(1).await;
        eventually(|| observer.context().next()).await;

        let reply = Msg::Agent {
            sender: assistant.id().clone(),
            timestamp: None,
            content: vec![ContentPart::Text("hi there".into())],
            tool_calls: vec![],
            model: ModelId::new("mock", "test"),
            reasoning_content: None,
            metadata: Default::default(),
        };
        group.append(reply.clone()).unwrap();

        scripted.wait_for_prompts(2).await;
        assert_eq!(scripted.recorded_messages(), vec![reply]);
    }
}
