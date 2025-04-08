use crate::event::define_event_listeners;
use crate::storage::{DocumentMoved, Error, Result, INTERNAL_DIR_NAME, WORKSPACE_CONFIG_FILENAME};
use crate::storage::folder::Folder; // Adjust path as needed
use std::path::{Path, PathBuf};
use std::sync::{Arc, Weak};
use serde::{Deserialize, Serialize};
use uuid::Uuid;
use tokio::fs;
use tracing::{debug, instrument, warn};

use super::{Document, Storage};


/// Represents the root workspace directory containing documents and folders,
/// along with internal configuration storage.
#[derive(Debug)]
pub struct Workspace {
    pub(crate) storage: Arc<Storage>,
    // Absolute path to the workspace root
    pub(crate) absolute_path: PathBuf,
    pub(crate) internal_dir: PathBuf,
    pub on: WorkspaceEvents,
}

// Proof of concept for now, will be expanded later
define_event_listeners!{ WorkspaceEvents {
    document_moved: DocumentMoved,
}}

impl Workspace {

    /// Returns the root path of the workspace.
    pub fn path(&self) -> &Path {
        &self.absolute_path
    }

    /// Returns the root folder of the workspace.
    pub async fn root(&self) -> Folder {
        Folder::new(self.absolute_path.clone(), self.arc().await)
    }

    /// Returns a folder instance for the specified path within the workspace.
    pub async fn folder(&self, path: &Path) -> Result<Folder> {
        // Compare absolute paths
        let abs_path = fs::canonicalize(path).await.map_err(Error::Io)?;
        if abs_path.starts_with(&self.absolute_path) {
            // Path is within the workspace
            // Check if the path is a directory
            if abs_path.is_dir() {
                Ok(Folder::new(abs_path, self.arc().await))
            } else {
                Err(Error::NotADirectory(abs_path))
            }
        } else {
            // Path is outside the workspace
            Err(Error::PathOutsideWorkspace(path.to_path_buf()))
        }
    }

    pub async fn document(&self, path: &Path) -> Result<Document> {

        // TODO: Be consistent about extension handling
        // Ensure the file has the correct extension
        let with_ext = path.with_extension("markhor");

        // Compare absolute paths
        let abs_path = fs::canonicalize(with_ext).await.map_err(Error::Io)?;
        if abs_path.starts_with(&self.absolute_path) {
            // Path is within the workspace
            // Remaining checks are carried out in Document::open()
            let doc = Document::open(abs_path, self.arc().await).await?;
            Ok(doc)
        } else {
            // Path is outside the workspace
            Err(Error::PathOutsideWorkspace(path.to_path_buf()))
        }
    }

    /// Retrieves an `Arc` reference to the workspace instance.
    /// This is useful because `Folder` and `Document` instances need a reference.
    async fn arc(&self) -> Arc<Workspace> {
        self.storage
            // Get always succeeds because this instance exists
            .get_or_insert(&self.absolute_path, async || unreachable!())
            // Will only yield if a workspace is currently being opened or created
            .await
            // We can just unwrap (see above: Get always succeeds)
            .unwrap()
    }

    /// Returns the path to the internal `.markhor` directory used for configuration and caching.
    pub(crate) fn internal_dir_path(&self) -> &Path {
        &self.internal_dir
    }

