//! Console chat with a model served by the Gemini API.

use anyhow::{Context, Result};
use clap::Args;
use markhor::chat::Simple;
use markhor_cli::ConsoleChatUi;
use markhor_core::{
    chat::{ChatModel, ChatUi},
    dependencies::Session,
    extension::{Comp, Extension, ExtensionConfig},
};
use markhor_extensions::google::{Client, ClientConfig};

const COMMANDS: [&str; 2] = ["help", "exit"];

/// Chat with a model served by the Gemini API in the terminal.
#[derive(Args)]
pub struct ChatArgs {
    /// Exact model id to use, e.g. `gemma-4-31b-it`. Defaults to `gemini-flash-lite-latest`.
    #[arg(long)]
    model: Option<String>,
}

/// Exposes the console chat UI as an extension, so that a `Session` can resolve it.
#[derive(Clone, Default)]
struct ConsoleExtension {
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

pub async fn run(args: ChatArgs) -> Result<()> {
    let allow = args
        .model
        .unwrap_or_else(|| "gemini-flash-lite-latest".to_string());

    let mut session = Session::new();
    session
        .add_api_keys_from_env()
        .context("invalid API key environment variables")?;
    let client_config = ClientConfig {
        allow_models: vec![allow.clone()],
        ..Default::default()
    };
    session
        .add_extension::<ClientConfig, Client>(client_config, ExtensionConfig::default())
        .await
        .context("failed to add the Google client")?;
    session
        .add_extension::<ConsoleExtension, ConsoleExtension>(
            ConsoleExtension::default(),
            ExtensionConfig::default(),
        )
        .await
        .context("failed to add the console UI")?;

    session
        .resolve::<Comp<dyn ChatModel>>()
        .with_context(|| format!("no chat model matching {allow:?} is available"))?;

    let chat = Simple::with_commands(
        session,
        "User",
        COMMANDS.iter().map(|c| c.to_string()).collect(),
    )?;
    // Ctrl-C ends the chat even when no prompt is reading, which would otherwise
    // be killed by the default handler.
    tokio::select! {
        _ = chat.run(|name, _args| {
            if name == "exit" {
                return false;
            }
            println!("Commands: {}", COMMANDS.map(|c| format!("/{c}")).join(", "));
            true
        }) => {}
        _ = tokio::signal::ctrl_c() => {}
    }

    // A prompt may still be blocked reading the terminal, which would stall
    // runtime shutdown, so exit explicitly.
    std::process::exit(0)
}
