pub mod chat;
pub mod error;
mod group;
mod msg;
pub mod prompter;

pub use error::ChatError;
pub use group::{AddParticipantError, AppendMessageError, Group, Participant};
pub use msg::Msg;
