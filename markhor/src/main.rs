use std::path::PathBuf;
use std::pin::Pin;
use std::future::Future;
use std::sync::Arc;
use std::task;

use anyhow::Result;
use clap::Parser;
use markhor::cli::{Cli, Commands};
use markhor::{commands, AppContext};
use markhor_core::storage::{Storage, Workspace};
use async_once_cell::OnceCell;

static STORAGE: OnceCell<Arc<Storage>> = OnceCell::new();
//static WORKSPACE: OnceCell<Arc<Workspace>> = OnceCell::new();

struct F;
impl Future for F {
    type Output = Storage;
    fn poll(self: Pin<&mut Self>, _: &mut task::Context) -> task::Poll<Storage> {
        return task::Poll::Ready(Storage::new());
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    STORAGE.get_or_init(async { Arc::new(Storage::new()) }).await;

    // TODO: Initialize logging/tracing (e.g., tracing_subscriber::fmt::init();)
    // TODO: Initialize configuration loading (env vars, config files) -> store in some AppContext struct?

    let cli = Cli::parse();

    // Example of using verbose/quiet flags (you'll need a proper logging setup)
    // let log_level = match cli.verbose {
    //     0 => tracing::Level::WARN, // Default level (adjust as needed)
    //     1 => tracing::Level::INFO,
    //     2 => tracing::Level::DEBUG,
    //     _ => tracing::Level::TRACE,
    // };
    // if cli.quiet {
    //     // Set level higher than ERROR or disable specific targets
    // }
    // Initialize your chosen logger here with the calculated level

    let ws = get_workspace(cli.workspace.clone()).await;
    let cx = AppContext {
        storage: STORAGE.get().unwrap().clone(),
        workspace: ws,
    };

    // Match the command and call the appropriate handler function
    match cli.command {
        Commands::Import(args) => {
            println!("Importing with args: {:?}", args);
            commands::handle_import(args).await?;
        }
        Commands::Chat(args) => {
            println!("Chatting with args: {:?}", args);
            commands::handle_chat(args).await?;
        }
        Commands::Show(args) => {
            println!("Showing info with args: {:?}", args);
            commands::handle_show(args).await?;
        }
        Commands::Open(args) => {
            println!("Opening document with args: {:?}", args);
            commands::handle_open(args).await?;
        }
        Commands::Search(args) => {
            println!("Searching with args: {:?}", args);
            commands::handle_search(args).await?;
        }
        Commands::Install(args) => {
            println!("Installing plugin with args: {:?}", args);
            commands::handle_install(args).await?;
        }
        Commands::Config(args) => {
            println!("Managing config with args: {:?}", args);
            commands::handle_config(args).await?;
            // This command itself has subcommands, so handle_config will need its own match
        }
        Commands::Workspace(args) => {
            println!("Managing workspace with args: {:?}", args);
            commands::handle_workspace(args, cx).await?;
             // This command itself has subcommands, so handle_workspace will need its own match
        }
    }

    Ok(())
}

async fn get_workspace(cli_ws_flag: Option<PathBuf>) -> Result<Arc<Workspace>> {
    let storage = STORAGE.get().unwrap();

    if let Some(ws_path) = cli_ws_flag {
        // Open the workspace at the specified path
        let ws = Workspace::open(STORAGE.get().unwrap(), &ws_path).await;
        if let Err(e) = ws {
            return Err(anyhow::anyhow!("Failed to open workspace at {}: {}", ws_path.display(), e));
        }
        return Ok(ws?);
    } else {
        // If no workspace is specified, find it in the current directory or its parents
        let mut dir = std::env::current_dir()?;
        while let Some(parent) = dir.parent() {
            let try_open = Workspace::open(storage, &*dir).await;
            if let Ok(ws) = try_open {
                return Ok(ws);
            }
            println!("No workspace found: {}", try_open.unwrap_err());
    
            // Move to the parent directory
            dir = parent.to_path_buf();
        }
    };

    Err(anyhow::anyhow!("No workspace found in current directory or its parents"))
}

