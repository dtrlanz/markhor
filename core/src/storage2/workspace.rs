use std::{f32::consts::E, path::{Path, PathBuf}, sync::Arc};

use serde::{Deserialize, Serialize};
use tokio::fs;

use crate::storage2::{AccessStorageError, Folder, WORKSPACE_CONFIG_DIR, WORKSPACE_METADATA_FILENAME};

#[derive(Debug, Clone)]
pub struct Workspace {
    inner: Arc<WorkspaceInner>,
}

#[derive(Debug)]
struct WorkspaceInner {
    absolute_path: PathBuf,
    metadata: WorkspaceMetadata,
}

impl Workspace {
    /// Returns the root path of the workspace.
    pub fn path(&self) -> &Path {
        self.inner.absolute_path.as_path()
    }

    /// Returns the root folder of the workspace.
    pub async fn root(&self) -> Folder {
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
            }),
        })
    }
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
    async fn open_workspace() {
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

        // Try opening non-existent workspace
        let fake_path = ws_path.join("fake");
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
        let fake_file = ws_path.join("fake.txt");
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
