//! Serializable types for the Gemini REST API (`v1beta`).
//!
//! These mirror the wire format closely but model it with Rust enums rather than
//! stringly-typed fields. Enums received from the API carry an `Unknown(String)`
//! fallback so that new values added by Google don't break deserialization.
//!
//! Only the subset of the API that Markhor uses is represented here; unknown
//! response fields are ignored.

mod error;
mod generate;
mod models;
mod turn;

pub(crate) use error::ErrorResponse;
pub(crate) use generate::{BlockReason, GenerateContentRequest, GenerateContentResponse};
pub(crate) use models::{GenerationMethod, ListModelsResponse};
pub(crate) use turn::{
    ModelPart, ModelPartKind, ModelTurn, SystemInstruction, Turn, UserPart, UserTurn,
};

/// Returns the wire representation of a unit-like enum value (e.g., `"MAX_TOKENS"`).
pub(crate) fn wire_name<T: serde::Serialize>(value: &T) -> String {
    match serde_json::to_value(value) {
        Ok(serde_json::Value::String(s)) => s,
        _ => String::new(),
    }
}
