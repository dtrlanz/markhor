mod document;
mod folder;
mod workspace;

use std::path::PathBuf;
use thiserror::Error;

pub use document::Document;
pub use folder::Folder;
pub use workspace::Workspace;


const METADATA_EXTENSION: &str = "mark";
const ATTACHMENTS_DIR: &str = "attachments";
const WORKSPACE_CONFIG_DIR: &str = ".markhor";
const WORKSPACE_METADATA_FILENAME: &str = "workspace.yaml";


#[derive(Debug, Error)]
pub enum AccessStorageError {
    #[error("IO error")]
    Io(#[from] std::io::Error),

    #[error("File not found: {0}")]
    FileNotFound(PathBuf),

    #[error("Directory not found: {0}")]
    DirectoryNotFound(PathBuf),

    #[error("Path is not a directory: {0}")]
    NotADirectory(PathBuf),

    #[error("Metadata serialization/deserialization error")]
    Metadata2(#[from] serde_yaml_ng::Error),

    #[error("Invalid ID: {0}")]
    InvalidId(String),
}

