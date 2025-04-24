use std::path::PathBuf;
use std::pin::Pin;
use std::future::Future;
use std::sync::Arc;
use std::task;

use anyhow::Result;
use clap::Parser;
use markhor::app::Markhor;
use markhor::cli::{Cli, Commands};
use markhor::commands;
use markhor_core::extension::Extension;
use markhor_core::storage::{Storage, Workspace};
use markhor_extensions::gemini::GeminiClientExtension;
use markhor_extensions::ocr::mistral::client::MistralClient;
use reqwest::Client;
use tracing::{debug, error, info, Level};
use tracing_subscriber::EnvFilter;


#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    // --- Tracing Initialization ---
    
    // Initialize tracing
    setup_tracing(cli.verbose, cli.quiet);

    // Log that tracing is set up (this will now be captured)
    tracing::debug!(args = ?cli, "Markhor CLI arguments parsed");

    // --- Configuration Loading ---

    let mut extensions: Vec<Arc<dyn Extension>> = vec![];

    // Process env vars
    dotenv::dotenv().ok();

    // Load Google API key from env var
    match std::env::var("GOOGLE_API_KEY") {
        Ok(key) => {
            info!("Google API key loaded from environment variables");
            match GeminiClientExtension::new(key) {
                Ok(ext) => extensions.push(Arc::new(ext)),
                Err(e) => {
                    error!("Failed to construct Gemini extension: {}", e);
                }
            }
        },
        Err(_) => {
            debug!("Google API key not found in environment variables");
        }
    };
    
    // Load Mistral API key from env var
    match std::env::var("MISTRAL_API_KEY") {
        Ok(key) => {
            info!("Mistral API key loaded from environment variables");
            extensions.push(Arc::new(MistralClient::new(key)));
        },
        Err(_) => {
            debug!("Mistral API key not found in environment variables");
        }
    };

    // --- Workspace Initialization ---

    let storage = Arc::new(Storage::new());
    let workspace = get_workspace(&storage, cli.workspace.clone()).await;
    // If the workspace is found, use current directory as default folder
    let folder = match &workspace {
        Ok(ws) => match std::env::current_dir() {
            Ok(cwd) => ws.folder(&cwd).await.ok(),
            Err(e) => {
                error!("Could not get current directory: {}", e);
                return Err(e.into());
            }
        }
        Err(e) => None
    };
    let app = Markhor {
        storage,
        workspace,
        folder,
        extensions,
    };

    // --- Command Dispatching ---

    // Match the command and call the appropriate handler function
    tracing::debug!(command = ?cli.command, "Dispatching command");
    let command_result = match cli.command {
        Commands::Import(args) => {
            println!("Importing with args: {:?}", args);
            commands::handle_import(args, app).await
        }
        Commands::Chat(args) => {
            println!("Chatting with args: {:?}", args);
            commands::handle_chat(args, app).await
        }
        Commands::Show(args) => {
            println!("Showing info with args: {:?}", args);
            commands::handle_show(args).await
        }
        Commands::Open(args) => {
            println!("Opening document with args: {:?}", args);
            commands::handle_open(args).await
        }
        Commands::Search(args) => {
            println!("Searching with args: {:?}", args);
            commands::handle_search(args).await
        }
        Commands::Install(args) => {
            println!("Installing plugin with args: {:?}", args);
            commands::handle_install(args).await
        }
        Commands::Config(args) => {
            println!("Managing config with args: {:?}", args);
            commands::handle_config(args).await
            // This command itself has subcommands, so handle_config will need its own match
        }
        Commands::Workspace(args) => {
            println!("Managing workspace with args: {:?}", args);
            commands::handle_workspace(args, app).await
             // This command itself has subcommands, so handle_workspace will need its own match
        }
    };

    // --- Command Result Handling ---

    if let Err(e) = command_result {
        // Log the detailed error using tracing
        // The {:?} format for anyhow::Error provides the context chain.
        tracing::error!(error = ?e, "Command failed");

        // Let anyhow print the user-facing error message to stderr automatically
        // when `main` returns the error.
        return Err(e);
    }

    tracing::debug!("Command executed successfully");
    Ok(())
}

/// Sets up the tracing subscriber.
/// Respects RUST_LOG environment variable first, then falls back
/// to verbosity flags (-v, -q).
fn setup_tracing(verbosity: u8, quiet: bool) {
    // RUST_LOG takes precedence over -v/ -q flags
    let filter = match std::env::var("RUST_LOG") {
        Ok(env_var) if !env_var.is_empty() => {
            EnvFilter::new(env_var)
        }
        // RUST_LOG is not set or empty, use CLI flags
        _ => {
            let base_level = if quiet {
                return;
            } else {
                match verbosity {
                    0 => Level::WARN,  // Default
                    1 => Level::INFO, // -v
                    2 => Level::DEBUG, // -vv
                    _ => Level::TRACE, // -vvv or more
                }
            };

            // You might want to quiet down noisy dependencies by default
            // For example: "info,tokio=warn,reqwest=warn"
            let filter_directives = format!("{},hyper=warn,reqwest=warn", base_level); // Adjust deps as needed

            EnvFilter::new(filter_directives)
        }
    };


    // Configure the fmt subscriber
    let subscriber = tracing_subscriber::fmt()
        .compact() // Use compact formatting
        .with_env_filter(filter)
        .with_level(true) // Include the level in the output (e.g., INFO, DEBUG)
        .with_target(true) // Include the module path in the output
        //.with_timer(tracing_subscriber::fmt::time::SystemTime)
        .with_writer(std::io::stderr) // Log to stderr to avoid interfering with stdout output
        .finish(); // Build the subscriber

    // Set the global default subscriber. `try_init` returns an error if already set.
    tracing::subscriber::set_global_default(subscriber)
        .expect("Failed to set global default tracing subscriber");

    // Log that tracing was initialized (this might be the first message)
    tracing::debug!("Tracing initialized");
}


async fn get_workspace(storage: &Arc<Storage>, cli_ws_flag: Option<PathBuf>) -> Result<Arc<Workspace>> {
    if let Some(ws_path) = cli_ws_flag {
        // Open the workspace at the specified path
        debug!("Opening workspace at: {}", ws_path.display());
        let ws = Workspace::open(storage, &ws_path).await;
        if let Err(e) = ws {
            error!("Could not open workspace specified in workspace flag: {}", e);
            return Err(anyhow::anyhow!("Failed to open workspace at {}: {}", ws_path.display(), e));
        }
        return Ok(ws?);
    } else {
        // If no workspace is specified, find it in the current directory or its parents
        debug!("Finding workspace in current directory or its parents");
        // Start from the current directory
        let mut dir = std::env::current_dir()?;
        while let Some(parent) = dir.parent() {
            match Workspace::open(storage, &*dir).await {
                Ok(ws) => {
                    info!("Found workspace at: {}", dir.display());
                    return Ok(ws);
                }
                Err(e) => {
                    debug!("No workspace found in: {}", dir.display());
                    // If the workspace is not found, continue to the parent directory
                    dir = parent.to_path_buf();
                }
            }
        }
    };

    // If no workspace is found in the current directory or its parents, return an error
    // Logging info (not error) because not all commands require a workspace.
    info!("Could not find workspace in current directory or its parents");
    // Returning error (not option), which can be propagated/displayed later if a workspace was
    // actually required.
    Err(anyhow::anyhow!("No workspace found in current directory or its parents"))
}

