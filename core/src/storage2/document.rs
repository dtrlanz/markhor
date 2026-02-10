use serde::{Deserialize, Serialize, de::DeserializeOwned};
use sha2::{Digest, Sha256, digest::{OutputSizeUser, generic_array::GenericArray}};
use tokio::io::{AsyncWriteExt};
use uuid::Uuid;
use tokio::fs::{self, OpenOptions};
use tracing::{debug, info, instrument, warn};
use std::{borrow::Borrow, collections::HashMap, ffi::OsStr, path::{Path, PathBuf}};

use crate::{chunking::{Chunker, ChunkerError}, embedding::Embedding, extension::F11y, markdown::{ToMarkdown, WITH_MILESTONES}, storage2::{ATTACHMENTS_DIR, AccessStorageError, METADATA_EXTENSION, Workspace}};



pub struct Document {
    /// Absolute path to the source file
    pub(crate) absolute_path: PathBuf,

    /// Workspace owning this document
    workspace: Workspace,
    metadata: DocumentMetadata,
    metadata_location: MetadataLocation,
    text_parts: Vec<(String, String)>,
    text_hash: Option<TextHash>,
    cache: DocCache,
    chunk_cache: HashMap<String, HashMap<String, Vec<ChunkCache>>>,
}

impl Document {
    /// Returns the relative path to the document within its workspace.
    pub fn path(&self) -> &Path {
        self.absolute_path.strip_prefix(&self.workspace.path()).unwrap()
    }

    pub fn name(&self) -> &str {
        self.absolute_path.file_stem()
            .and_then(OsStr::to_str).unwrap()
    }

    /// Returns the workspace owning this document.
    pub fn workspace(&self) -> &Workspace {
        &self.workspace
    }

    pub fn id(&self) -> &Uuid {
        &self.metadata.id
    }

    pub fn text(&self) -> Option<String> {
        if self.text_parts.is_empty() {
            None
        } else if self.text_parts.len() == 1 && self.text_parts[0].0.is_empty() {
            Some(self.text_parts[0].1.clone())
        } else {
            let mut text = String::new();
            for (n, (id, part)) in self.text_parts.iter().enumerate() {
                text.push_str(&format!("---\npart_idx: {}\n", n));
                if !id.is_empty() {
                    text.push_str(&format!("id: {}\n", id));
                }
                text.push_str("---\n");
                text.push_str(part);
                text.push_str("\n");
            }
            Some(text)
        }
    }

    pub fn text_parts(&self) -> impl Iterator<Item = (&str, &str)> {
        self.text_parts.iter().map(|(id, text)| (id.as_str(), text.as_str()))
    }

    pub fn text_parts_mut(&mut self) -> impl Iterator<Item = (&str, &mut String)> {
        // Content may change, invalidate text hash
        self.text_hash = None;
        self.text_parts.iter_mut().map(|(id, text)| (id.as_str(), text))
    }

    fn text_hash(&mut self) -> &TextHash {
        if self.text_hash.is_none() {
            let text_iter = self.text_parts.iter()
                .map(|(id, text)| [&**id, &**text].into_iter())
                .flatten();
            self.text_hash = Some(TextHash::from_iter(text_iter));
        }
        self.text_hash.as_ref().unwrap()
    }

    pub(crate) fn extension_cache(&self, extension: &str) -> Option<&ExtensionCache> {
        self.cache.extensions.get(extension)
    }

    pub fn chunks(&mut self, chunker: F11y<dyn Chunker>) -> Result<Chunks<'_>, ChunkerError> {
        let chunker_id = chunker.metadata_id();
        let text_hash = *self.text_hash();
        let chunk_cache_is_valid = self.cache.extensions
            .get(chunker.extension().uri())
            .map(|ext_cache| ext_cache.hash == Some(text_hash))
            .unwrap_or(false);

