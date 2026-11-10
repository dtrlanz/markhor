//! Serializable types for the Anthropic Messages API (`/v1/messages`) and Models API
//! (`/v1/models`).
//!
//! Content is modelled as blocks tagged by `"type"`. The set of blocks differs by direction
//! (a user message can't contain `tool_use`; an assistant message can't contain `tool_result`),
//! so each direction has its own enum. Enums received from the API carry an `Unknown`/`Opaque`
//! fallback so that values added by Anthropic don't break deserialization, and unknown response
//! fields are ignored.
//!
//! Not modelled yet: server tools (web search, code execution, ...), citations, search results,
//! the Files API, MCP, containers, batches, streaming events and context management. Assistant
//! blocks of unmodelled kinds are kept verbatim as [`AssistantBlock::Opaque`] so they can be
//! sent back unchanged.

mod error;
mod message;
mod models;
mod request;
mod response;

pub(crate) use error::{ApiError, ErrorKind, ErrorResponse};
pub(crate) use message::{
    AssistantBlock, CacheControl, CacheTtl, CitationsConfig, DocumentBlock, DocumentSource,
    ImageBlock, ImageMediaType, ImageSource, Message, PdfMediaType, PlainTextMediaType,
    RedactedThinking, TextBlock, Thinking, ToolResult, ToolResultContent, ToolUse, UserBlock,
};
pub(crate) use models::{CapabilitySupport, ListModelsResponse, ModelCapabilities, ModelInfo};
pub(crate) use request::{
    Effort, InputSchema, MessagesRequest, Metadata, OutputConfig, OutputFormat, ServiceTier,
    ThinkingConfig, ThinkingDisplay, ToolChoice, ToolDefinition,
};
pub(crate) use response::{MessagesResponse, ResponseServiceTier, StopReason, Usage};

pub(crate) const DEFAULT_BASE_URL: &str = "https://api.anthropic.com/v1";
pub(crate) const API_KEY_HEADER: &str = "x-api-key";
pub(crate) const VERSION_HEADER: &str = "anthropic-version";
pub(crate) const API_VERSION: &str = "2023-06-01";

fn is_false(value: &bool) -> bool {
    !*value
}
