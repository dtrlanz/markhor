use std::{backtrace, path::{Path, PathBuf}, sync::Arc};

use tokio::fs;
use tracing::instrument;

use crate::storage2::{AccessStorageError, Document, document::open_document, workspace::find_workspace_descendant};

use super::Workspace;

/// Represents a directory within a Workspace or another Folder,
/// which can contain Documents and other Folders.
#[derive(Debug, Clone)]
pub struct Folder {
    // Absolute path to the folder
    absolute_path: PathBuf,
    // Workspace owning this document
    workspace: Workspace,
}

impl Folder {
    /// Creates a Folder instance. Intended for internal use.
    /// Assumes the path already points to a valid, existing directory *inside* the workspace.
    pub(crate) fn new(absolute_path: PathBuf, workspace: Workspace) -> Self {
        Folder { absolute_path, workspace }
    }

    /// Returns the relative path to the folder within its workspace.
    pub fn path(&self) -> &Path {
        self.absolute_path.strip_prefix(&self.workspace.path()).unwrap()
    }

    /// Returns the name of the folder.
    pub fn name(&self) -> &str {
        self.absolute_path.file_name().unwrap().to_str()
            .expect("Not supported: Folder name is not valid UTF-8")
    }

    /// Returns the workspace owning this folder.
    pub fn workspace(&self) -> &Workspace {
        &self.workspace
    }

    /// Opens the specified directory as a workspace folder.
    /// 
    /// If the directory is located inside of an explicit workspace (with a workspace config 
    /// directory), the directory is opened as a folder of that workspace. If no such workspace
    /// exists, the directory is opened as the root folder of an implicit workspace.
    /// 
    /// # Errors
    /// 
    /// Returns an error if the directory cannot be opened or does not exist.
    pub async fn open(path: impl AsRef<Path>) -> Result<Self, AccessStorageError> {
        let path = std::path::absolute(path)?;
        match Workspace::open(&path).await {
            Ok(workspace) => Ok(workspace.root()),
            Err(AccessStorageError::InWorkspace(ws)) => {
                println!("In workspace: {:?}", ws);
                let ws = Workspace::open(ws).await?;
                println!("Opened workspace: {:?}", ws);
                ws.folder(path).await
            },
            Err(e) => Err(e),
        }
    }

    /// Opens the folder with the specified name.
    /// 
    /// # Errors
    /// 
    /// Returns an error if the folder cannot be opened or does not exist.
    pub async fn folder(&self, name: impl AsRef<Path>) -> Result<Folder, AccessStorageError> {
        let path = name.as_ref();
        let absolute_path = if path.is_absolute() {
            path.to_path_buf()
        } else {
            self.absolute_path.join(path)
        };
        open_folder(absolute_path, self.workspace.clone()).await
    }

    /// Opens the document with the specified name.
    /// 
    /// # Errors
    /// 
    /// Returns an error if the document cannot be opened or does not exist.
    #[instrument(skip(self), fields(folder_path = %self.absolute_path.display(), name = %name.as_ref().display()))]
    pub async fn document(&self, name: impl AsRef<Path>) -> Result<Document, AccessStorageError> {
        let path = name.as_ref();
        let absolute_path = if path.is_absolute() {
            path.to_path_buf()
        } else {
            self.absolute_path.join(path)
        };
        open_document(&absolute_path, self.workspace.clone()).await
    }
}

async fn open_folder(
    absolute_path: PathBuf,
    workspace: Workspace,
) -> Result<Folder, AccessStorageError> {
    // Check if path exists and all that
    let absolute_path = fs::canonicalize(&absolute_path).await
        .map_err(|e| if e.kind() == std::io::ErrorKind::NotFound {
            AccessStorageError::DirectoryNotFound(absolute_path)
        } else {
            AccessStorageError::Io(e)
    })?;
    // Check if path is inside of workspace
    if !absolute_path.starts_with(&workspace.path()) {
        return Err(AccessStorageError::NotInWorkspace(absolute_path));
    }
    // Check if path points to a directory
    let path_metadata = fs::metadata(&absolute_path).await?;
    if !path_metadata.is_dir() {
        return Err(AccessStorageError::NotADirectory(absolute_path));
    }
    // All good
    Ok(Folder {
        absolute_path,
        workspace,
    })
}

#[cfg(test)]
mod tests {
    use crate::storage2::WORKSPACE_CONFIG_DIR;

    use super::*;

    #[tokio::test]
    async fn folder_open() {
        let dir = tempfile::tempdir().unwrap();
        let dir_path = dir.path();

        // Open path in implicit workspace
        let folder = Folder::open(&dir_path).await.unwrap();
        let ws_path = folder.workspace().path();
        assert_eq!(folder.path(), "");
        assert_eq!(
            fs::canonicalize(ws_path).await.unwrap(), 
            fs::canonicalize(dir_path).await.unwrap()
        );

        // Open path as root of explict workspace
        let config_dir = ws_path.join(WORKSPACE_CONFIG_DIR);
        fs::create_dir(&config_dir).await.unwrap();
        let folder = Folder::open(&dir_path).await.unwrap();
        assert_eq!(folder.path(), "");
        assert_eq!(
            fs::canonicalize(ws_path).await.unwrap(), 
            fs::canonicalize(dir_path).await.unwrap()
        );

        // Open path as child of explicit workspace
        let child_path = ws_path.join("child");
        fs::create_dir(&child_path).await.unwrap();
        let folder = Folder::open(&child_path).await.unwrap();
        assert_eq!(folder.path(), "child");
        let ws_path = folder.workspace().path();
        assert_eq!(
            fs::canonicalize(ws_path).await.unwrap(), 
            fs::canonicalize(dir_path).await.unwrap()
        );
    }

    #[tokio::test]
    async fn folder_document() {
        // Document without metadata
        let dir = tempfile::tempdir().unwrap();
        let dir_path = dir.path();
        let doc_path = dir_path.join("doc.md");
        fs::write(&doc_path, "foo").await.unwrap();

        let folder = Folder::open(&dir_path).await.unwrap();
        let doc = folder.document("doc.md").await.unwrap();
        assert_eq!(doc.path(), "doc.md");
        assert_eq!(doc.workspace(), folder.workspace());
        assert_eq!(doc.text(), Some("foo"));
    }
}