        if !chunk_cache_is_valid {
            // Re-chunk the document
            for (id, text) in self.text_parts.iter() {
                let chunk_caches = self.chunk_cache
                    .entry(chunker_id.clone())
                    .or_default()
                    .entry(id.clone())
                    .or_default();
                let chunks = chunker.chunk(&text)?;
                chunk_caches.truncate(chunks.len());
                for (idx, chunk) in chunks.into_iter().enumerate() {
                    let chunk_text = &text[chunk.text_range.clone()];
                    let hash = TextHash::from(chunk_text);
                    if idx < chunk_caches.len() {
                        // Chunk has been cached before; check equality
                        if chunk_caches[idx].chunk == chunk && chunk_caches[idx].hash == hash {
                            // Cache is valid, skip
                            continue;
                        }
                        // Cache is outdated, update & drop invalid embeddings
                        chunk_caches[idx] = ChunkCache {
                            chunk,
                            hash,
                            embeddings: HashMap::new(),
                        };
                    } else {
                        // New chunk, add to cache
                        chunk_caches.push(ChunkCache {
                            chunk: chunk,
                            hash,
                            embeddings: HashMap::new(),
                        });
                    }
                }
            }
        }

        Ok(Chunks {
            text_parts: self.text_parts.iter().collect(),
            data: self.chunk_cache.get(&chunker_id).unwrap(),
            chunk_idx: 0,
        })
    }

    // pub(crate) fn chunk_cache(&self, chunker_id: String, text_id: String, idx: usize) -> Option<&ChunkCache> {
    //     self.chunk_cache.get(&chunker_id)
    //         .and_then(|by_text| by_text.get(&text_id))
    //         .and_then(|chunks| chunks.get(idx))
    // }

    // pub(crate) async fn chunk_cache_entry(&mut self, chunker_id: String, text_id: String, idx: usize) -> Option<&ChunkCache> {
    //     let read_cache = self.read_cache_file(format!(".chunks_{}.yaml", chunker_id).as_str());
    //     let mut by_text = match self.chunk_cache.entry(chunker_id) {
    //         Entry::Occupied(occupied) => occupied.get_mut(),
    //         Entry::Vacant(vacant) => {
    //             let mut by_text = read_cache.await.ok().flatten().unwrap_or_default();
    //             vacant.insert(by_text)
    //         },
    //     };
    //     let mut chunks = by_text.entry(text_id).or_default();
    //     // TODO: figure out what we actually want to do here
    //     while chunks.len() <= idx {
    //         chunks.push(ChunkCache {
    //             chunk: Default::default(),
    //             hash: (),
    //             embeddings: Default::default(),
    //         });
    //     }
    //     Some(&mut chunks[idx])
    // }

    fn read_cache_file<T: DeserializeOwned>(&self, name: &str) -> impl Future<Output = Result<Option<T>, AccessStorageError>> + use<T> {
        let cache_path = self.attachment_path_raw(name);
        // Use async block instead of async fn to avoid capturing lifetime of `&self`
        async move {
            match fs::read_to_string(&cache_path).await {
                Ok(content) => serde_yaml_ng::from_str(&content).map_err(|e| {
                    warn!("Failed to parse cache file: {:?}", cache_path.file_name());
                    AccessStorageError::Metadata(e)
                }),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                    debug!("Cache file not found {:?}", cache_path.file_name());
                    Ok(None)
                },
                Err(e) => {
                    warn!("Failed to read cache file: {:?}", cache_path.file_name());
                    Err(e.into())
                },
            }
        }
    }

    async fn load(&mut self) -> Result<(), AccessStorageError> {
        // Attempt to load cache from cache file
        let read_cache = self.read_cache_file(".cache.yaml");
        // let cache_path = self.attachment_path_raw(".cache.yaml");
        let cache = tokio::spawn(async move {
            read_cache.await
        });

        // Attempt to load metadata from metadata file
        match read_markdown_file::<DocumentMetadata>(&self.metadata_path()).await {
            Ok((text, Ok(metadata))) => {
                let mut text_option = None;
                match metadata.text_location {
                    TextLocation::SourceFile => {
                        // Read source file
                        text_option = Some(fs::read_to_string(&self.absolute_path).await?);
                    },
                    TextLocation::MetadataFile => {
                        text_option = Some(text);
                    },
                    TextLocation::Inferred => {
                        if text.trim_start().is_empty() {
                            text_option = Some(fs::read_to_string(&self.absolute_path).await?);
                        }
                    },
                    TextLocation::None => (),
                }
                self.metadata = metadata;
                self.update_text(text_option);
                return Ok(());
            },
            Ok((_, Err(e))) => {
                return Err(AccessStorageError::Metadata(e));
            },
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                // Metadata file not found, try source file
                debug!("Metadata file not found for document {:?}, trying source file", self.absolute_path);
            },
            Err(e) => {
                return Err(AccessStorageError::Io(e));
            }
        }

        // Read text and metadata from source file
        let mut metadata_location = MetadataLocation::SourceFile;
        let (text, metadata_result) = read_markdown_file::<DocumentMetadata>(&self.absolute_path).await?;
        let metadata = match metadata_result {
            Ok(metadata) => metadata,
            Err(e) => {
                debug!("Failed to read metadata from source file {:?}: {}", self.absolute_path, e);
                // Create metadata file
                let metadata = DocumentMetadata::new(&self.absolute_path);
                write_markdown_file(&self.metadata_path(), "", &metadata).await?;
                metadata_location = MetadataLocation::MetadataFile;
                info!("Created missing metadata file for document {:?}", self.absolute_path);
                metadata
            }
        };
        self.metadata = metadata;
        self.metadata_location = metadata_location;
        self.update_text(Some(text));

        self.cache = cache.await.unwrap().ok().flatten().unwrap_or_default();

        Ok(())
    }

    fn update_text(&mut self, new_text: Option<String>) {
        self.text_hash = None;
        match (new_text, self.metadata.doc_parts.as_ref()) {
            // Text representation of document only has one part
            (Some(text), None) => {
                self.text_parts = vec![(String::new(), text)];
            },
            // Text representation has multiple parts (e.g., spreadsheet converted to multiple
            // tables in markdown or CSV)
            (Some(text), Some(_parts)) => {
                self.text_parts.clear();
                for r in text.to_markdown(WITH_MILESTONES).regions() {
                    if &*r.unit == "part" {
                        let id = r.attribute("id").flatten().map(|s| s.to_string()).unwrap_or_default();
                        if self.text_parts.iter().any(|(part_id, _)| part_id == &id) {
                            warn!("Duplicate text part id '{}' in document {:?}, skipping", id, self.absolute_path);
                            continue;
                        }
                        self.text_parts.push((id, r.content().to_string()));
                    }
                }
            },
            // No text representation available
            (None, _) => {
                self.text_parts.clear();
            },
        };
    }

    fn metadata_path(&self) -> PathBuf {
        self.absolute_path.with_added_extension(METADATA_EXTENSION)
    }

    fn attachment_path_raw(&self, name: &str) -> PathBuf {
        //     .../workspace/doc.md
        self.absolute_path
            // .../workspace/attachments
            .with_file_name(ATTACHMENTS_DIR)
            // .../workspace/attachments/doc.md
            .join(self.absolute_path.file_name().unwrap())
            // .../workspace/attachments/doc.md/attachment_name
            .join(name)
    }

    fn attachment_path_escaped(&self, name: &str) -> PathBuf {
        assert_ne!(name, "", "Attachment name cannot be empty");
        let mut path = self.attachment_path_raw(name);
        let file_name = path.file_name().unwrap().to_str().unwrap();
        // Leading dots are reserved for cache files
        if file_name.starts_with(".") {
            // Escape with additional dot
            path.set_file_name(format!(".{}", file_name));
        }
        path
    }

    fn attachment_name(&self, path: &Path) -> Option<String> {
        let mut file_name = path
            .strip_prefix(self.attachment_path_raw(""))
            .unwrap().to_str().unwrap().to_string();
        
        if !file_name.starts_with("..") {
            file_name.remove(0);    // Unescape leading dot
            Some(file_name)
        } else if file_name.starts_with(".") {
            None    // Cache file, not an attachment
        } else {
            Some(file_name)
        }
    }
}

