use std::{sync::Arc, time::SystemTime};

use serde_json::Value;
use smallvec::SmallVec;

use crate::chat::chat::{ContentPart, ToolCallRequest};

#[derive(Debug, Clone, PartialEq)]
pub enum Msg {
    Platform(String),
    Developer(String),
    User {
        sender: Arc<str>,
        timestamp: Option<SystemTime>,
        content: Vec<ContentPart>,
    },
    Agent {
        sender: Arc<str>,
        timestamp: Option<SystemTime>,
        content: Vec<ContentPart>,
        tool_calls: Vec<ToolCallRequest>,
    },
    Direct {
        sender: Arc<str>,
        timestamp: Option<SystemTime>,
        tool_call_id: String, // ?
        recipients: SmallVec<[Arc<str>; 1]>,
        content: Vec<ContentPart>,
    },
    ToolResult {
        recipients: SmallVec<[Arc<str>; 1]>,
        timestamp: Option<SystemTime>,
        tool_call_id: String,
        content: Vec<ContentPart>,
    },
    Event {
        recipients: SmallVec<[Arc<str>; 1]>,
        timestamp: Option<SystemTime>,
        source: String,
        content: Value,
    },
    ControlSignal {
        recipients: SmallVec<[Arc<str>; 1]>, // ?
        compaction_start: usize,
        content: String,
    },
}
