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

#[cfg(test)]
mod tests {
    use super::*;
    use markhor_core::extension::ModelId;

    fn member(id: &str) -> MemberId {
        MemberId::new(id, false)
    }

    fn user_msg(sender: &str) -> Msg {
        Msg::User {
            sender: MemberId::new(sender, true),
            timestamp: None,
            content: vec![],
        }
    }

    fn agent_msg(sender: &str) -> Msg {
        Msg::Agent {
            sender: member(sender),
            timestamp: None,
            content: vec![],
            tool_calls: vec![],
            model: ModelId::new("model", "provider"),
            reasoning_content: None,
            metadata: Default::default(),
        }
    }

    fn direct_msg(sender: &str, recipients: &[&str]) -> Msg {
        Msg::Direct {
            sender: member(sender),
            timestamp: None,
            tool_call_id: String::new(),
            recipients: recipients.iter().map(|r| member(r)).collect(),
            content: vec![],
        }
    }

    #[test]
    fn attends_to_user_messages() {
        let agent = member("assistant");
        assert!(Attentiveness.attends_to(&agent, &user_msg("alice")));
    }

    #[test]
    fn ignores_platform_and_developer_messages() {
        let agent = member("assistant");
        assert!(!Attentiveness.attends_to(&agent, &Msg::Platform("notice".into())));
        assert!(!Attentiveness.attends_to(&agent, &Msg::Developer("instructions".into())));
    }

    #[test]
    fn attends_to_other_agents_but_not_itself() {
        let agent = member("assistant");
        assert!(Attentiveness.attends_to(&agent, &agent_msg("bob")));
        assert!(!Attentiveness.attends_to(&agent, &agent_msg("assistant")));
    }

    #[test]
    fn attends_to_direct_messages_only_when_addressed() {
        let agent = member("assistant");
        assert!(Attentiveness.attends_to(&agent, &direct_msg("bob", &["carol", "assistant"])));
        assert!(!Attentiveness.attends_to(&agent, &direct_msg("bob", &["carol"])));
    }

    #[test]
    fn simple_format_adds_no_delimiters() {
        let (open, close) = MsgFormat::Simple.circumfix(&user_msg("alice"));
        assert!(open.is_empty());
        assert!(close.is_empty());
    }

    #[test]
    fn xml_format_includes_sender_id_and_name() {
        let msg = Msg::User {
            sender: MemberId::with_display_name("alice", "Alice", true),
            timestamp: None,
            content: vec![],
        };
        let (open, close) = MsgFormat::Xml.circumfix(&msg);
        assert!(open.starts_with("<user_message id=\"alice\" name=\"Alice\""));
        assert!(close.is_empty());
    }

    #[test]
    fn xml_format_lists_direct_message_recipients() {
        let (open, _) = MsgFormat::Xml.circumfix(&direct_msg("alice", &["bob", "carol"]));
        assert!(open.starts_with("<agent_message id=\"alice\""));
        assert!(open.contains("recipients=\"bob,carol\""));
    }
}