pub struct Chunks<'a> {
    text_parts: Vec<&'a (String, String)>,
    data: &'a HashMap<String, Vec<ChunkCache>>,
    chunk_idx: usize,
}

impl<'a> Iterator for Chunks<'a> {
    type Item = Chunk<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.text_parts.is_empty() {
            return None;
        }
        let (text_id, _text) = self.text_parts[0];
        let chunks = self.data.get(text_id)?;
        if self.chunk_idx >= chunks.len() {
            self.text_parts.remove(0);
            self.chunk_idx = 0;
            return self.next();
        }
        let chunk_data = &chunks[self.chunk_idx];
        self.chunk_idx += 1;
        Some(Chunk {
            data: chunk_data,
            text: self.text_parts.iter().find(|(id, _text)| id == text_id).unwrap().1.as_str(),
        })
    }
}

pub(crate) async fn open_document(
    absolute_path: &Path,
    workspace: Workspace,
) -> Result<Document, AccessStorageError> {
    // Check if path exists and all that
    let absolute_path = fs::canonicalize(&absolute_path).await
        .map_err(|e| if e.kind() == std::io::ErrorKind::NotFound {
            AccessStorageError::FileNotFound(absolute_path.to_path_buf())
        } else {
            AccessStorageError::Io(e)
    })?;
    // Check if path is inside of workspace
    if !absolute_path.starts_with(&workspace.path()) {
        return Err(AccessStorageError::NotInWorkspace(absolute_path));
    }

    let metadata = DocumentMetadata::new(&absolute_path);
    let mut doc = Document {
        absolute_path,
        workspace,
        metadata,
        metadata_location: MetadataLocation::None,
        text_parts: Vec::new(),
        text_hash: None,
        cache: Default::default(),
        chunk_cache: Default::default(),
    };
    doc.load().await?;
    Ok(doc)
}


