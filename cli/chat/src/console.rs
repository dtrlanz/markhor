//! Exposes the console chat UI as an extension, so that a `Session` can resolve it.

use markhor_cli::ConsoleChatUi;
use markhor_core::{chat::ChatUi, extension::Extension};

#[derive(Clone, Default)]
pub struct ConsoleExtension {
    ui: ConsoleChatUi,
}

impl Extension for ConsoleExtension {
    fn uri(&self) -> &str {
        "markhor://console-ui"
    }

    fn name(&self) -> &str {
        "Console chat UI"
    }

    fn description(&self) -> &str {
        "Terminal chat interface"
    }

    fn chat_uis(&self) -> Vec<Box<dyn ChatUi>> {
        vec![Box::new(self.ui.clone())]
    }
}
