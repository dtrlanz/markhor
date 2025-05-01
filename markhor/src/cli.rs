use clap::{Args, Parser, Subcommand};
use std::path::PathBuf;

/// Markhor: Interact with AI models and local files.
#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
#[command(propagate_version = true)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Commands,

    /// Override the default workspace path detection.
    #[arg(long, global = true)]
    pub workspace: Option<PathBuf>,

    /// Increase verbosity (use multiple times for more).
    #[arg(short, long, action = clap::ArgAction::Count, global = true)]
    pub verbose: u8,

    /// Suppress all output except errors.
    #[arg(short, long, global = true)]
    pub quiet: bool,

    // Consider adding --output-format (text, json, yaml) later
}

#[derive(Subcommand, Debug)]
pub enum Commands {
    /// Import documents into the current workspace.
    Import(ImportArgs),
    /// Start an interactive chat session.
    Chat(ChatArgs),
    /// Show information about documents or the workspace.
    Show(ShowArgs),
    /// Open a document using the system's default application.
    Open(OpenArgs),
    /// Search for documents within the workspace.
    Search(SearchArgs),
    /// Install plugins (details TBD).
    Install(InstallArgs),
    /// Manage Markhor configuration.
    Config(ConfigArgs),
    /// Manage Markhor workspaces.
    Workspace(WorkspaceArgs),
    // Help is usually handled automatically by clap
    // Help(HelpArgs),
}

// --- Argument Structs for each Subcommand ---

#[derive(Args, Debug)]
pub struct ImportArgs {
    /// Path(s) to the document(s) or directory to import.
    #[arg(required = true)]
    pub paths: Vec<PathBuf>,

    /// Attach metadata key-value pairs (e.g., --metadata source=web type=pdf).
    #[arg(long, value_parser = clap::value_parser!(String))] // Basic parsing, needs refinement later for key=value
    pub metadata: Vec<String>, // TODO: Parse into HashMap<String, String>

    /// Attach tags to the imported document(s).
    #[arg(long, short)]
    pub tags: Vec<String>,

    /// Specify the embedding model to use (overrides workspace default).
    #[arg(long)]
    pub model: Option<String>,
    // Add other import options like recursion depth, file type filters etc.
}

#[derive(Args, Debug)]
pub struct ChatArgs {
    /// Initial prompt to start the chat with.
    #[arg(long, short)]
    pub prompt: Option<String>,

    /// Specify the chat model to use (overrides default).
    #[arg(long, short)]
    pub model: Option<String>,

    /// Limit the chat scope to specific documents or tags.
    #[arg(long)]
    pub scope: Vec<String>, // TODO: Define syntax (e.g., tag:meeting, doc:report.pdf)

    /// Enable specific plugins for this session.
    #[arg(long)]
    pub plugins: Vec<String>,
    // Add options for temperature, context window, etc.
}

#[derive(Args, Debug)]
pub struct ShowArgs {
    /// Path of the document to show details for. If omitted, shows workspace info.
    pub document: Option<String>,

    /// Show document metadata.
    #[arg(long, short)]
    pub metadata: bool,

    /// Show document embedding information (e.g., model used, dimensions).
    #[arg(long, short)]
    pub embeddings: bool,
    // Add options to show tags, content snippet, etc.
}

#[derive(Args, Debug)]
pub struct OpenArgs {
    /// ID or path of the document to open.
    #[arg(required = true)]
    pub document_id: String,
}

#[derive(Args, Debug)]
pub struct SearchArgs {
    /// The search query.
    #[arg(required = true)]
    pub query: String,

    /// Specify the embedding model to use for the query (if applicable).
    #[arg(long, short)]
    pub model: Option<String>,

    /// Limit the search scope to specific documents or tags.
    #[arg(long)]
    pub scope: Vec<String>, // TODO: Define syntax

    /// Number of results to return.
    #[arg(long, short, default_value = "10")]
    pub limit: usize,
}

#[derive(Args, Debug)]
pub struct InstallArgs {
    /// Path to a local plugin.
    #[arg(long, conflicts_with = "url")]
    pub path: Option<PathBuf>,

    /// URL to a plugin repository or manifest.
    #[arg(long, conflicts_with = "path")]
    pub url: Option<String>,
    // Need to define plugin format/source later
}

#[derive(Args, Debug)]
pub struct ConfigArgs {
    #[command(subcommand)]
    pub command: ConfigCommands,
}

#[derive(Subcommand, Debug)]
pub enum ConfigCommands {
    /// Get the value of a configuration key.
    Get {
        /// The configuration key (e.g., `ai.default_model`, `user.name`).
        key: String,
        /// View the global configuration instead of the workspace configuration.
        #[arg(long)]
        global: bool,
    },
    /// Set a configuration key to a value.
    Set {
        /// The configuration key (e.g., `ai.default_model`, `user.name`).
        key: String,
        /// The value to set.
        value: String,
        /// Set the value in the global configuration instead of the workspace configuration.
        #[arg(long)]
        global: bool,
    },
    /// List all configuration keys and values.
    List {
        /// List the global configuration instead of the workspace configuration.
        #[arg(long)]
        global: bool,
    },
    /// Show the location of the configuration file(s).
    Locate {},
}

#[derive(Args, Debug)]
pub struct WorkspaceArgs {
    #[command(subcommand)]
    pub command: WorkspaceCommands,
}

#[derive(Subcommand, Debug)]
pub enum WorkspaceCommands {
    /// Create and initialize a new workspace at the specified path. Defaults to current directory.
    Create {
        /// Optional path where the new workspace should be created.
        path: Option<PathBuf>,
        /// Name for the workspace (optional, might be stored globally).
        #[arg(long, short)]
        name: Option<String>,
    },
    /// List known workspaces (requires global tracking).
    List {},
    /// Delete a workspace (potentially requires confirmation).
    Delete {
        /// Path or name of the workspace to delete.
        target: String, // Could be path or name
        #[arg(long, short)]
        force: bool, // Skip confirmation
    },
    /// Show information about the current or a specified workspace.
    Info { // Added this based on ShowArgs having workspace info possibility
         /// Path or name of the workspace to show info for. Defaults to current.
        target: Option<String>,
    }
    // 'Open' might not be needed if we use directory-based detection primarily.
    // If global tracking exists, 'open' could switch the 'current' global default.
}

// HelpArgs might not be needed if using clap's built-in help
// #[derive(Args, Debug)]
// pub struct HelpArgs {
//     /// Specific command to get help for.
//     pub command: Option<String>,
// }