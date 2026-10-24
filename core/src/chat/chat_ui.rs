use std::ops::Deref;

use async_trait::async_trait;

use crate::chat::{
    MemberId, Msg,
    common::{ContentPart, ToolCallRequest, ToolResult},
};

#[async_trait]
pub trait ChatUi: Send + Sync {
    /// Prompt the user for text input in the chat interface.
    async fn prompt(&self) -> String {
        self.prompt_with_commands(vec![]).await
    }

    /// Prompt the user for input in the chat interface, with a list of
    /// commands that can be used.
    ///
    /// The `commands` argument contains the list of available commands.
    ///
    /// Commands are usually implemented as slash commands (e.g., `/model`,
    /// `/exit`). However, this is up to the UI implementation, and the list
    /// should be passed without the slash (e.g., `model`, `exit`).
    async fn prompt_with_commands(&self, commands: Vec<String>) -> String;

    /// Prompt the user to select an option from a list.
    ///
    /// The `options` argument contains the list of available options.
    async fn select(&self, options: &Vec<String>) -> Option<usize>;

    /// Handle events related to chat messages.
    ///
    /// The UI implementation receives all messages visible to the user through
    /// this event handler.
    ///
    /// Note that this applies both to incoming messages and to messages sent
    /// by the user. In cases where a use message is already visible after
    /// [`ChatUi::prompt`] returns (e.g., CLI contexts), the implementation
    /// should take care not to display it twice.
    ///
    /// See [`MsgEvent`] for the list of events that can be received.
    fn handle_message(&self, event: &MsgEvent);

    /// Handle events related to reasoning traces.
    ///
    /// Every event received through this handler is associated with a prior
    /// message, so that the reasoning trace can be displayed in the context
    /// of that message.
    ///
    /// See [`ReasoningEvent`] for details.
    fn handle_reasoning(&self, event: &ReasoningEvent);

    /// Handle events related to tool calls.
    ///
    /// Every event received through this handler is associated with a prior
    /// message, so that the tool call can be displayed in the context of that
    /// message.
    ///
    /// See [`ToolCallEvent`] for details.
    fn handle_tool_call(&self, event: &ToolCallEvent);

    /// Handle events related to worker agents.
    ///
    /// Workers are background agents that can perform tasks on behalf of
    /// other agents. Even though their messages are not addressed to the user,
    /// UI implementations may choose to surface information about their
    /// activity.
    ///
    /// See [`WorkerEvent`] for details.
    fn handle_worker(&self, event: &WorkerEvent);
}

/// Events related to chat messages.
pub enum MsgEvent {
    /// Indicates that a new message is being sent, received, or drafted.
    ///
    /// This event is emitted for any message, whether already completed (in
    /// which case `MsgEvent::Full` should follow immediately) or still being
    /// generated. In may even be emitted in cases where the full message
    /// never arrives (e.g., in case of a network error or if human user begins
    /// typing a message but does not proceed to send it).
    Begin(MsgId, MemberId),

    /// Represents the next part of a message's content.
    ///
    /// This event is emitted for messages that are being streamed.
    Part(MsgId, ContentPart),

    /// Represents a message that has been fully received.
    ///
    /// This event is emitted for every completed message, whether it was
    /// streamed or not. In case of streamed messages, it is emitted *after*
    /// the last `Msg::Part` event.
    Full(MsgId, Msg),

    /// Represents a log event related to a message.
    ///
    /// This is not meant to serve as a debugging mechanism but rather as a
    /// user-facing way to surface information about the message's status. UI
    /// implementations may choose whether/how to display this information.
    Log(MsgId, String),
}

/// Events related to reasoning traces.
pub enum ReasoningEvent {
    /// Indicates the beginning of a new reasoning trace associated with a
    /// message.
    ///
    /// The `MsgId` matches that of a prior `MsgEvent::Begin` event.
    Begin(MsgId),

    /// Represents the next part of a reasoning trace.
    ///
    /// This event is emitted for reasoning traces that are being streamed.
    Part(MsgId, String),

    /// Represents an interim summary of a reasoning trace.
    ///
    /// Interim summaries are a series of shorter, digestible summaries of the
    /// raw reasoning trace. They may be emitted insteaad of or in addition to
    /// the raw reasoning trace depending on model choice, configuration, and
    /// other factors. They are meant to be more user-friendly and easier to
    /// read.
    ///
    /// The first string is a phrase that may serve as a title or short
    /// description (e.g. "Considering alternative solutions"), while the
    /// second is a prose summary (e.g., 1-2 sentences) of this part of the
    /// reasoning trace. UI implementations might display either of these,
    /// both, or neither.
    Summary(MsgId, String, String),

    /// Represents a completed reasoning trace.
    ///
    /// This event is emitted for every completed reasoning trace, whether it
    /// was streamed or not. In case of streamed reasoning traces, it is
    /// emitted *after* the last `ReasoningEvent::Part` event.
    Full(MsgId, String),

    /// Represents a log event related to a reasoning trace.
    Log(MsgId, String),
}

/// Events related to tool calls.
pub enum ToolCallEvent {
    /// Indicates the beginning of a new tool call associated with a message.
    ///
    /// The `MsgId` matches that of a prior `MsgEvent::Begin` event.
    Begin(MsgId, ToolCallId, ToolCallRequest),

    /// Represents part of a tool call's console output.
    ///
    /// This is not part of the tool's return value but output from the tool's
    /// execution (e.g., stdout).
    Part(ToolCallId, String),

    /// Represents a completed tool call.
    Full(ToolCallId, ToolResult),

    /// Represents a log event related to a tool call.
    Log(ToolCallId, String),
}

/// Events related to worker agents.
pub enum WorkerEvent {
    /// Indicates that a new worker agent has been created.
    Begin(MemberId),

    /// Represents an interim summary of a worker agent's activity.
    ///
    /// The first string is a phrase that may serve as a title or short
    /// description (e.g. "Searching for relevant sources"), while the second
    /// is a prose summary (e.g., 1-2 sentences) of the agent's current task.
    /// UI implementations might display either of these, both, or neither.
    Summary(MemberId, String, String),

    /// Indicates that a worker agent has completed its tasks.
    ///
    /// The string may contain the final output of the worker agent or a
    /// summary of it. It may be empty if no text representation is available.
    Full(MemberId, String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct MsgId(usize);

impl Deref for MsgId {
    type Target = usize;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ToolCallId(usize);

impl Deref for ToolCallId {
    type Target = usize;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}
