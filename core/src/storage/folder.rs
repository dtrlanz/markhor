use crate::storage::{Error, Result};
use crate::storage::document::Document;
use std::path::{Path, PathBuf};
use tokio::fs;
use tracing::{debug, instrument, warn};

const MARKHOR_EXTENSION: &str = "markhor";
const INTERNAL_DIR_NAME: &str = ".markhor";

/// Represents a directory within a Workspace or another Folder,
/// which can contain Documents and other Folders.
#[derive(Debug, Clone)]
pub struct Folder {
    path: PathBuf,
}

impl Folder {
    /// Creates a Folder instance. Intended for internal use.
    /// Assumes the path already points to a valid, existing directory.
    pub(crate) fn new(path: PathBuf) -> Self {
        // Consider adding an assertion or quick check in debug mode?
        // debug_assert!(path.is_dir(), "Folder::new called with non-directory path");
        Folder { path }
    }

    /// Returns the path to this folder's directory.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Returns the name of the folder.
    pub fn name(&self) -> Option<&str> {
        self.path.file_name()?.to_str()
    }

    /// Lists the documents directly contained within this folder (non-recursive).
    ///
    /// Invalid `.markhor` files that fail to open will be skipped and logged as warnings.
    #[instrument(skip(self), fields(folder_path = %self.path.display()))]
    pub async fn list_documents(&self) -> Result<Vec<Document>> {
        list_documents_in_dir(&self.path).await
    }

    /// Lists the subfolders directly contained within this folder (non-recursive).
    #[instrument(skip(self), fields(folder_path = %self.path.display()))]
    pub async fn list_folders(&self) -> Result<Vec<Folder>> {
        list_folders_in_dir(&self.path, None).await // No directory to exclude
    }

    // Potential future methods: delete, rename, move_to, create_subfolder, etc.
}

// --- Helper function for listing documents (used by Folder and Workspace) ---
#[instrument(skip(dir_path), fields(path = %dir_path.display()))]
pub(crate) async fn list_documents_in_dir(dir_path: &Path) -> Result<Vec<Document>> {
    debug!("Listing documents in directory");
    let mut documents = Vec::new();
    let mut read_dir = match fs::read_dir(dir_path).await {
        Ok(rd) => rd,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            // If the directory itself doesn't exist, return an empty list or error?
            // Let's return Ok([]) as the list of documents *in* a non-existent dir is empty.
             debug!("Directory not found, returning empty document list.");
            return Ok(Vec::new());
        }
        Err(e) => return Err(Error::Io(e)),
    };

    while let Some(entry) = read_dir.next_entry().await.map_err(Error::Io)? {
        let path = entry.path();
        if path.is_file() {
            if path.extension().and_then(|ext| ext.to_str()) == Some(MARKHOR_EXTENSION) {
                debug!("Found potential document file: {}", path.display());
                match Document::open(path.clone()).await {
                    Ok(doc) => documents.push(doc),
                    Err(e) => {
                        // Log and skip invalid/inaccessible document files
                        warn!(
                            "Skipping invalid or inaccessible document file '{}': {}",
                            path.display(),
                            e
                        );
                    }
                }
            }
        }
    }
    debug!("Found {} valid documents", documents.len());
    Ok(documents)
}

// --- Helper function for listing folders (used by Folder and Workspace) ---
#[instrument(skip(dir_path, exclude_dir_name), fields(path = %dir_path.display()))]
pub(crate) async fn list_folders_in_dir(
    dir_path: &Path,
    exclude_dir_name: Option<&str>,
) -> Result<Vec<Folder>> {
    debug!("Listing subfolders");
    let mut folders = Vec::new();
     let mut read_dir = match fs::read_dir(dir_path).await {
        Ok(rd) => rd,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            debug!("Directory not found, returning empty folder list.");
            return Ok(Vec::new());
        }
        Err(e) => return Err(Error::Io(e)),
    };

    while let Some(entry) = read_dir.next_entry().await.map_err(Error::Io)? {
        let path = entry.path();
        if path.is_dir() {
            if let Some(exclude_name) = exclude_dir_name {
                if entry.file_name().to_str() == Some(exclude_name) {
                    debug!("Skipping excluded directory: {}", path.display());
                    continue;
                }
            }
            debug!("Found subfolder: {}", path.display());
            folders.push(Folder::new(path));
        }
    }
    debug!("Found {} subfolders", folders.len());
    Ok(folders)
}