    /// Opens an existing workspace directory.
    ///
    /// Checks that the directory exists and contains the `.markhor` subdirectory.
    pub async fn open(storage: &Arc<Storage>, path: &Path) -> Result<Arc<Workspace>> {
        debug!("Attempting to open workspace");

        // Check if the path exists (necessary prior to canonicalization)
        let meta = fs::metadata(&path).await.map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                Error::DirectoryNotFound(path.to_path_buf())
            } else {
                Error::Io(e)
            }
        })?;

        if !meta.is_dir() {
            return Err(Error::NotADirectory(path.to_path_buf()));
        }

        // Canonicalization ensures consistent keys for the storage map
        let absolute_path = fs::canonicalize(path).await.map_err(Error::Io)?;
        debug!("Canonicalized workspace path: {}", absolute_path.display());

        // Get workspace instance from storage map
        let arc = storage.clone();
        let ws = storage.as_ref().get_or_insert(&*absolute_path, || 
            // Open workspace if not open already
            Workspace::open_internal(arc, absolute_path.clone())
        ).await;
        ws
    }

    /// Creates a new workspace at the specified path.
    ///
    /// - If the path does not exist, creates the directory and the `.markhor` subdirectory.
    /// - If the path exists and is an empty directory, creates the `.markhor` subdirectory.
    /// - Fails if the path exists and is a file, is a non-empty directory,
    ///   or already contains a `.markhor` file/directory.
    pub async fn create(storage: &Arc<Storage>, path: &Path) -> Result<Arc<Workspace>> {
        let arc = storage.clone();

        // Unlike `open`, we cannot canonicalize the path here because it should not exist yet.
        // Instead `create_internal` will error if the workspace already exists.
        let new_ws = Workspace::create_internal(arc, path.to_path_buf()).await?;

        // Now that it exists, we can canonicalize.
        let absolute_path = new_ws.absolute_path.clone();

        storage.get_or_insert(&*absolute_path, async || 
            Ok(new_ws)
        ).await
    }

    /// Opens an existing workspace directory.
    ///
    /// Checks that the directory exists and contains the `.markhor` subdirectory.
    #[instrument(skip(absolute_path), fields(absolute_path = %absolute_path.display()))]
    async fn open_internal(storage: Arc<Storage>, absolute_path: PathBuf) -> Result<Arc<Workspace>> {

        let internal_dir = absolute_path.join(INTERNAL_DIR_NAME);
        let internal_meta = fs::metadata(&internal_dir).await.map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                Error::NotAWorkspace(absolute_path.clone()) // .markhor dir missing means not a workspace
            } else {
                Error::Io(e)
            }
        })?;

        if !internal_meta.is_dir() {
            return Err(Error::NotAWorkspace(absolute_path)); // .markhor exists but isn't a directory
        }

        // Read and validate workspace metadata config file
        let metadata_path = internal_dir.join(WORKSPACE_CONFIG_FILENAME);
        debug!("Attempting to read workspace metadata from {}", metadata_path.display());
        let _metadata = read_workspace_metadata(&metadata_path).await?; // Read but don't store yet
        debug!("Successfully validated workspace metadata file.");        

        debug!("Workspace opened successfully");
        let mut arc = Arc::new(Workspace {
            storage,
            absolute_path, 
            internal_dir,
            on: WorkspaceEvents::new(),
        });
        Ok(arc)
    }

    /// Creates a new workspace at the specified path.
    ///
    /// - If the path does not exist, creates the directory and the `.markhor` subdirectory.
    /// - If the path exists and is an empty directory, creates the `.markhor` subdirectory.
    /// - Fails if the path exists and is a file, is a non-empty directory,
    ///   or already contains a `.markhor` file/directory.
    #[instrument(skip(path), fields(path = %path.display()))]
    async fn create_internal(storage: Arc<Storage>, path: PathBuf) -> Result<Arc<Workspace>> {
        debug!("Attempting to create workspace");

        let internal_dir = path.join(INTERNAL_DIR_NAME);

        match fs::metadata(&path).await {
            Ok(meta) => {
                // Path exists
                if !meta.is_dir() {
                    debug!("Workspace creation failed: path exists and is a file");
                    return Err(Error::PathIsFile(path));
                }

                // Path exists and is a directory, check if empty and if .markhor exists
                if fs::metadata(&internal_dir).await.is_ok() {
                    debug!("Workspace creation failed: '.markhor' directory already exists");
                    return Err(Error::WorkspaceCreationConflict(path));
                }

                // Check if directory is empty
                let mut read_dir = fs::read_dir(&path).await.map_err(Error::Io)?;
                if read_dir.next_entry().await.map_err(Error::Io)?.is_some() {
                    debug!("Workspace creation failed: directory is not empty");
                    return Err(Error::WorkspaceCreationConflict(path));
                }

                // Directory exists and is empty, proceed to create internal dir
                debug!("Path exists and is an empty directory. Creating internal directory.");
                fs::create_dir(&internal_dir).await.map_err(Error::Io)?;
                 
                // Create and write initial workspace metadata
                let metadata = WorkspaceMetadata::new();
                let metadata_path = internal_dir.join(WORKSPACE_CONFIG_FILENAME);
                write_workspace_metadata(&metadata_path, &metadata).await?;
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                // Path does not exist, create both dirs
                debug!("Path does not exist. Creating workspace directory and internal directory.");
                fs::create_dir_all(&path).await.map_err(Error::Io)?; // Use create_dir_all for parent dirs
                fs::create_dir(&internal_dir).await.map_err(Error::Io)?;
                
                // Create and write initial workspace metadata
                let metadata = WorkspaceMetadata::new();
                let metadata_path = internal_dir.join(WORKSPACE_CONFIG_FILENAME);
                write_workspace_metadata(&metadata_path, &metadata).await?;                
            }
            Err(e) => {
                // Other FS error accessing path
                return Err(Error::Io(e));
            }
        }
        debug!("Workspace created successfully");

        let absolute_path = fs::canonicalize(path).await.map_err(Error::Io)?;
        debug!("Canonicalized workspace path: {}", absolute_path.display());

        Ok(Arc::new(Workspace { 
            storage,
            absolute_path,
            internal_dir,
            on: WorkspaceEvents::new(),
        }))
    }
}


