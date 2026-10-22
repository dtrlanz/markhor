use std::time::SystemTime;

use serde_json::Value;
use smallvec::SmallVec;

use crate::chat::{
    chat::{ContentPart, ToolCallRequest},
    member::MemberId,
};

#[derive(Debug, Clone, PartialEq)]
pub enum Msg {
    Platform(String),
    Developer(String),
    User {
        sender: MemberId,
        timestamp: Option<SystemTime>,
        content: Vec<ContentPart>,
    },
    Agent {
        sender: MemberId,
        timestamp: Option<SystemTime>,
        content: Vec<ContentPart>,
        tool_calls: Vec<ToolCallRequest>,
    },
    Direct {
        sender: MemberId,
        timestamp: Option<SystemTime>,
        tool_call_id: String, // ?
        recipients: SmallVec<[MemberId; 1]>,
        content: Vec<ContentPart>,
    },
    ToolResult {
        recipients: SmallVec<[MemberId; 1]>,
        timestamp: Option<SystemTime>,
        tool_call_id: String,
        content: Vec<ContentPart>,
    },
    Event {
        recipients: SmallVec<[MemberId; 1]>,
        timestamp: Option<SystemTime>,
        source: String,
        content: Value,
    },
    ControlSignal {
        recipients: SmallVec<[MemberId; 1]>, // ?
        compaction_start: usize,
        content: String,
    },
}

impl Msg {
    /// Returns true if this message was sent by the first-person subject ("I")
    /// in a given context, false otherwise.
    ///
    /// See [`Member::context`] for more information about the way context is
    /// relative to a specific group member.
    pub fn belongs_to_first_person(&self) -> bool {
        match self {
            Msg::User { sender, .. } | Msg::Agent { sender, .. } => sender.is_first_person(),
            _ => false,
        }
    }
}
