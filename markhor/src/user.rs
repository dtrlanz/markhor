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
    use crate::test_utils::{ScriptedChatUi, eventually, session_with_ui};
    use markhor_core::{
        chat::{ChatUi, Msg, common::ContentPart},
        dependencies::Session,
        extension::ModelId,
    };
    use std::time::Duration;

    fn ui(session: &Session) -> Comp<dyn ChatUi> {
        session.resolve().unwrap()
    }

    fn user_texts(messages: &[Msg]) -> Vec<String> {
        messages
            .iter()
            .filter_map(|msg| match msg {
                Msg::User { content, .. } => Some(
                    content
                        .iter()
                        .filter_map(|part| match part {
                            ContentPart::Text(text) => Some(text.as_str()),
                            _ => None,
                        })
                        .collect::<String>(),
                ),
                _ => None,
            })
            .collect()
    }

    /// Settles a reply from `bot` with the given text.
    fn bot_reply(group: &Group, bot: &Member, text: &str) {
        group
            .begin_message(bot.id())
            .unwrap()
            .complete(Msg::Agent {
                sender: bot.id().clone(),
                timestamp: None,
                content: vec![ContentPart::Text(text.into())],
                tool_calls: vec![],
                model: ModelId::new("scripted", "markhor://test-user"),
                reasoning_content: None,
                metadata: Default::default(),
            })
            .unwrap();
    }

    /// Settles a failed reply from `bot`.
    fn bot_failure(group: &Group, bot: &Member) {
        group
            .begin_message(bot.id())
            .unwrap()
            .fail("boom".to_string());
    }

    #[tokio::test]
    async fn default_ids_are_distinct_and_human() {
        let session = session_with_ui(&ScriptedChatUi::new(&[])).await;
        let group = Group::new();
        let first = UserBuilder::new(&group, ui(&session)).spawn().unwrap();
        let second = UserBuilder::new(&group, ui(&session)).spawn().unwrap();
        assert_ne!(first.inner.member.id().id(), second.inner.member.id().id());
        assert!(first.inner.member.human());
        assert!(second.inner.member.human());
    }

    #[tokio::test]
    async fn explicit_id_collision_is_an_error() {
        let session = session_with_ui(&ScriptedChatUi::new(&[])).await;
        let group = Group::new();
        let _first = UserBuilder::new(&group, ui(&session))
            .id("alice")
            .spawn()
            .unwrap();
        let second = UserBuilder::new(&group, ui(&session)).id("alice").spawn();
        assert!(matches!(second, Err(InitUserError::IdCollision(_))));
    }

    #[tokio::test]
    async fn appending_after_responder_stopped_does_not_panic() {
        let session = session_with_ui(&ScriptedChatUi::new(&[])).await;
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
    async fn prompts_once_at_startup() {
        let scripted = ScriptedChatUi::new(&[]);
        let session = session_with_ui(&scripted).await;
        let group = Group::new();
        let _user = UserBuilder::new(&group, ui(&session)).spawn().unwrap();
        scripted.wait_for_prompts(1).await;
        assert_eq!(scripted.prompt_count(), 1);
    }

    #[tokio::test]
    async fn agent_reply_prompts_and_posts_answer() {
        let scripted = ScriptedChatUi::new(&["hello", "again"]);
        let session = session_with_ui(&scripted).await;
        let group = Group::new();
        let _user = UserBuilder::new(&group, ui(&session))
            .id("alice")
            .spawn()
            .unwrap();
        let bot = group.add_member(MemberId::new("bot", false)).unwrap();

        eventually(|| (user_texts(&scripted.recorded_messages()) == ["hello"]).then_some(())).await;
        bot_reply(&group, &bot, "hi");
        eventually(|| {
            (user_texts(&scripted.recorded_messages()) == ["hello", "again"]).then_some(())
        })
        .await;
        assert_eq!(scripted.prompt_count(), 2);
    }

    #[tokio::test]
    async fn failed_reply_prompts_again() {
        let scripted = ScriptedChatUi::new(&["hi", "retry"]);
        let session = session_with_ui(&scripted).await;
        let group = Group::new();
        let _user = UserBuilder::new(&group, ui(&session))
            .id("alice")
            .spawn()
            .unwrap();
        let bot = group.add_member(MemberId::new("bot", false)).unwrap();

        eventually(|| (user_texts(&scripted.recorded_messages()) == ["hi"]).then_some(())).await;
        bot_failure(&group, &bot);
        eventually(|| {
            (user_texts(&scripted.recorded_messages()) == ["hi", "retry"]).then_some(())
        })
        .await;
        assert_eq!(scripted.prompt_count(), 2);
    }

    #[tokio::test]
    async fn empty_answer_reprompts() {
        let scripted = ScriptedChatUi::new(&["", "hi"]);
        let session = session_with_ui(&scripted).await;
        let group = Group::new();
        let _user = UserBuilder::new(&group, ui(&session)).spawn().unwrap();

        eventually(|| (user_texts(&scripted.recorded_messages()) == ["hi"]).then_some(())).await;
        assert_eq!(scripted.prompt_count(), 2);
    }

    #[tokio::test]
    async fn events_are_forwarded_to_ui() {
        let scripted = ScriptedChatUi::new(&[]);
        let session = session_with_ui(&scripted).await;
        let group = Group::new();
        let _user = UserBuilder::new(&group, ui(&session)).spawn().unwrap();
        let bot = group.add_member(MemberId::new("bot", false)).unwrap();

        bot_failure(&group, &bot);
        eventually(|| (scripted.begun_count() > 0).then_some(())).await;
    }

    /// A reply settles while the user is still typing, so its signal is
    /// queued. When the user then sends, the queued signal must not prompt
    /// again, or the thread would get two user messages in a row.
    #[tokio::test]
    async fn stale_signal_while_typing_does_not_prompt() {
        let scripted = ScriptedChatUi::new(&[]);
        let session = session_with_ui(&scripted).await;
        let group = Group::new();
        let _user = UserBuilder::new(&group, ui(&session))
            .id("alice")
            .spawn()
            .unwrap();
        let bot = group.add_member(MemberId::new("bot", false)).unwrap();
        scripted.wait_for_prompts(1).await;

        bot_reply(&group, &bot, "early");
        scripted.push_answer("hello");
        eventually(|| (user_texts(&scripted.recorded_messages()) == ["hello"]).then_some(())).await;
        // Give a stale prompt time to appear before checking for it.
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert_eq!(scripted.prompt_count(), 1, "prompted while awaiting a reply");

        bot_reply(&group, &bot, "reply");
        scripted.wait_for_prompts(2).await;
    }
}