/// Represents metadata associated with a Workspace.
/// Stored in `.markhor/config.json` within the workspace directory.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[serde(crate = "serde")] // Add this if facing issues with derive macro resolution
pub(crate) struct WorkspaceMetadata {
    /// A unique identifier for the workspace instance.
    id: Uuid,
    /// A version number for the metadata format, useful for future migrations.
    version: u32,
    // Add other simple fields here later if needed, e.g.:
    // name: Option<String>,
    // created_at: u64, // Consider chrono if more complex time handling needed
}

impl WorkspaceMetadata {
    /// Creates a new metadata instance with default values.
    pub(crate) fn new() -> Self {
        WorkspaceMetadata {
            id: Uuid::new_v4(),
            version: 1, // Start at version 1
        }
    }
}

/// Helper to read and deserialize workspace metadata.
pub(crate) async fn read_workspace_metadata(path: &Path) -> Result<WorkspaceMetadata> {
    let content = fs::read(path).await.map_err(|e| {
        warn!("Failed to read workspace config file '{}': {}", path.display(), e);
        Error::InvalidWorkspaceConfig(path.to_path_buf()) // Config missing or unreadable
    })?;

    serde_json::from_slice(&content).map_err(|e| {
        warn!("Failed to parse workspace config file '{}': {}", path.display(), e);
        Error::InvalidWorkspaceConfig(path.to_path_buf()) // Config malformed
    })
}

/// Helper to serialize and write workspace metadata.
pub(crate) async fn write_workspace_metadata(path: &Path, metadata: &WorkspaceMetadata) -> Result<()> {
    let content = serde_json::to_string_pretty(metadata)
        .map_err(Error::Metadata)?; // Handle serialization error cleanly
    fs::write(path, content).await.map_err(Error::Io)?;
    debug!("Workspace metadata written successfully to {}", path.display());
    Ok(())
}

