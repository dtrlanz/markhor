
use std::{path::{Path, PathBuf}, sync::Arc};

// Import argument structs from the cli module
use crate::{app::Markhor, cli::{
    ChatArgs, ConfigArgs, ConfigCommands, ImportArgs, InstallArgs, OpenArgs, SearchArgs, ShowArgs, WorkspaceArgs, WorkspaceCommands
}};
use anyhow::Result;
use markhor_core::storage::Workspace;
use tracing::{error, info};
use uuid::Uuid;

// --- Handler Functions ---

pub async fn handle_import(args: ImportArgs, markhor: Markhor) -> Result<()> {
    println!("Executing import command for: {:?}", args.paths);
    // TODO: Implement import logic using markhor-core
    // - Validate paths
    // - Iterate through paths/files
    // - Call core library function to parse, embed, and store documents
    // - Handle metadata and tags
    // - Use args.model if provided, otherwise use default from config/workspace
    // - Show progress indicator (indicatif)

    for path in args.paths {
        markhor.import(&*path).await?; // Assuming import is a method in Markhor
        println!("  Imported: {}", path.display());
    }

    Ok(())
}

pub async fn handle_chat(args: ChatArgs, markhor: Markhor) -> Result<()> {
    println!("Executing chat command...");
    if let Some(prompt) = args.prompt.as_ref() {
        println!("  Initial prompt: {}", prompt);
    }
    // TODO: Implement chat logic using markhor-core
    // - Identify chat model (args.model or default)
    // - Determine context/scope (args.scope -> retrieve relevant docs/embeddings)
    // - Start interactive session or handle single prompt
    // - Manage chat history
    // - Handle plugins (args.plugins)

    let paths = args.scope.iter()
        .map(|s| PathBuf::from(s))
        .collect::<Vec<_>>();

    markhor.chat(args).await?;
    Ok(())
}

pub async fn handle_show(args: ShowArgs, markhor: Markhor) -> Result<()> {
    if let Some(doc_arg) = args.document {
        let doc = match Uuid::try_parse(&doc_arg) {
            Ok(uuid) => {
                println!("Executing show command for document ID: {}", uuid);
                anyhow::bail!("ID-based document retrieval not implemented yet.");
            }
            Err(_) => {
                println!("Executing show command for document: {}", doc_arg);
                markhor.workspace?.document(&*PathBuf::from(doc_arg)).await?
            }
        };
        
        let metadata = doc.read_metadata().await?;
        println!("  Document Path: {:?}", doc.path().display());
        println!("  Document ID: {:?}", doc.id());

        let files = metadata.files_with_metadata().collect::<Vec<_>>();

        if args.metadata {
            println!();
            println!("  Document metadata version: {:?}", metadata.markhor_version());
            println!("  Files with metadata: {}", files.join(", "));
        }

        if args.embeddings {
            let embedders = markhor.extensions.iter()
                .flat_map(|ext| ext.embedders())
                .collect::<Vec<_>>();

            for embedder in embedders {
                println!();
                println!("  Embedding model: {}", embedder.model_name());
                for &file in files.iter() {
                    if let Some(file_embeddings) = metadata.file(file)
                        .and_then(|md| md.embeddings(&embedder)) 
                    {
                        println!();
                        println!("  File: {}", file);
                        println!("    No. |     Range    | Bytes | Tokens | Heading");
                        println!("    ----|--------------|-------|--------|-------------------------------------------------------------------");
                        

                        for (idx, (_, chunk_data)) in file_embeddings.iter().enumerate() {
                            print!("   {:4} | {:5?} | {:5} | ", idx, chunk_data.text_range, chunk_data.text_range.len());
                            if let Some(token_count) = chunk_data.token_count {
                                print!("{:6} | ", token_count);
                            } else {
                                print!("  --   | ");
                            }
                            if let Some(heading_path) = chunk_data.heading_path.as_ref() {
                                const HEADING_LENGTH_CUTOFF: usize = 70;
                                if heading_path.len() <= HEADING_LENGTH_CUTOFF {
                                    print!("{}", heading_path);
                                } else {
                                    let mut i = heading_path.len() - HEADING_LENGTH_CUTOFF;
                                    while !heading_path.is_char_boundary(i) {
                                        i -= 1;
                                    }
                                    print!("...{}.", &heading_path[i..]);
                                }
                            }
                            println!();
                        }
                    }
                }
            }
        }
        
    } else {
        println!("Executing show command for workspace info.");
        // TODO: Implement logic to show workspace overview
        // - Number of documents, models used, config summary, etc.
    }
    Ok(())
}

