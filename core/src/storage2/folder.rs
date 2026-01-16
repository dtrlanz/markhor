use std::{path::{Path, PathBuf}, sync::Arc};

use tracing::instrument;

use crate::storage2::{Document, AccessStorageError};

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
        Folder { absolute_path, workspace }
    }

    /// Returns the relative path to the folder within its workspace.
    pub fn path(&self) -> &Path {
        self.absolute_path.strip_prefix(&self.workspace.absolute_path).unwrap()
    }

    /// Returns the name of the folder.
    pub fn name(&self) -> &str {
        self.absolute_path.file_name().unwrap().to_str()
            .expect("Not supported: Folder name is not valid UTF-8")
    }

    /// Opens the document with the specified name within this folder.
    /// 
    /// The document name should not include the `.markhor` extension.
    /// 
    /// # Errors
    /// 
    /// Returns an error if the document cannot be opened or does not exist.
    #[instrument(skip(self), fields(folder_path = %self.absolute_path.display()))]
    pub async fn document_by_name(&self, name: &str) -> Result<Document, AccessStorageError> {
        let document_path = self.absolute_path.join(name);
        Document::open(document_path, self.workspace.clone()).await
    }
}