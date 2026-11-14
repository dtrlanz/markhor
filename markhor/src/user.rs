use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::SystemTime,
};

use markhor_core::{
    chat::{AddMemberError, ChatUi, Group, Member, MemberId, Msg, MsgEvent, common::ContentPart},
    event::Subscription,
    extension::Comp,
};

use thiserror::Error;
use tokio::{sync::Notify, task};

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

        let ui = Arc::new(self.ui);
        let reply_pending = Arc::new(AtomicBool::new(false));
        let reply_settled = Arc::new(Notify::new());

        let sub = {
            let ui = ui.clone();
            let reply_pending = reply_pending.clone();
            let reply_settled = reply_settled.clone();
            member.thread().on_message(move |event| {
                ui.handle_message(event);
                if matches!(event, MsgEvent::Full(_, Msg::Agent { .. }) | MsgEvent::Log(..)) {
                    reply_pending.store(false, Ordering::SeqCst);
                    reply_settled.notify_one();
                }
            })
        };

        let inner = Arc::new(UserInner {
            member,
            ui,
            reply_pending,
            reply_settled,
        });
        let cloned = inner.clone();
        let handle = task::spawn(async move {
            cloned.run().await;
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
    ui: Arc<Comp<dyn ChatUi>>,
    /// Set when the user posts, cleared when an agent reply or failure settles.
    reply_pending: Arc<AtomicBool>,
    /// Signalled each time a reply settles. Repeated signals coalesce into one.
    reply_settled: Arc<Notify>,
}

impl UserInner {
    async fn run(&self) {
        self.prompt_and_post().await;
        loop {
            self.reply_settled.notified().await;
            // A signal queued while the user was typing is stale if they have
            // since replied, so prompt only when no reply is outstanding.
            if !self.reply_pending.load(Ordering::SeqCst) {
                self.prompt_and_post().await;
            }
        }
    }

    /// Prompts until a non-blank answer is given, then posts it.
    async fn prompt_and_post(&self) {
        let answer = loop {
            let input = self.ui.prompt_with_commands(Vec::new()).await;
            if !input.trim().is_empty() {
                break input;
            }
        };
        // Set before posting, so a reply that settles right after cannot be missed.
        self.reply_pending.store(true, Ordering::SeqCst);
        self.member
            .group()
            .begin_message(self.member.id())
            .expect("user is a member of its own group")
            .complete(Msg::User {
                sender: self.member.id().clone(),
                timestamp: Some(SystemTime::now()),
                content: vec![ContentPart::Text(answer)],
            })
            .expect("user message is sent by the user");
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