pub async fn handle_open(args: OpenArgs) -> Result<()> {
    println!("Executing open command for document: {}", args.document_id);
    // TODO: Implement open logic
    // - Call core library to resolve document ID to a file path
    // - Use crate like `opener` or `open` to open the file with the default system application
    Ok(())
}

pub async fn handle_search(args: SearchArgs, markhor: Markhor) -> Result<()> {
    let paths = args.scope.iter()
        .map(|s| PathBuf::from(s))
        .collect::<Vec<_>>();

    markhor.search(&args.query, args.limit, paths).await?;
    Ok(())
}

pub async fn handle_install(args: InstallArgs) -> Result<()> {
    println!("Executing install command...");
    // TODO: Define plugin structure and implement installation logic
    if let Some(path) = args.path {
        println!("  Installing from path: {}", path.display());
    } else if let Some(url) = args.url {
        println!("  Installing from URL: {}", url);
    }
    unimplemented!("Plugin installation not implemented yet");
    // Ok(())
}

pub async fn handle_config(args: ConfigArgs) -> Result<()> {
    println!("Executing config command...");
    match args.command {
        ConfigCommands::Get { key, global } => {
            println!("  Getting config key: '{}', global: {}", key, global);
            // TODO: Implement config get logic
            // - Load appropriate config (global or workspace)
            // - Retrieve and print value
            // - Handle key not found
        }
        ConfigCommands::Set { key, value, global } => {
             println!("  Setting config key: '{}' to '{}', global: {}", key, value, global);
             // TODO: Implement config set logic
             // - Load appropriate config
             // - Update or add key/value
             // - Save config file
        }
        ConfigCommands::List { global } => {
            println!("  Listing config, global: {}", global);
            // TODO: Implement config list logic
            // - Load appropriate config
            // - Print all key-value pairs
        }
        ConfigCommands::Locate {} => {
            println!("  Locating config files...");
            // TODO: Implement config locate logic
            // - Determine and print global config path (using dirs crate)
            // - Determine and print current workspace config path (if in a workspace)
        }
    }
    Ok(())
}

pub async fn handle_workspace(args: WorkspaceArgs, markhor: Markhor) -> Result<()> {
    match args.command {
        WorkspaceCommands::Create { path, name } => {
            if markhor.workspace.is_ok() {
                error!("Cannot create a new workspace while in an existing one.");
                return Err(anyhow::anyhow!("Cannot create a new workspace while in an existing one."));
            }

            // Check if any descendant directories already contain workspaces
            // Todo

            let mut target_path = path.unwrap_or_else(|| std::env::current_dir().expect("Cannot get current dir"));
            info!("Creating workspace at: {}", target_path.display());
            println!("  Creating workspace at: {}", target_path.display());
            if let Some(n) = name {
                println!("  Named: {}", n);
                target_path.push(n);
            }

            let new_ws = Workspace::create(&markhor.storage, &target_path).await?;
            println!("  Workspace created successfully.");
        }
        WorkspaceCommands::List {} => {
            println!("  Listing workspaces...");
            // TODO: Implement workspace listing (deferred - requires global tracking)
            unimplemented!("Workspace listing requires global tracking (deferred)");
        }
        WorkspaceCommands::Delete { target, force } => {
            println!("  Deleting workspace: {}", target);
            if force { println!("  Forcing deletion (no confirmation)."); }
            // TODO: Implement workspace deletion
            // - Resolve target (path or name)
            // - Ask for confirmation if not forced
            // - Remove `.markhor` directory and potentially global registration (deferred)
            unimplemented!("Workspace deletion not implemented yet");
        }
        WorkspaceCommands::Info{ target } => {
            if let Ok(ws) = markhor.workspace {
                println!("  Current workspace: {}", ws.path().display());
            } else {
                println!("  No current workspace found.\n{}", markhor.workspace.err().unwrap());
            }
        }
    }
    Ok(())
}