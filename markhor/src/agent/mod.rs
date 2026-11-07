use std::{
    sync::{Arc, Mutex},
    time::SystemTime,
};

use markhor_core::{
    chat::{AddMemberError, ChatModel, ChatOptions, Group, IterMsg, Member, MemberId, Msg},
    event::Subscription,
    extension::{Comp, ModelId},
    tool::Tool,
};

mod config;

pub use config::{Attentiveness, MsgFormat};
use thiserror::Error;
use tokio::{sync::mpsc, task};

pub struct AgentBuilder {
    group: Arc<Group>,
    model: Comp<dyn ChatModel>,
    id: Option<String>,
    display_name: Option<String>,
    msg_format: Option<MsgFormat>,
    tools: Vec<Comp<dyn Tool + Send + Sync>>,
    attn: Attentiveness,
}

impl AgentBuilder {
    pub fn new(group: &Arc<Group>, model: Comp<dyn ChatModel>) -> Self {
        Self {
            group: group.clone(),
            model,
            id: Default::default(),
            display_name: Default::default(),
            msg_format: Default::default(),
            tools: Default::default(),
            attn: Default::default(),
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

    pub fn msg_format(mut self, msg_format: MsgFormat) -> Self {
        self.msg_format = Some(msg_format);
        self
    }

    pub fn add_tool(mut self, tool: Comp<dyn Tool + Send + Sync>) -> Self {
        self.tools.push(tool);
        self
    }

    pub fn spawn(self) -> Result<Agent, InitAgentError> {
        let mut sfx = 0;
        let member = loop {
            let sfx_str = if sfx == 0 { "" } else { &sfx.to_string() };
            let id = MemberId::with_display_name(
                self.id
                    .as_ref()
                    .map(String::as_str)
                    .unwrap_or(format!("assistant{}", sfx_str).as_str()),
                self.display_name
                    .as_ref()
                    .map(String::as_str)
                    .unwrap_or_default(),
                false,
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

        let (tx, rx) = mpsc::unbounded_channel();
        let id = member.id().clone();
        let sub = member.thread().on_message_appended(move |msg, idx| {
            if self.attn.attends_to(&id, msg) {
                // Fails only after the responder has stopped, which is harmless.
                let _ = tx.send((idx, sender_of(msg).cloned()));
            }
        });

        let inner = Arc::new(AgentInner {
            member,
            model: self.model,
            msg_format: self.msg_format.unwrap_or(MsgFormat::Xml).into(),
            tools: self.tools,
        });
        let cloned = inner.clone();
        let handle = task::spawn(async move {
            cloned.run(rx).await;
        });
        Ok(Agent {
            inner,
            handle,
            thread_sub: sub,
        })
    }
}

pub struct Agent {
    inner: Arc<AgentInner>,
    handle: tokio::task::JoinHandle<()>,
    thread_sub: Subscription,
}

struct AgentInner {
    member: Member,
    model: Comp<dyn ChatModel>,
    msg_format: Mutex<MsgFormat>,
    tools: Vec<Comp<dyn Tool + Send + Sync>>,
}

impl AgentInner {
    async fn run(&self, mut rx: mpsc::UnboundedReceiver<(usize, Option<MemberId>)>) {
        while let Some((msg_idx, to)) = rx.recv().await {
            self.respond(msg_idx, to).await;
        }
    }

    /// Replies to the message at `msg_idx`. On model failure, reports the error
    /// to `to`, the sender of the triggering message, if there is one.
    async fn respond(&self, msg_idx: usize, to: Option<MemberId>) {
        let msg_format = *self.msg_format.lock().unwrap();
        // `context_until` excludes its end index, so include the triggering message.
        let context: Vec<Msg> = self
            .member
            .context_until(msg_idx + 1)
            .circumfix_content_with(|msg| msg_format.circumfix(msg))
            .collect();

        match self.model.generate(&context, &ChatOptions::default()).await {
            Ok(response) => {
                let msg = Msg::Agent {
                    sender: self.member.id().clone(),
                    timestamp: Some(SystemTime::now()),
                    content: response.content,
                    tool_calls: response.tool_calls,
                    model: ModelId::new(self.model.model_name(), self.model.extension().uri()),
                    reasoning_content: None,
                    metadata: Default::default(),
                };
                // Okay to unwrap: the sender is a member of this group.
                self.member.group().append(msg).unwrap();
            }
            Err(err) => match to {
                Some(recipient) => {
                    let event = Msg::Event {
                        recipients: std::iter::once(recipient).collect(),
                        timestamp: Some(SystemTime::now()),
                        source: self.member.id().id().to_string(),
                        content: err.to_string().into(),
                    };
                    // Okay to unwrap: the sender is a member of this group.
                    self.member.group().append(event).unwrap();
                }
                None => {
                    tracing::error!(error = %err, "model call failed with no one to report it to")
                }
            },
        }

        // also, tool calls once those are implemented
    }
}

/// The member who sent `msg`, if the message names one.
fn sender_of(msg: &Msg) -> Option<&MemberId> {
    match msg {
        Msg::User { sender, .. } | Msg::Agent { sender, .. } | Msg::Direct { sender, .. } => {
            Some(sender)
        }
        _ => None,
    }
}

#[derive(Error, Debug)]
pub enum InitAgentError {
    #[error("Error adding new member to group: {0}")]
    IdCollision(#[from] AddMemberError),
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_utils::{
        ScriptedChatModel, ScriptedChatUi, eventually, record_messages, session_with,
        session_with_mocks,
    };
    use markhor_core::{
        chat::{Msg, common::ContentPart},
        dependencies::Session,
    };

    fn model(session: &Session) -> Comp<dyn ChatModel> {
        session.resolve().unwrap()
    }

    #[tokio::test]
    async fn default_id_gets_numeric_suffix_on_collision() {
        let session = session_with_mocks().await;
        let group = Group::new();
        let first = AgentBuilder::new(&group, model(&session)).spawn().unwrap();
        let second = AgentBuilder::new(&group, model(&session)).spawn().unwrap();
        assert_eq!(first.inner.member.id().id(), "assistant");
        assert_eq!(second.inner.member.id().id(), "assistant1");
    }

    #[tokio::test]
    async fn explicit_id_collision_is_an_error() {
        let session = session_with_mocks().await;
        let group = Group::new();
        let _first = AgentBuilder::new(&group, model(&session))
            .id("bot")
            .spawn()
            .unwrap();
        let second = AgentBuilder::new(&group, model(&session)).id("bot").spawn();
        assert!(matches!(second, Err(InitAgentError::IdCollision(_))));
    }

    #[tokio::test]
    async fn responder_task_stays_alive_while_agent_exists() {
        let session = session_with_mocks().await;
        let group = Group::new();
        let agent = AgentBuilder::new(&group, model(&session)).spawn().unwrap();
        tokio::task::yield_now().await;
        assert!(!agent.handle.is_finished());
    }

    #[tokio::test]
    async fn appending_after_responder_stopped_does_not_panic() {
        let session = session_with_mocks().await;
        let group = Group::new();
        let agent = AgentBuilder::new(&group, model(&session)).spawn().unwrap();
        agent.handle.abort();
        tokio::task::yield_now().await;
        let msg = Msg::Direct {
            sender: MemberId::new("alice", true),
            timestamp: None,
            tool_call_id: String::new(),
            recipients: std::iter::once(agent.inner.member.id().clone()).collect(),
            content: vec![],
        };
        group.append(msg).unwrap();
    }

    fn say(group: &Group, sender: &Member, text: &str) {
        group
            .append(Msg::User {
                sender: sender.id().clone(),
                timestamp: None,
                content: vec![ContentPart::Text(text.into())],
            })
            .unwrap();
    }

    fn text_of(content: &[ContentPart]) -> String {
        content
            .iter()
            .filter_map(|part| match part {
                ContentPart::Text(text) => Some(text.as_str()),
                _ => None,
            })
            .collect()
    }

    #[tokio::test]
    async fn agent_replies_to_user_message() {
        let model_script = ScriptedChatModel::new(&[Ok("hi there")]);
        let session = session_with(&ScriptedChatUi::new(&[]), &model_script).await;
        let group = Group::new();
        let alice = group.add_member(MemberId::new("alice", true)).unwrap();
        let (_sub, seen) = record_messages(&alice);
        let _agent = AgentBuilder::new(&group, model(&session)).spawn().unwrap();
        say(&group, &alice, "hello");

        let reply = eventually(|| {
            seen.lock().unwrap().iter().find_map(|msg| match msg {
                Msg::Agent { content, .. } => Some(content.clone()),
                _ => None,
            })
        })
        .await;
        assert_eq!(reply, vec![ContentPart::Text("hi there".into())]);
    }

    #[tokio::test]
    async fn model_context_includes_triggering_message() {
        let model_script = ScriptedChatModel::new(&[Ok("hi there")]);
        let session = session_with(&ScriptedChatUi::new(&[]), &model_script).await;
        let group = Group::new();
        let alice = group.add_member(MemberId::new("alice", true)).unwrap();
        let _agent = AgentBuilder::new(&group, model(&session)).spawn().unwrap();
        say(&group, &alice, "hello");

        let request = eventually(|| model_script.requests().first().cloned()).await;
        let sent: String = request
            .iter()
            .filter_map(|msg| match msg {
                Msg::User { content, .. } => Some(text_of(content)),
                _ => None,
            })
            .collect();
        assert!(sent.contains("hello"), "model was sent: {sent}");
    }

    #[tokio::test]
    async fn model_error_posts_event_to_user() {
        let model_script = ScriptedChatModel::new(&[Err("boom")]);
        let session = session_with(&ScriptedChatUi::new(&[]), &model_script).await;
        let group = Group::new();
        let alice = group.add_member(MemberId::new("alice", true)).unwrap();
        let (_sub, seen) = record_messages(&alice);
        let _agent = AgentBuilder::new(&group, model(&session)).spawn().unwrap();
        say(&group, &alice, "hello");

        let (recipients, content) = eventually(|| {
            seen.lock().unwrap().iter().find_map(|msg| match msg {
                Msg::Event {
                    recipients,
                    content,
                    ..
                } => Some((recipients.clone(), content.clone())),
                _ => None,
            })
        })
        .await;
        assert!(recipients.contains(alice.id()));
        assert!(content.to_string().contains("boom"), "event was: {content}");
    }
}
