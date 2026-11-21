//! The `markhor` command-line interface.

mod commands;

use anyhow::Result;
use clap::{Parser, Subcommand};
use tracing_subscriber::EnvFilter;

use commands::{chat::ChatArgs, import::ImportArgs};

/// Markhor: chat with AI models and import documents into collections.
#[derive(Parser)]
#[command(version, about)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Chat with an AI assistant in the terminal.
    Chat(ChatArgs),
    /// Import a document, recognizing its text with OCR.
    Import(ImportArgs),
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

    match Cli::parse().command {
        Command::Chat(args) => commands::chat::run(args).await,
        Command::Import(args) => commands::import::run(args).await,
    }
}
