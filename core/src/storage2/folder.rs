use std::{backtrace, path::{Path, PathBuf}, sync::Arc};

use tokio::fs::{self, ReadDir};
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

    /// Returns a stream over the entries in this folder. Each entry is either a Folder or a 
    /// Document.
    pub async fn read(&self) -> Result<Read, std::io::Error> {
        let read_dir = fs::read_dir(&self.absolute_path).await?;
        Ok(Read {
            workspace: self.workspace.clone(),
            read_dir,
        })
    }

    pub async fn read_recursive(&self) -> Result<ReadRecursive, std::io::Error> {
        let read = self.read().await?;
        Ok(ReadRecursive {
            vec: vec![read],
        })
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

#[derive(Debug)]
pub struct Read {
    workspace: Workspace,
    read_dir: ReadDir,
}

impl Read {
    pub async fn next_entry(&mut self) -> Result<Option<ReadEntry>, AccessStorageError> {
        loop {
            match self.read_dir.next_entry().await? {
                Some(entry) => {
                    let path = entry.path();
                    let metadata = entry.metadata().await?;
                    let entry = if metadata.is_dir() {
                        let folder = open_folder(path, self.workspace.clone()).await?;
                        ReadEntry::Folder(folder)
                    } else if metadata.is_file() {
                        // Include source files only, ignore metadata files
                        if path.extension().and_then(|ext| ext.to_str()) == Some(crate::storage2::METADATA_EXTENSION) {
                            continue;
                        }
                        let document = open_document(&path, self.workspace.clone()).await?;
                        ReadEntry::Document(document)
                    } else {
                        // Ignore other types of entries (symlinks, etc.)
                        continue;
                    };
                    return Ok(Some(entry));
                },
                None => return Ok(None),
            }
        }
    }
}

#[derive(Debug, Clone)]
pub enum ReadEntry {
    Folder(Folder),
    Document(Document),
}

#[derive(Debug)]
pub struct ReadRecursive {
    vec: Vec<Read>,
}

impl ReadRecursive {
    pub async fn next_entry(&mut self) -> Result<Option<Document>, AccessStorageError> {
        while let Some(read) = self.vec.last_mut() {
            match read.next_entry().await? {
                Some(entry) => {
                    match entry {
                        ReadEntry::Folder(folder) => {
                            let read = folder.read().await?;
                            self.vec.push(read);
                        },
                        ReadEntry::Document(document) => {
                            return Ok(Some(document));
                        },
                    }
                },
                None => {
                    self.vec.pop();
                },
            }
        }
        Ok(None)
    }
}


#[cfg(test)]
mod tests {
    use crate::storage2::fs_test_utils::{TempTree, fs_tree};
    use crate::storage2::WORKSPACE_CONFIG_DIR;

    use super::*;

    #[tokio::test]
    async fn folder_open() {
        let mut dir = TempTree::new(fs_tree! {
            "child" => {
                "doc.md" => "foo",
            },
        }).await.unwrap();

        // Open path in implicit workspace
        let folder = Folder::open(&dir).await.unwrap();
        let ws_path = folder.workspace().path();
        assert_eq!(folder.path(), "");
        assert_eq!(
            fs::canonicalize(ws_path).await.unwrap(), 
            fs::canonicalize(&dir).await.unwrap()
        );

        // Open path as root of explict workspace
        dir.add(fs_tree! {
            WORKSPACE_CONFIG_DIR => {},
        }).await.unwrap();
        let folder = Folder::open(&dir).await.unwrap();
        assert_eq!(folder.path(), "");
        assert_eq!(
            fs::canonicalize(ws_path).await.unwrap(), 
            fs::canonicalize(&dir).await.unwrap()
        );

        // Open path as child of explicit workspace
        let child_path = dir.join("child");
        let folder = Folder::open(&child_path).await.unwrap();
        assert_eq!(folder.path(), "child");
        let ws_path = folder.workspace().path();
        assert_eq!(
            fs::canonicalize(ws_path).await.unwrap(), 
            fs::canonicalize(&dir).await.unwrap()
        );
    }

    #[tokio::test]
    async fn folder_document() {
        // Document without metadata
        let dir = TempTree::new(fs_tree! {
            "doc.md" => "foo",
        }).await.unwrap();

        let folder = Folder::open(&dir).await.unwrap();
        let doc = folder.document("doc.md").await.unwrap();
        assert_eq!(doc.path(), "doc.md");
        assert_eq!(doc.workspace(), folder.workspace());
        assert_eq!(doc.text().as_deref(), Some("foo"));
    }
}