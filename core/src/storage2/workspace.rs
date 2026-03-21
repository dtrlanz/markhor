use std::{collections::HashMap, path::{Path, PathBuf}, sync::{Arc, Mutex}};

use serde::{Deserialize, Serialize};
use tokio::fs;

use crate::{embedding::Embedder, extension::F11y, storage2::{AccessStorageError, Document, Folder, WORKSPACE_CONFIG_DIR, WORKSPACE_METADATA_FILENAME}, vector_store::VectorStore};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Workspace {
    inner: Arc<WorkspaceInner>,
}

impl Workspace {
    /// Returns the root path of the workspace.
    pub fn path(&self) -> &Path {
        self.inner.absolute_path.as_path()
    }

    /// Returns the root folder of the workspace.
    pub fn root(&self) -> Folder {
        Folder::new(self.inner.absolute_path.clone(), self.clone())
    }

    /// Opens an existing directory as a workspace.
    ///
    /// Loads workspace metadata from the config subdirectory if it exists.
    pub async fn open(path: impl AsRef<Path>) -> Result<Self, AccessStorageError> {
        let absolute_path = fs::canonicalize(path.as_ref()).await
            .map_err(|e| if e.kind() == std::io::ErrorKind::NotFound {
                AccessStorageError::DirectoryNotFound(path.as_ref().to_path_buf())
            } else {
                AccessStorageError::Io(e)
        })?;
        let path_metadata = fs::metadata(&absolute_path).await?;
        if !path_metadata.is_dir() {
            return Err(AccessStorageError::NotADirectory(absolute_path));
        }
        if let Some(ws) = find_workspace_descendant(&absolute_path).await? {
            return Err(AccessStorageError::InWorkspace(ws));
        }

        let config_dir = absolute_path.join(WORKSPACE_CONFIG_DIR);
        let wd_md_path = config_dir.join(WORKSPACE_METADATA_FILENAME);
        let metadata: WorkspaceMetadata = match fs::read_to_string(wd_md_path).await {
            Ok(content) => serde_yaml_ng::from_str(&content)?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                WorkspaceMetadata::default()
            },
            Err(e) => {
                return Err(AccessStorageError::Io(e));
            }
        };

        Ok(Self {
            inner: Arc::new(WorkspaceInner {
                absolute_path,
                metadata,
                embeddings: Default::default(),
            }),
        })
    }

    /// Opens the folder with the specified name.
    /// 
    /// # Errors
    /// 
    /// Returns an error if the folder cannot be opened or does not exist.
    pub async fn folder(&self, name: impl AsRef<Path>) -> Result<Folder, AccessStorageError> {
        self.root().folder(name).await
    }

    /// Opens the document with the specified name.
    /// 
    /// # Errors
    /// 
    /// Returns an error if the document cannot be opened or does not exist.
    pub async fn document(&self, name: impl AsRef<Path>) -> Result<Document, AccessStorageError> {
        self.root().document(name).await
    }

    pub fn vector_store(&self, embedder: &F11y<dyn Embedder>) -> VectorStore {
        let mut embeddings = self.inner.embeddings.lock().unwrap();
        let id = embedder.metadata_id();
        Clone::clone(embeddings.entry(id).or_insert_with(|| VectorStore::new()))
    }
}

#[derive(Debug)]
struct WorkspaceInner {
    absolute_path: PathBuf,
    metadata: WorkspaceMetadata,
    embeddings: Mutex<HashMap<String, VectorStore>>,
}

impl PartialEq for WorkspaceInner {
    fn eq(&self, other: &Self) -> bool {
        self.absolute_path == other.absolute_path
    }
}

impl Eq for WorkspaceInner {}

/// Checks if a given path is a descendant of any workspace and returns its path if it is.
/// 
/// A workspace is considered to be a directory containing a workspace config directory. This 
/// function is used to check for nested workspaces, which are currently disallowed. 
/// 
/// Returns
/// 
/// - `Some` if the path is a descendant of a workspace
/// - `None` if the path is outside of any workspace
/// - `None` if the path is a workspace itself (i.e., points to the root directory)
pub(crate) async fn find_workspace_descendant(absolute_path: &Path) -> Result<Option<PathBuf>, AccessStorageError> {
    if let Some(parent) = absolute_path.parent() {
        for ancestor in parent.ancestors() {
            let config_dir = ancestor.join(WORKSPACE_CONFIG_DIR);
            match fs::metadata(&config_dir).await {
                Ok(metadata) => {
                    if metadata.is_dir() {
                        return Ok(Some(ancestor.to_path_buf()));
                    }
                },
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                    continue;
                },
                Err(e) => {
                    return Err(AccessStorageError::Io(e));
                }
            }
        }
    }
    Ok(None)
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct WorkspaceMetadata {
    foo: String,
}


#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[tokio::test]
    async fn workspace_open() {
        let dir = tempdir().unwrap();
        let ws_path = dir.path();

        // Open empty directory as workspace
        let ws = Workspace::open(&ws_path).await.unwrap();
        assert_eq!(ws.path(), fs::canonicalize(ws_path).await.unwrap());
        assert_eq!(ws.inner.metadata, WorkspaceMetadata::default());

        // Create config directory
        let config_dir = ws_path.join(WORKSPACE_CONFIG_DIR);
        fs::create_dir(&config_dir).await.unwrap();

        // Open workspace with empty config directory
        let ws = Workspace::open(&ws_path).await.unwrap();
        assert_eq!(ws.path(), fs::canonicalize(ws_path).await.unwrap());
        assert_eq!(ws.inner.metadata, WorkspaceMetadata::default());
        
        // Create metadata file
        let metadata_file = config_dir.join(WORKSPACE_METADATA_FILENAME);
        fs::write(&metadata_file, "foo: bar").await.unwrap();

        // Open workspace with metadata file
        let ws = Workspace::open(&ws_path).await.unwrap();
        assert_eq!(ws.path(), fs::canonicalize(ws_path).await.unwrap());
        assert_eq!(ws.inner.metadata, WorkspaceMetadata {
            foo: "bar".to_string(),
        });

        // Try opening workspace descendant as workspace
        let child_path = ws_path.join("child");
        fs::create_dir(&child_path).await.unwrap();
        let result = Workspace::open(&child_path).await;
        match result {
            Err(AccessStorageError::InWorkspace(path)) => {
                let ws_path = fs::canonicalize(ws_path).await.unwrap();
                assert_eq!(path, ws_path);
            },
            _ => {
                panic!("Wrong error type: {:?}", result);
            }
        }

        // Try opening non-existent workspace
        let dir = tempdir().unwrap();
        let dir_path = dir.path();
        let fake_path = dir_path.join("fake");
        let result = Workspace::open(&fake_path).await;
        assert!(result.is_err());
        match result {
            Err(AccessStorageError::DirectoryNotFound(path)) => {
                assert_eq!(path, fake_path);
            },
            _ => {
                panic!("Wrong error type: {:?}", result);
            }
        }

        // Try opening non-directory
        let fake_file = dir_path.join("fake.txt");
        fs::write(&fake_file, "foo").await.unwrap();
        let result = Workspace::open(&fake_file).await;
        assert!(result.is_err());
        match result {
            Err(AccessStorageError::NotADirectory(path)) => {
                assert_eq!(path, fs::canonicalize(fake_file).await.unwrap());
            },
            _ => {
                panic!("Wrong error type: {:?}", result);
            }
        }
    }
}