// --- Tests ---
#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    // Helper to create a dummy file/dir
    async fn create_dummy(path: &Path, is_dir: bool) {
        if is_dir {
            fs::create_dir_all(path).await.expect("Failed to create dummy dir");
        } else {
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent).await.expect("Failed to create parent dir");
            }
            fs::write(path, "").await.expect("Failed to create dummy file");
        }
    }

    #[tokio::test]
    async fn test_workspace_create_new() {
        let dir = tempdir().unwrap();
        let ws_path = dir.path().join("new_ws");
        let storage = Arc::new(Storage::new());

        let ws = Workspace::create(&storage, &*ws_path).await.unwrap();
        assert!(ws_path.exists());
        assert!(ws_path.is_dir());
        assert!(ws.internal_dir_path().exists());
        assert!(ws.internal_dir_path().is_dir());
        assert_eq!(ws.internal_dir_path().file_name().unwrap(), INTERNAL_DIR_NAME);
        // check for config.json
        let config_path = ws.internal_dir_path().join(WORKSPACE_CONFIG_FILENAME);
        assert!(config_path.exists(), "Workspace config file should exist");
        assert!(config_path.is_file(), "Workspace config should be a file");
        let content = fs::read_to_string(&config_path).await.unwrap();
        let meta: serde_json::Value = serde_json::from_str(&content).expect("Config file should be valid JSON");        
        // todo: update when this is replaced with actual config
        assert!(meta.get("id").is_some()); // Check for UUID field
    }

     #[tokio::test]
    async fn test_workspace_create_in_empty_dir() {
        let dir = tempdir().unwrap();
        let ws_path = dir.path().join("empty_dir_ws");
        let storage = Arc::new(Storage::new());
        create_dummy(&ws_path, true).await; // Create empty dir first

        println!("Creating workspace in empty dir: {}", ws_path.display());
        let ws = Workspace::create(&storage, &*ws_path).await.unwrap();
        assert!(ws_path.exists());
        assert!(ws.internal_dir_path().exists());
    }

    #[tokio::test]
    async fn test_workspace_create_fails_if_file() {
        let dir = tempdir().unwrap();
        let ws_path = dir.path().join("file_path_ws");
        let storage = Arc::new(Storage::new());
        create_dummy(&ws_path, false).await; // Create a file

        let result = Workspace::create(&storage, &*ws_path).await;
        assert!(matches!(result, Err(Error::PathIsFile(_))));
    }

    #[tokio::test]
    async fn test_workspace_create_fails_if_non_empty() {
        let dir = tempdir().unwrap();
        let ws_path = dir.path().join("non_empty_ws");
        let storage = Arc::new(Storage::new());
        create_dummy(&ws_path.join("some_file.txt"), false).await; // Create a file inside

        let result = Workspace::create(&storage, &*ws_path).await;
        assert!(matches!(result, Err(Error::WorkspaceCreationConflict(_))));
    }

     #[tokio::test]
    async fn test_workspace_create_fails_if_internal_dir_exists() {
        let dir = tempdir().unwrap();
        let ws_path = dir.path().join("already_ws");
        let storage = Arc::new(Storage::new());
        create_dummy(&ws_path.join(INTERNAL_DIR_NAME), true).await; // Create internal dir

        let result = Workspace::create(&storage, &*ws_path).await;
        assert!(matches!(result, Err(Error::WorkspaceCreationConflict(_))));
    }

    #[tokio::test]
    async fn test_workspace_open_ok() {
        let dir = tempdir().unwrap();
        let ws_path = dir.path().join("existing_ws");
        let storage = Arc::new(Storage::new());

        // Create a valid workspace structure first
        Workspace::create(&storage, &*ws_path).await.unwrap();

        // Now open it
        let ws = Workspace::open(&storage, &*ws_path).await.unwrap();
        assert_eq!(ws.path(), fs::canonicalize(ws_path.as_path()).await.unwrap());
        assert!(ws.internal_dir_path().exists());
    }

     #[tokio::test]
    async fn test_workspace_open_fails_if_not_dir() {
        let dir = tempdir().unwrap();
        let ws_path = dir.path().join("not_a_dir_ws");
        let storage = Arc::new(Storage::new());
        create_dummy(&ws_path, false).await; // Create a file

        let result = Workspace::open(&storage, &*ws_path).await;
        assert!(matches!(result, Err(Error::NotADirectory(_))));
    }

     #[tokio::test]
    async fn test_workspace_open_fails_if_no_internal_dir() {
        let dir = tempdir().unwrap();
        let ws_path = dir.path().join("no_internal_dir_ws");
        let storage = Arc::new(Storage::new());
        create_dummy(&ws_path, true).await; // Create dir, but not internal one

        let result = Workspace::open(&storage, &*ws_path).await;
        assert!(matches!(result, Err(Error::NotAWorkspace(_))));
    }

     #[tokio::test]
    async fn test_workspace_open_fails_if_internal_is_file() {
        let dir = tempdir().unwrap();
        let ws_path = dir.path().join("internal_is_file_ws");
        let storage = Arc::new(Storage::new());
        create_dummy(&ws_path, true).await;
        create_dummy(&ws_path.join(INTERNAL_DIR_NAME), false).await; // Create internal as file

        let result = Workspace::open(&storage, &*ws_path).await;
        assert!(matches!(result, Err(Error::NotAWorkspace(_))));
    }

    #[tokio::test]
    async fn test_workspace_open_fails_if_config_missing() {
        let dir = tempdir().unwrap();
        let ws_path = dir.path().join("config_missing_ws");
        let storage = Arc::new(Storage::new());
    
        // Create workspace structure manually *without* config.json
        create_dummy(&ws_path, true).await;
        create_dummy(&ws_path.join(INTERNAL_DIR_NAME), true).await;
    
        let open_err = Workspace::open(&storage, &*ws_path).await;
        assert!(matches!(open_err, Err(Error::InvalidWorkspaceConfig(_))), "Opening workspace without config should fail");
    }
    
    #[tokio::test]
    async fn test_workspace_open_fails_if_config_malformed() {
        let dir = tempdir().unwrap();
        let ws_path = dir.path().join("config_malformed_ws");
        let storage = Arc::new(Storage::new());
        let internal_dir_path = ws_path.join(INTERNAL_DIR_NAME);
        let config_path = internal_dir_path.join(WORKSPACE_CONFIG_FILENAME);
    
        // Create workspace structure with invalid config.json
        create_dummy(&ws_path, true).await;
        create_dummy(&internal_dir_path, true).await;
        fs::write(&config_path, "{ not json }").await.unwrap(); // Write malformed JSON
    
        let open_err = Workspace::open(&storage, &*ws_path).await;
        assert!(matches!(open_err, Err(Error::InvalidWorkspaceConfig(_))), "Opening workspace with malformed config should fail");
    }    

    // #[tokio::test]
    // async fn test_list_items_in_workspace_and_folder() {
    //     let dir = tempdir().unwrap();
    //     let ws_path = dir.path().join("list_ws");
    //     let storage = Arc::new(Storage::new());

    //     // Setup workspace
    //     let ws = Workspace::create(&storage, &*ws_path).await.unwrap();

    //     // Items in root
    //     let doc1_path = ws_path.join("root_doc.markhor");
    //     let folder1_path = ws_path.join("folder1");
    //     create_dummy(&ws_path.join("ignored.txt"), false).await;
    //     let _doc1 = Document::create(doc1_path.clone(), ws.path().to_path_buf()).await.unwrap();
    //     create_dummy(&folder1_path, true).await;

    //     // Items in folder1
    //     let doc2_path = folder1_path.join("nested_doc.markhor");
    //     let folder2_path = folder1_path.join("folder2");
    //     let _doc2 = Document::create(doc2_path.clone(), ws.path().to_path_buf()).await.unwrap();
    //     create_dummy(&folder2_path, true).await;
    //     create_dummy(&folder1_path.join("another.file"), false).await;


    //     // --- Test Workspace Listing ---
    //     let root_docs = ws.root().list_documents().await.unwrap();
    //     assert_eq!(root_docs.len(), 1);
    //     assert_eq!(root_docs[0].path(), PathBuf::from("root_doc.markhor"));

    //     let root_folders = ws.root().list_folders().await.unwrap();
    //     println!("Root folders: {:?}", root_folders.iter().map(|f| f.path()).collect::<Vec<_>>());
    //     assert_eq!(root_folders.len(), 1);
    //     assert_eq!(root_folders[0].path(), PathBuf::from("folder1"));
    //     assert_ne!(root_folders[0].path().file_name().unwrap(), INTERNAL_DIR_NAME); // Ensure .markhor excluded


    //     // --- Test Folder Listing ---
    //     let folder1 = root_folders.into_iter().next().unwrap();
    //     let folder1_docs = folder1.list_documents().await.unwrap();
    //     assert_eq!(folder1_docs.len(), 1);
    //     assert_eq!(folder1_docs[0].path(), PathBuf::from("folder1/nested_doc.markhor"));

    //     let folder1_folders = folder1.list_folders().await.unwrap();
    //     assert_eq!(folder1_folders.len(), 1);
    //     assert_eq!(folder1_folders[0].path(), PathBuf::from("folder1/folder2"));

    //      // --- Test Empty Folder Listing ---
    //      let folder2 = folder1_folders.into_iter().next().unwrap();
    //      let folder2_docs = folder2.list_documents().await.unwrap();
    //      assert!(folder2_docs.is_empty());
    //      let folder2_folders = folder2.list_folders().await.unwrap();
    //      assert!(folder2_folders.is_empty());
    // }

    // #[tokio::test]
    // async fn test_list_documents_skips_invalid() {
    //     let dir = tempdir().unwrap();
    //     let ws_path = dir.path().join("invalid_doc_ws");
    //     let storage = Arc::new(Storage::new());
    //     let ws = Workspace::create(&storage, &*ws_path).await.unwrap();

    //     let valid_doc_path = ws_path.join("valid.markhor");
    //     let invalid_doc_path = ws_path.join("invalid.markhor"); // Will be empty, causing JSON error
    //     let not_json_path = ws_path.join("not_json.markhor");

    //     let _valid_doc = Document::create(valid_doc_path.clone(), ws.path().to_path_buf()).await.unwrap();
    //     create_dummy(&invalid_doc_path, false).await; // Empty file
    //     fs::write(&not_json_path, "this is not json").await.unwrap();

    //     // Set up tracing subscriber to capture warnings (optional but good practice)
    //     // let subscriber = tracing_subscriber::fmt().with_max_level(tracing::Level::WARN).finish();
    //     // let _guard = tracing::subscriber::set_default(subscriber);

    //     let docs = ws.root().list_documents().await.unwrap();
    //     assert_eq!(docs.len(), 1); // Only the valid document should be listed
    //     assert_eq!(docs[0].path(), PathBuf::from("valid.markhor"));
    //     // Check logs manually or via subscriber for warnings about invalid.markhor & not_json.markhor
    // }
}
