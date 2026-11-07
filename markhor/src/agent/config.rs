/// Types for configuring agent experience.
///
use std::time::SystemTime;

use markhor_core::chat::{MemberId, Msg};

// TODO: bikeshed name
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub struct Attentiveness;

impl Attentiveness {
    // TODO: bikeshed name
    pub(super) fn attends_to(&self, agent: &MemberId, msg: &Msg) -> bool {
        match msg {
            Msg::Platform(_) | Msg::Developer(_) => false,
            // always attend to user messages for now, consider checking
            // content later (e.g., selective attentiveness in case of
            // @mentions)
            Msg::User { .. } => true,
            Msg::Agent { sender, .. } => sender != agent,
            Msg::Direct { recipients, .. }
            | Msg::ToolResult { recipients, .. }
            | Msg::Event { recipients, .. }
            | Msg::ControlSignal { recipients, .. } => recipients.contains(agent),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MsgFormat {
    Simple,
    Xml,
}

impl MsgFormat {
    pub(super) fn circumfix(&self, msg: &Msg) -> (String, String) {
        let (tag, id, name, recp, time) = match msg {
            Msg::User {
                sender, timestamp, ..
            } => (
                "user_message",
                sender.id(),
                sender.display_name(),
                String::new(),
                self.format_time(timestamp),
            ),
            Msg::Agent {
                sender, timestamp, ..
            } => (
                "agent_message",
                sender.id(),
                sender.display_name(),
                String::new(),
                self.format_time(timestamp),
            ),
            Msg::Direct {
                sender,
                timestamp,
                recipients,
                ..
            } => (
                "agent_message",
                sender.id(),
                sender.display_name(),
                recipients
                    .iter()
                    .map(|r| r.id())
                    .collect::<Vec<_>>()
                    .join(","),
                self.format_time(timestamp),
            ),
            Msg::ToolResult { .. } => {
                unimplemented!()
            }
            Msg::Event {
                timestamp, source, ..
            } => (
                "system_event",
                source.as_str(),
                None,
                String::new(),
                self.format_time(timestamp),
            ),
            _ => ("", "", None, String::new(), None),
        };
        match self {
            MsgFormat::Simple => (String::new(), String::new()),
            MsgFormat::Xml => (
                format!(
                    "<{} id=\"{}\" {} recipients=\"{}\" {}>",
                    tag,
                    id,
                    name.map(|n| format!("name=\"{}\"", n)).unwrap_or_default(),
                    recp,
                    time.map(|t| format!("timestamp=\"{}\"", t))
                        .unwrap_or_default(),
                ),
                String::new(),
            ),
        }
    }

    fn format_time(&self, _timestamp: &Option<SystemTime>) -> Option<String> {
        None
    }
}