async fn read_markdown_file<T: DeserializeOwned>(path: &Path) -> Result<(String, Result<T, serde_yaml_ng::Error>), std::io::Error> {
    let content = fs::read_to_string(path).await?;
    let markdown = content.to_markdown(WITH_MILESTONES);
    let metadata_result = markdown.metadata::<T>();
    let text = String::from(markdown.skip_metadata().as_ref());
    Ok((text, metadata_result))
}

async fn write_markdown_file<T: Serialize + ?Sized>(path: &Path, text: &str, metadata: &T) -> Result<(), AccessStorageError> {
    let metadata = serde_yaml_ng::to_string(metadata)?;
    let markdown = format!("{}\n{}", metadata, text);
    let mut file = OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(path)
        .await?;
    file.write_all(markdown.as_bytes()).await?;
    Ok(())
}

pub enum MetadataLocation {
    SourceFile,
    MetadataFile,
    None,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DocumentMetadata {
    pub id: Uuid,
    pub mime: String,
    #[serde(default)] #[serde(skip_serializing_if = "is_default")]
    pub text_location: TextLocation,
    #[serde(default)] #[serde(skip_serializing_if = "is_default")]
    pub doc_parts: Option<String>,
    #[serde(flatten)]
    other_fields: serde_yaml_ng::Mapping,
}

impl DocumentMetadata {
    pub fn new(path: &Path) -> Self {
        Self {
            id: Uuid::new_v4(),
            // TODO: infer mime type from extension
            mime: "text/markdown".to_string(),
            text_location: Default::default(),
            doc_parts: Default::default(),
            other_fields: Default::default(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum TextLocation {
    SourceFile,
    MetadataFile,
    Inferred,
    None,
}

impl Default for TextLocation {
    fn default() -> Self {
        TextLocation::Inferred
    }
}

fn is_default<T: Default + PartialEq>(value: &T) -> bool {
    value == &T::default()
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct DocCache {
    extensions: HashMap<String, ExtensionCache>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub(crate) struct ExtensionCache {
    #[serde(default)] #[serde(skip_serializing_if = "is_default")]
    hash: Option<TextHash>,
    #[serde(default)] #[serde(skip_serializing_if = "is_default")]
    data: serde_yaml_ng::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ChunkCache {
    #[serde(flatten)]
    chunk: crate::chunking::ChunkData,
    hash: TextHash,
    embeddings: HashMap<String, Embedding>,
}

#[derive(Debug, Clone)]
pub struct Chunk<'a> {
    data: &'a ChunkCache,
    text: &'a str,
}

impl<'a> Chunk<'a> {
    pub fn text(&self) -> &'a str {
        &self.text[self.data.chunk.text_range.clone()]
    }

    pub fn embedding(&self, embedder_id: &str) -> Option<&Embedding> {
        self.data.embeddings.get(embedder_id)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TextHash {
    value: GenericArray<u8, <Sha256 as OutputSizeUser>::OutputSize>,
}

impl<T: Borrow<str> + ?Sized> From<&T> for TextHash {
    fn from(value: &T) -> Self {
        let mut hasher = sha2::Sha256::new();
        hasher.update(value.borrow().as_bytes());
        TextHash { value: hasher.finalize() }
    }
}

impl<'a> FromIterator<&'a str> for TextHash {
    fn from_iter<I: IntoIterator<Item = &'a str>>(iter: I) -> Self {
        let mut hasher = sha2::Sha256::new();
        for value in iter {
            hasher.update(value.as_bytes());
        }
        TextHash { value: hasher.finalize() }
    }
}


impl Serialize for TextHash {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error> where S: serde::Serializer {
        let hex_string = self.value.iter().map(|byte| format!("{:02x}", byte)).collect::<String>();
        serializer.serialize_str(&hex_string)
    }
}

impl<'de> Deserialize<'de> for TextHash {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error> where D: serde::Deserializer<'de> {
        let hex_string = String::deserialize(deserializer)?;
        let bytes = hex::decode(hex_string).map_err(serde::de::Error::custom)?;
        let value = GenericArray::from_slice(&bytes).clone();
        Ok(TextHash { value })
    }
}


#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[tokio::test]
    async fn helper_open_document() {
        // Document without metadata
        let dir = tempdir().unwrap();
        let dir_path = dir.path();
        let doc_path = dir_path.join("doc.md");
        let ws = Workspace::open(&dir_path).await.unwrap();
        fs::write(&doc_path, "foo").await.unwrap();
        let doc = open_document(&doc_path, ws.clone()).await.unwrap();
        assert_eq!(doc.path(), "doc.md");
        assert_eq!(doc.workspace(), &ws);
        assert_eq!(doc.text().as_deref(), Some("foo"));

        // Document with metadata
        let doc_path = dir_path.join("doc2.md");
        let metadata: DocumentMetadata = DocumentMetadata::new(&doc_path);
        let metadata_str = serde_yaml_ng::to_string(&metadata).unwrap();
        fs::write(&doc_path, format!("---\n{}---\nfoo", &metadata_str)).await.unwrap();
        let doc = open_document(&doc_path, ws.clone()).await.unwrap();
        assert_eq!(doc.path(), "doc2.md");
        assert_eq!(doc.workspace(), &ws);
        assert_eq!(doc.metadata.id, metadata.id);
        assert_eq!(doc.text().as_deref(), Some("\nfoo"));
    }

    #[tokio::test]
    async fn extension_cache() {
        let extension_id = "foo";
        let dir = tempdir().unwrap();
        let dir_path = dir.path();
        let doc_path = dir_path.join("doc.md");
        let ws = Workspace::open(&dir_path).await.unwrap();
        fs::write(&doc_path, "hello world").await.unwrap();
        let cache_path = dir_path.join(ATTACHMENTS_DIR).join("doc.md").join(".cache.yaml");
        fs::create_dir_all(cache_path.parent().unwrap()).await.unwrap();
        fs::write(&cache_path, r#"
extensions:
  foo:
    data: 42"#).await.unwrap();
        let doc = open_document(&doc_path, ws.clone()).await.unwrap();
        assert_eq!(doc.path(), "doc.md");
        assert_eq!(doc.workspace(), &ws);
        assert_eq!(doc.text().as_deref(), Some("hello world"));
        assert_eq!(doc.extension_cache(extension_id), Some(&ExtensionCache {
            hash: None,
            data: 42.into(),
        }));
    }
}
