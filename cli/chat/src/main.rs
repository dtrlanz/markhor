//! Console chat with a Gemma model served by the Gemini API.

mod console;

use anyhow::{Context, Result};
use clap::Parser;
use markhor::chat::Simple;
use markhor_core::{
    chat::ChatModel,
    dependencies::Session,
    extension::{Comp, ExtensionConfig},
};
use markhor_extensions::google::{Client, ClientConfig};
use tracing_subscriber::EnvFilter;

use console::ConsoleExtension;

const COMMANDS: [&str; 2] = ["help", "exit"];

/// Chat with a Gemma model in the terminal.
#[derive(Parser)]
#[command(version, about)]
struct Args {
    /// Exact Gemma model id to use, e.g. `gemma-4-31b-it`. Defaults to the first listed Gemma 4 model.
    #[arg(long)]
    model: Option<String>,
}

#[tokio::main]
async fn main() -> Result<()> {
    dotenv::dotenv().ok();
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("warn")),
        )
        .with_writer(std::io::stderr)
        .init();
    let args = Args::parse();
    let allow = args.model.unwrap_or_else(|| "gemma-4*".to_string());

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
