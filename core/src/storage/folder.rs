use crate::storage::{Error, Result, INTERNAL_DIR_NAME, MARKHOR_EXTENSION};
use crate::storage::document::Document;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::fs;
use tracing::{debug, instrument, warn};

use super::Workspace;

/// Represents a directory within a Workspace or another Folder,
/// which can contain Documents and other Folders.
#[derive(Debug, Clone)]
pub struct Folder {
    // Absolute path to the folder
    absolute_path: PathBuf,
    // Workspace owning this document
    workspace: Arc<Workspace>,
}

impl Folder {
    /// Creates a Folder instance. Intended for internal use.
    /// Assumes the path already points to a valid, existing directory *inside* the workspace.
    pub(crate) fn new(absolute_path: PathBuf, workspace: Arc<Workspace>) -> Self {
        // Consider adding an assertion or quick check in debug mode?
        // debug_assert!(path.is_dir(), "Folder::new called with non-directory path");
        Folder { absolute_path, workspace }
    }

    /// Returns the path to this folder's directory.
    pub fn path(&self) -> &Path {
        &self.absolute_path.strip_prefix(&self.workspace.absolute_path)
            .expect("Internal error: Document is not in workspace")
    }

    /// Returns the name of the folder.
    pub fn name(&self) -> Option<&str> {
        self.absolute_path.file_name()?.to_str()
    }

    /// Opens the document with the specified name within this folder.
    /// 
    /// The document name should not include the `.markhor` extension.
    /// 
    /// # Errors
    /// 
    /// Returns an error if the document cannot be opened or does not exist.
    #[instrument(skip(self), fields(folder_path = %self.absolute_path.display()))]
    pub async fn document_by_name(&self, name: &str) -> Result<Document> {
        let document_path = self.absolute_path.join(format!("{}.{}", name, MARKHOR_EXTENSION));
        Document::open(document_path, self.workspace.clone()).await
    }

    /// Creates a new document in this folder with the specified name.
    /// 
    /// The document name should not include the `.markhor` extension.
    /// 
    /// # Errors
    /// 
    /// Returns an error if the document cannot be created or already exists.
    #[instrument(skip(self), fields(folder_path = %self.absolute_path.display()))]
    pub async fn create_document(&self, name: &str) -> Result<Document> {
        let document_path = self.absolute_path.join(format!("{}.{}", name, MARKHOR_EXTENSION));
        Document::create(document_path, self.workspace.clone()).await
    }

    /// Creates a new subfolder within this folder with the specified name.
    /// 
    /// # Errors
    /// 
    /// Returns an error if the subfolder cannot be created or already exists.
    #[instrument(skip(self), fields(folder_path = %self.absolute_path.display()))]
    pub async fn create_subfolder(&self, name: &str) -> Result<Folder> {
        let subfolder_path = self.absolute_path.join(name);
        fs::create_dir_all(&subfolder_path).await.map_err(Error::Io)?;
        Ok(Folder::new(subfolder_path, self.workspace.clone()))
    }

    /// Lists the documents directly contained within this folder (non-recursive).
    ///
    /// Invalid `.markhor` files that fail to open will be skipped and logged as warnings.
    #[instrument(skip(self), fields(folder_path = %self.absolute_path.display()))]
    pub async fn list_documents(&self) -> Result<Vec<Document>> {
        debug!("Listing documents in directory");
        let mut documents = Vec::new();
        let mut read_dir = match fs::read_dir(&self.absolute_path).await {
            Ok(rd) => rd,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                // If the directory itself doesn't exist, return an empty list or error?
                // Let's return Ok([]) as the list of documents in a non-existent dir is empty.
                debug!("Directory not found, returning empty document list.");
                return Ok(Vec::new());
            }
            Err(e) => return Err(Error::Io(e)),
        };
    
        while let Some(entry) = read_dir.next_entry().await.map_err(Error::Io)? {
            // Note that DirEntry.path() returns an absolute path (which is what we need here)
            let path = entry.path();
            if path.is_file() {
                if path.extension().and_then(|ext| ext.to_str()) == Some(MARKHOR_EXTENSION) {
                    debug!("Found potential content file: {}", path.display());
                    match Document::open(path.clone(), self.workspace.clone()).await {
                        Ok(doc) => documents.push(doc),
                        Err(e) => {
                            // Log and skip invalid/inaccessible content files
                            warn!(
                                "Skipping invalid or inaccessible content file '{}': {}",
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

    /// Lists the subfolders directly contained within this folder (non-recursive).
    #[instrument(skip(self), fields(folder_path = %self.absolute_path.display()))]
    pub async fn list_folders(&self) -> Result<Vec<Folder>> {
        debug!("Listing subfolders");
        let mut folders = Vec::new();
        let mut read_dir = match fs::read_dir(&self.absolute_path).await {
            Ok(rd) => rd,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                debug!("Directory not found, returning empty folder list.");
                return Ok(Vec::new());
            }
            Err(e) => return Err(Error::Io(e)),
        };
    
        while let Some(entry) = read_dir.next_entry().await.map_err(Error::Io)? {
            // Note that DirEntry.path() returns an absolute path (which is what we need here)
            let path = entry.path();
            if path.is_dir() {
                if entry.file_name().to_str() == Some(INTERNAL_DIR_NAME) {
                    debug!("Skipping excluded directory: {}", path.display());
                    continue;
                }
                debug!("Found subfolder: {}", path.display());
                folders.push(Folder::new(path, self.workspace.clone()));
            }
        }
        debug!("Found {} subfolders", folders.len());
        Ok(folders)
    }

    // Potential future methods: delete, rename, move_to, create_subfolder, etc.
}

/// Represents a scope for searching or filtering documents.
#[derive(Debug, Clone)]
pub struct Scope {
    folder: Folder,
    tags: Vec<String>, // TODO - tags don't exist yet
}

impl From<Folder> for Scope {
    fn from(folder: Folder) -> Self {
        Scope {
            folder,
            tags: Vec::new(),
        }
    }
}