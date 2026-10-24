pub mod chat;
pub mod error;
mod group;
mod member;
mod msg;
pub mod prompter;

pub use error::ChatError;
pub use group::{AddMemberError, AppendMessageError, Group, InitContext};
pub use member::{Member, MemberId};
pub use msg::{IterMsg, Msg};
