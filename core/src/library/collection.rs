use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

use serde::{Deserialize, Deserializer, Serialize};
use time::OffsetDateTime;
use tokio::fs;

use crate::{
    embedding::EmbeddingModel,
    extension::Comp,
    library::{
        AccessLibraryError, COLLECTION_CONFIG_DIR, COLLECTION_METADATA_FILENAME, Document, Folder,
    },
    permissions::{ON_DEVICE, Permission, all_permissions},
    vector_store::VectorStore,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Collection {
    inner: Arc<CollectionInner>,
}

impl Collection {
    pub async fn create(path: impl AsRef<Path>) -> Result<Self, AccessLibraryError> {
        // TODO: consider creating collection config directory and files directly, or add options
        // to control this behavior
        // TODO: add unit tests
        let absolute_path = fs::canonicalize(path.as_ref()).await.map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                AccessLibraryError::DirectoryNotFound(path.as_ref().to_path_buf())
            } else {
                AccessLibraryError::Io(e)
            }
        })?;
        if let Some(collection) = find_collection_descendant(&absolute_path).await? {
            return Err(AccessLibraryError::InCollection(collection));
        }
        fs::create_dir_all(&absolute_path).await?;
        Ok(Self {
            inner: Arc::new(CollectionInner {
                absolute_path,
                metadata: CollectionMetadata::default(),
                embeddings: Default::default(),
            }),
        })
    }

    /// Returns the root path of the collection.
    pub fn path(&self) -> &Path {
        self.inner.absolute_path.as_path()
    }

    /// Returns the root folder of the collection.
    pub fn root(&self) -> Folder {
        Folder::new(self.inner.absolute_path.clone(), self.clone())
    }

    /// Returns the permissions given to documents that do not specify their own.
    ///
    /// This is the strictest level, so documents are restricted unless their owner says otherwise.
    /// It is a placeholder until permissions can be configured per collection.
    pub fn default_permissions(&self) -> &[Permission] {
        &[ON_DEVICE]
    }

    /// Returns the permissions that may be used in this collection.
    ///
    /// These are the built-in permissions for now. They will become configurable per collection.
    pub fn available_permissions(&self) -> Vec<Permission> {
        all_permissions()
    }

    /// Opens an existing directory as a collection.
    ///
    /// Loads collection metadata from the config subdirectory if it exists.
    pub async fn open(path: impl AsRef<Path>) -> Result<Self, AccessLibraryError> {
        let absolute_path = fs::canonicalize(path.as_ref()).await.map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                AccessLibraryError::DirectoryNotFound(path.as_ref().to_path_buf())
            } else {
                AccessLibraryError::Io(e)
            }
        })?;
        let path_metadata = fs::metadata(&absolute_path).await?;
        if !path_metadata.is_dir() {
            return Err(AccessLibraryError::NotADirectory(absolute_path));
        }
        if let Some(collection) = find_collection_descendant(&absolute_path).await? {
            return Err(AccessLibraryError::InCollection(collection));
        }

        let config_dir = absolute_path.join(COLLECTION_CONFIG_DIR);
        let coll_md_path = config_dir.join(COLLECTION_METADATA_FILENAME);
        let metadata: CollectionMetadata = match fs::read_to_string(coll_md_path).await {
            Ok(content) => serde_yaml_ng::from_str(&content)?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => CollectionMetadata {
                created: dir_created_time(&path_metadata),
                description: None,
            },
            Err(e) => {
                return Err(AccessLibraryError::Io(e));
            }
        };

        Ok(Self {
            inner: Arc::new(CollectionInner {
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
    pub async fn folder(&self, name: impl AsRef<Path>) -> Result<Folder, AccessLibraryError> {
        self.root().folder(name).await
    }

    /// Opens the document with the specified name.
    ///
    /// # Errors
    ///
    /// Returns an error if the document cannot be opened or does not exist.
    pub async fn document(&self, name: impl AsRef<Path>) -> Result<Document, AccessLibraryError> {
        self.root().document(name).await
    }

    pub fn vector_store(&self, embedder: &Comp<dyn EmbeddingModel>) -> VectorStore {
        let mut embeddings = self.inner.embeddings.lock().unwrap();
        let id = embedder.metadata_id();
        Clone::clone(embeddings.entry(id).or_insert_with(|| VectorStore::new()))
    }
}

#[derive(Debug)]
struct CollectionInner {
    absolute_path: PathBuf,
    metadata: CollectionMetadata,
    embeddings: Mutex<HashMap<String, VectorStore>>,
}

impl PartialEq for CollectionInner {
    fn eq(&self, other: &Self) -> bool {
        self.absolute_path == other.absolute_path
    }
}

impl Eq for CollectionInner {}

/// Checks if a given path is a descendant of any collection and returns its path if it is.
///
/// A collection is considered to be a directory containing a collection config directory. This
/// function is used to check for nested collections, which are currently disallowed.
///
/// Returns
///
/// - `Some` if the path is a descendant of a collection
/// - `None` if the path is outside of any collection
/// - `None` if the path is a collection itself (i.e., points to the root directory)
pub(crate) async fn find_collection_descendant(
    absolute_path: &Path,
) -> Result<Option<PathBuf>, AccessLibraryError> {
    if let Some(parent) = absolute_path.parent() {
        for ancestor in parent.ancestors() {
            let config_dir = ancestor.join(COLLECTION_CONFIG_DIR);
            match fs::metadata(&config_dir).await {
                Ok(metadata) => {
                    if metadata.is_dir() {
                        return Ok(Some(ancestor.to_path_buf()));
                    }
                }
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                    continue;
                }
                Err(e) => {
                    return Err(AccessLibraryError::Io(e));
                }
            }
        }
    }
    Ok(None)
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CollectionMetadata {
    /// When the collection was created.
    #[serde(with = "time::serde::rfc3339")]
    pub created: OffsetDateTime,
    /// Optional description. If present, it is never empty.
    #[serde(
        default,
        deserialize_with = "deserialize_non_empty",
        skip_serializing_if = "Option::is_none"
    )]
    pub description: Option<String>,
}

impl Default for CollectionMetadata {
    fn default() -> Self {
        Self {
            created: OffsetDateTime::now_utc(),
            description: None,
        }
    }
}

/// Returns the directory's creation time, or the current time if the platform or filesystem
/// does not record it.
fn dir_created_time(metadata: &std::fs::Metadata) -> OffsetDateTime {
    metadata
        .created()
        .map(OffsetDateTime::from)
        .unwrap_or_else(|_| OffsetDateTime::now_utc())
}

fn deserialize_non_empty<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<String>, D::Error> {
    Ok(Option::<String>::deserialize(deserializer)?.filter(|s| !s.is_empty()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::library::fs_test_utils::{TempTree, fs_tree};
    use time::macros::datetime;

    #[tokio::test]
    async fn collection_open() {
        let mut dir = TempTree::new(fs_tree! {
            "file.txt" => "hello world",
            "child" => {},
        })
        .await
        .unwrap();

        // Open empty directory as collection
        let collection = Collection::open(&dir).await.unwrap();
        assert_eq!(collection.path(), fs::canonicalize(&dir).await.unwrap());
        assert_eq!(collection.inner.metadata.description, None);

        // Without a metadata file, `created` is stable across opens
        let reopened = Collection::open(&dir).await.unwrap();
        assert_eq!(
            collection.inner.metadata.created,
            reopened.inner.metadata.created
        );

        // Create config directory
        dir.add(fs_tree! {
            COLLECTION_CONFIG_DIR => {},
        })
        .await
        .unwrap();

        // Open collection with empty config directory
        let collection = Collection::open(&dir).await.unwrap();
        assert_eq!(collection.path(), fs::canonicalize(&dir).await.unwrap());
        assert_eq!(collection.inner.metadata.description, None);

        // Create metadata file
        dir.add(fs_tree! {
            COLLECTION_CONFIG_DIR => {
                COLLECTION_METADATA_FILENAME => "created: 2024-01-02T03:04:05Z\ndescription: bar",
            },
        })
        .await
        .unwrap();

        // Open collection with metadata file
        let collection = Collection::open(&dir).await.unwrap();
        assert_eq!(collection.path(), fs::canonicalize(&dir).await.unwrap());
        assert_eq!(
            collection.inner.metadata,
            CollectionMetadata {
                created: datetime!(2024-01-02 03:04:05 UTC),
                description: Some("bar".to_string()),
            }
        );

        // Empty description is treated as absent
        dir.add(fs_tree! {
            COLLECTION_CONFIG_DIR => {
                COLLECTION_METADATA_FILENAME => "created: 2024-01-02T03:04:05Z\ndescription: ''",
            },
        })
        .await
        .unwrap();
        let collection = Collection::open(&dir).await.unwrap();
        assert_eq!(collection.inner.metadata.description, None);

        // Try opening collection descendant as collection
        let result = Collection::open(&dir.join("child")).await;
        match result {
            Err(AccessLibraryError::InCollection(path)) => {
                let collection_path = fs::canonicalize(&dir).await.unwrap();
                assert_eq!(path, collection_path);
            }
            _ => {
                panic!("Wrong error type: {:?}", result);
            }
        }

        // Try opening non-existent collection
        let result = Collection::open(&dir.join("fake")).await;
        assert!(result.is_err());
        match result {
            Err(AccessLibraryError::DirectoryNotFound(path)) => {
                assert_eq!(path, dir.join("fake"));
            }
            _ => {
                panic!("Wrong error type: {:?}", result);
            }
        }

        // Try opening non-directory
        let result = Collection::open(&dir.join("file.txt")).await;
        assert!(result.is_err());
        match result {
            Err(AccessLibraryError::NotADirectory(path)) => {
                assert_eq!(path, fs::canonicalize(dir.join("file.txt")).await.unwrap());
            }
            _ => {
                panic!("Wrong error type: {:?}", result);
            }
        }
    }

    #[tokio::test]
    async fn collection_document() {
        let dir = TempTree::new(fs_tree! {
            "doc.md" => "foo",
            "child" => {
                "nested.md" => "nested",
            },
        })
        .await
        .unwrap();

        let collection = Collection::open(&dir).await.unwrap();
        let doc = collection.document("doc.md").await.unwrap();
        assert_eq!(doc.path(), "doc.md");
        assert_eq!(doc.collection().path(), collection.path());
        assert_eq!(
            doc.text().await.unwrap().export(&mut None).as_deref(),
            Some("foo")
        );

        let child = collection.folder("child").await.unwrap();
        let doc = child.document("nested.md").await.unwrap();
        assert_eq!(doc.path(), "child/nested.md");
        assert_eq!(doc.collection().path(), collection.path());
        assert_eq!(
            doc.text().await.unwrap().export(&mut None).as_deref(),
            Some("nested")
        );
    }
}
