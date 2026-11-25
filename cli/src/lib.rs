//! A classic, stdout-first terminal UI for Markhor chats.
//!
//! [`ConsoleChatUi`] implements [`markhor_core::chat::ChatUi`]. Messages are
//! converted to HTML in a single, replaceable place ([`convert`]) and rendered
//! to the terminal from there ([`html`]), so richer formats can be added
//! without touching the UI.

pub mod convert;
pub mod html;
mod prompt;
pub mod stream;
mod term;
pub mod text;
pub mod theme;
mod ui;

pub use convert::{MarkdownToHtml, PlainToHtml, TextToHtml};
pub use stream::{AtEnd, FlushPolicy};
pub use theme::Theme;
pub use ui::{ChatUiConfig, ConsoleChatUi, ReasoningDisplay};
