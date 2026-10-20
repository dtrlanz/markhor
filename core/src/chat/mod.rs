pub mod chat;
pub mod error;
mod group;
mod msg;
pub mod prompter;

pub use error::ChatError;
pub use group::{AddMemberError, AppendMessageError, Group, Member, MemberId};
pub use msg::Msg;
