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
use tokio::{sync::watch, task};

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

        let (tx, rx) = watch::channel(None);
        let id = member.id().clone();
        let sub = member.thread().on_message_appended(move |msg, idx| {
            if self.attn.attends_to(&id, msg) {
                tx.send_replace(Some(idx));
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
    async fn run(&self, mut rx: watch::Receiver<Option<usize>>) {
        while rx.changed().await.is_ok() {
            let msg_idx = *rx.borrow_and_update();
            if let Some(msg_idx) = msg_idx {
                self.respond(msg_idx).await;
            }
        }
    }

    /// Replies to the message at `msg_idx`. The reply begins as soon as the
    /// request is sent, and ends with either a completed message or a log.
    async fn respond(&self, msg_idx: usize) {
        let msg_format = *self.msg_format.lock().unwrap();
        // `context_until` excludes its end index, so include the triggering message.
        let context: Vec<_> = self
            .member
            .context_until(msg_idx + 1)
            .circumfix_content_with(|msg| msg_format.circumfix(msg))
            .collect();

        let pending = self
            .member
            .group()
            .begin_message(self.member.id())
            .expect("agent is a member of its own group");

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
                pending
                    .complete(msg)
                    .expect("agent message is sent by the agent");
            }
            Err(err) => pending.fail(err.to_string()),
        }

        // tool calls once those are implemented
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
        chat::{MsgEvent, MsgId, common::ContentPart},
        dependencies::Session,
    };

    fn model(session: &Session) -> Comp<dyn ChatModel> {
        session.resolve().unwrap()
    }

    #[tokio::test]
    async fn default_ids_are_distinct() {
        let session = session_with_mocks().await;
        let group = Group::new();
        let first = AgentBuilder::new(&group, model(&session)).spawn().unwrap();
        let second = AgentBuilder::new(&group, model(&session)).spawn().unwrap();
        assert_ne!(first.inner.member.id().id(), second.inner.member.id().id());
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

    fn record_events(member: &Member) -> (Subscription, Arc<Mutex<Vec<MsgEvent>>>) {
        let events = Arc::new(Mutex::new(Vec::new()));
        let sink = events.clone();
        let sub = member
            .thread()
            .on_message(move |e: &MsgEvent| sink.lock().unwrap().push(e.clone()));
        (sub, events)
    }

    /// The id of the first `Begin` event sent for `sender`.
    fn begin_id(events: &[MsgEvent], sender: &MemberId) -> Option<MsgId> {
        events.iter().find_map(|e| match e {
            MsgEvent::Begin(id, s) if s == sender => Some(*id),
            _ => None,
        })
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
        assert!(
            format!("{request:?}").contains("hello"),
            "triggering message missing from model context: {request:?}"
        );
    }

    #[tokio::test]
    async fn reply_begins_before_model_responds() {
        // An empty script makes the model block, so no response ever arrives.
        let model_script = ScriptedChatModel::new(&[]);
        let session = session_with(&ScriptedChatUi::new(&[]), &model_script).await;
        let group = Group::new();
        let alice = group.add_member(MemberId::new("alice", true)).unwrap();
        let (_sub, events) = record_events(&alice);
        let agent = AgentBuilder::new(&group, model(&session)).spawn().unwrap();
        let agent_id = agent.inner.member.id().clone();
        say(&group, &alice, "hello");

        eventually(|| begin_id(&events.lock().unwrap(), &agent_id)).await;
        assert!(!model_script.requests().is_empty());
        let seen = events.lock().unwrap();
        assert!(
            !seen
                .iter()
                .any(|e| matches!(e, MsgEvent::Full(_, Msg::Agent { .. })))
        );
    }

    #[tokio::test]
    async fn successful_reply_emits_full_with_begin_id() {
        let model_script = ScriptedChatModel::new(&[Ok("hi there")]);
        let session = session_with(&ScriptedChatUi::new(&[]), &model_script).await;
        let group = Group::new();
        let alice = group.add_member(MemberId::new("alice", true)).unwrap();
        let (_sub, events) = record_events(&alice);
        let agent = AgentBuilder::new(&group, model(&session)).spawn().unwrap();
        let agent_id = agent.inner.member.id().clone();
        say(&group, &alice, "hello");

        eventually(|| {
            let seen = events.lock().unwrap();
            let id = begin_id(&seen, &agent_id)?;
            seen.iter()
                .any(|e| matches!(e, MsgEvent::Full(f, Msg::Agent { .. }) if *f == id))
                .then_some(())
        })
        .await;
    }

    #[tokio::test]
    async fn model_failure_emits_log() {
        let model_script = ScriptedChatModel::new(&[Err("boom")]);
        let session = session_with(&ScriptedChatUi::new(&[]), &model_script).await;
        let group = Group::new();
        let alice = group.add_member(MemberId::new("alice", true)).unwrap();
        let (_sub, events) = record_events(&alice);
        let agent = AgentBuilder::new(&group, model(&session)).spawn().unwrap();
        let agent_id = agent.inner.member.id().clone();
        say(&group, &alice, "hello");

        let reason = eventually(|| {
            let seen = events.lock().unwrap();
            let id = begin_id(&seen, &agent_id)?;
            seen.iter().find_map(|e| match e {
                MsgEvent::Log(l, reason) if *l == id => Some(reason.clone()),
                _ => None,
            })
        })
        .await;
        assert!(reason.contains("boom"), "log was: {reason}");
    }

    #[tokio::test]
    async fn aborted_responder_emits_log() {
        let model_script = ScriptedChatModel::new(&[]);
        let session = session_with(&ScriptedChatUi::new(&[]), &model_script).await;
        let group = Group::new();
        let alice = group.add_member(MemberId::new("alice", true)).unwrap();
        let (_sub, events) = record_events(&alice);
        let agent = AgentBuilder::new(&group, model(&session)).spawn().unwrap();
        let agent_id = agent.inner.member.id().clone();
        say(&group, &alice, "hello");
        let id = eventually(|| begin_id(&events.lock().unwrap(), &agent_id)).await;

        agent.handle.abort();

        eventually(|| {
            events
                .lock()
                .unwrap()
                .iter()
                .any(|e| matches!(e, MsgEvent::Log(l, _) if *l == id))
                .then_some(())
        })
        .await;
    }
}
