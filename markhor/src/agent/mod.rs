use std::sync::{Arc, Mutex};

use markhor_core::{
    chat::{AddMemberError, Group, IterMsg, Member, MemberId, chat::ChatModel},
    event::Subscription,
    extension::Comp,
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

        let (tx, rx) = watch::channel(0);
        let id = member.id().clone();
        let sub = member.thread().on_message_appended(move |msg, idx| {
            if self.attn.attends_to(&id, msg) {
                tx.send(idx).unwrap();
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
            cloned.run(rx);
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
    async fn run(&self, mut rx: watch::Receiver<usize>) {
        loop {
            let Ok(msg_idx) = rx.wait_for(|i| *i > 0).await.map(|r| *r) else {
                break;
            };

            self.respond(msg_idx).await;
        }
    }

    async fn respond(&self, msg_idx: usize) {
        let msg_format = *self.msg_format.lock().unwrap();
        let context: Vec<_> = self
            .member
            .context_until(msg_idx)
            .circumfix_content_with(|msg| msg_format.circumfix(msg))
            .collect();

        todo!();
        // send to model

        // process response, convert to Msg

        // also, tool calls once those are implemented

        // Okay to unwrap: Msg::Agent, sender is known
        //self.member.group().append(msg).unwrap();
    }
}

#[derive(Error, Debug)]
pub enum InitAgentError {
    #[error("Error adding new member to group: {0}")]
    IdCollision(#[from] AddMemberError),
}
