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

        let (tx, rx) = watch::channel(0);
        let id = member.id().clone();
        // TODO: add other events needed consumed by `ChatUi`
        let sub = member.thread().on_message_appended(move |_, idx| {
            tx.send(idx).unwrap();
        });

        let inner = Arc::new(UserInner {
            member,
            ui: self.ui,
        });
        let cloned = inner.clone();
        let handle = task::spawn(async move {
            cloned.run(rx);
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
    async fn run(&self, mut rx: watch::Receiver<usize>) {
        loop {
            let Ok(msg_idx) = rx.wait_for(|i| *i > 0).await.map(|r| *r) else {
                break;
            };

            self.respond(msg_idx).await;
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
    use crate::test_utils::session_with_mocks;
    use markhor_core::{chat::ChatUi, dependencies::Session};

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
}
