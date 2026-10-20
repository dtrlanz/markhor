use std::time::SystemTime;

use serde_json::Value;
use smallvec::SmallVec;

use crate::chat::{
    chat::{ContentPart, ToolCallRequest},
    group::MemberId,
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
