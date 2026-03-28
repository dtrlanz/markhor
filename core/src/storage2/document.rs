use serde::{Deserialize, Serialize, de::DeserializeOwned};
use sha2::{Digest, Sha256, digest::{OutputSizeUser, generic_array::GenericArray}};
use tokio::io::{AsyncWriteExt};
use uuid::Uuid;
use tokio::fs::{self, OpenOptions};
use tracing::{debug, info, instrument, warn};
use std::{borrow::Borrow, collections::{HashMap, hash_map::Entry}, ffi::OsStr, path::{Path, PathBuf}};

use crate::{chunking::{Chunker, ChunkerError}, embedding::Embedding, extension::F11y, markdown::{ToMarkdown, WITH_MILESTONES}, storage2::{ATTACHMENTS_DIR, AccessStorageError, METADATA_EXTENSION, Tag, Workspace}};


#[derive(Debug, Clone)]
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

    pub fn tags(&self) -> impl Iterator<Item = &Tag> {
        // TODO
        std::iter::empty()
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

    fn text_hash(&mut self) -> TextHash {
        if self.text_hash.is_none() {
            let text_iter = self.text_parts.iter()
                .map(|(id, text)| [&**id, &**text].into_iter())
                .flatten();
            self.text_hash = Some(TextHash::from_iter(text_iter));
        }
        self.text_hash.unwrap()
    }

    pub fn doc_hash(&self) -> TextHash {
        // hash of text and metadata
        todo!()
    }

    pub(crate) fn extension_cache(&self, extension: &str) -> Option<&ExtensionCache> {
        self.cache.extensions.get(extension)
    }

    // It's not clear there's a use for this method, so keeping it private for now. It's analogous 
    // to `chunks_mut`, but it still takes `&mut self` because we may need to create/update chunks
    // before iterating.
    /// Returns an iterator over all chunks in the document for the given chunker.
    async fn chunks(&mut self, chunker: &F11y<dyn Chunker>) -> Result<Chunks<'_>, ChunkerError> {
        let chunker_id = chunker.metadata_id();
        let text_hash = self.text_hash();
        let chunk_cache_is_valid = self.cache.extensions
            .get(&chunker_id)
            .map(|ext_cache| ext_cache.hash == Some(text_hash))
            .unwrap_or(false);


        let read_cache = self.read_cache_file(format!(".chunks_{}.yaml", chunker_id).as_str());
        let chunk_cache = match self.chunk_cache.entry(chunker_id) {
            Entry::Occupied(occupied) => {
                debug!("Chunk cache hit for chunker '{}'", chunker.metadata_id());
                occupied.into_mut()
            },
            Entry::Vacant(vacant) => {
                debug!("Chunk cache miss for chunker '{}', attempting to load cache file", chunker.metadata_id());
                let chunk_cache = read_cache.await.ok().flatten().unwrap_or_else(|| {
                    debug!("Failed to load chunk cache for chunker '{}', creating new cache", chunker.metadata_id());
                    HashMap::new()
                });
                vacant.insert(chunk_cache)
            },
        };

        if !chunk_cache_is_valid {
            // Delete cache for any text parts that no longer exist
            let text_part_ids: Vec<_> = self.text_parts.iter().map(|(id, _)| id).collect();
            chunk_cache.retain(|text_id, _| text_part_ids.contains(&text_id));

            // Re-chunk the document
            for (id, text) in self.text_parts.iter() {
                let cached_chunks = chunk_cache.entry(id.clone()).or_default();
                let chunks = chunker.chunk(&text)?;
                debug!("Generated {} chunks for text part '{}'", chunks.len(), id);
                cached_chunks.truncate(chunks.len());
                for (idx, chunk) in chunks.into_iter().enumerate() {
                    let chunk_text = &text[chunk.text_range.clone()];
                    let hash = TextHash::from(chunk_text);
                    if idx < cached_chunks.len() {
                        // Chunk has been cached before; check equality
                        if cached_chunks[idx].chunk == chunk && cached_chunks[idx].hash == hash {
                            // Cache is valid, skip
                            continue;
                        }
                        // Cache is outdated, update & drop invalid embeddings
                        cached_chunks[idx] = ChunkCache {
                            chunk,
                            hash,
                            embeddings: HashMap::new(),
                        };
                    } else {
                        // New chunk, add to cache
                        cached_chunks.push(ChunkCache {
                            chunk: chunk,
                            hash,
                            embeddings: HashMap::new(),
                        });
                    }
                }
            }

            // Update extension cache hash, indicating extension cache overall is now valid
            self.cache.extensions.entry(chunker.metadata_id()).or_default().hash = Some(text_hash);
        }

        Ok(Chunks {
            chunker_id: chunker.metadata_id(),
            text_parts: self.text_parts.iter().collect(),
            data: chunk_cache,
            chunk_idx: 0,
        })
    }

    /// Returns a mutable iterator over all chunks in the document for the given chunker.
    #[instrument(skip(self, chunker))]
    pub async fn chunks_mut(&mut self, chunker: &F11y<dyn Chunker>) -> Result<impl Iterator<Item = ChunkMut<'_>>, ChunkerError> {
        // Ensure chunk cache is valid
        let chunker_id = chunker.metadata_id();
        self.chunks(chunker).await?;

        // Create iterator over all chunks with text parts
        let text_parts = &self.text_parts;
        let with_text_part = self.chunk_cache.get_mut(&chunker_id).unwrap()
            .iter_mut()
            .map(|(text_id, chunks)| {
                let text_part = text_parts.iter().find(|(id, _)| id == text_id).unwrap();
                (text_id, text_part.1.as_str(), chunks)
            });
        let chunk_parts = with_text_part
            .flat_map(|(text_id, text, chunks)| chunks.iter_mut()
            .enumerate()
            .map(move |(idx, chunk_cache)| (text_id, text, chunk_cache, idx)));

        let chunks = chunk_parts
            .map(|(text_id, text, chunk_cache, idx)| ChunkMut {
                text,
                data: chunk_cache,
                idx: ChunkIdx {
                    chunker_id: chunker.metadata_id().to_string(),
                    text_part_id: text_id.clone(),
                    chunk_idx: idx,
                }
            });

        Ok(chunks)
    }

    pub(crate) fn chunk(&self, idx: ChunkIdx) -> Option<Chunk> {
        self.chunk_cache.get(&idx.chunker_id)?.get(&idx.text_part_id)?.get(idx.chunk_idx).and_then(|chunk_cache| {
            let text_part = self.text_parts.iter().find(|(id, _)| id == &idx.text_part_id)?;
            Some(Chunk {
                text: text_part.1.as_str(),
                data: chunk_cache,
                idx,
            })
        })
    }

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
    chunker_id: String,
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
        let chunk = Chunk {
            data: chunk_data,
            text: self.text_parts.iter().find(|(id, _text)| id == text_id).unwrap().1.as_str(),
            idx: ChunkIdx {
                chunker_id: self.chunker_id.clone(),
                text_part_id: text_id.to_string(),
                chunk_idx: self.chunk_idx,
            },
        };
        self.chunk_idx += 1;
        Some(chunk)
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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
    idx: ChunkIdx,
}

impl<'a> Chunk<'a> {
    pub fn text(&self) -> &'a str {
        &self.text[self.data.chunk.text_range.clone()]
    }

    pub fn hash(&self) -> &TextHash {
        &self.data.hash
    }

    pub(crate) fn chunk_idx(&self) -> &ChunkIdx {
        &self.idx
    }

    pub fn embedding(&self, embedder_id: &str) -> Option<&Embedding> {
        self.data.embeddings.get(embedder_id)
    }
}

#[derive(Debug)]
pub struct ChunkMut<'a> {
    data: &'a mut ChunkCache,
    text: &'a str,
    idx: ChunkIdx,
}

impl<'a> ChunkMut<'a> {
    pub fn text(&self) -> &'a str {
        &self.text[self.data.chunk.text_range.clone()]
    }

    pub fn hash(&self) -> &TextHash {
        &self.data.hash
    }

    pub(crate) fn chunk_idx(&self) -> &ChunkIdx {
        &self.idx
    }

    pub fn embedding(&self, embedder_id: &str) -> Option<&Embedding> {
        self.data.embeddings.get(embedder_id)
    }

    pub fn embedding_mut(&mut self, embedder_id: &str) -> Option<&mut Embedding> {
        self.data.embeddings.get_mut(embedder_id)
    }

    pub fn embedding_entry(&mut self, embedder_id: String) -> Entry<'_, String, Embedding> {
        self.data.embeddings.entry(embedder_id)
    }
}

/// Index type for storing chunks in a vector store.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ChunkIdx {
    // TODO: make this more efficient by using numeric IDs for chunkers and text parts instead of strings
    chunker_id: String,
    text_part_id: String,
    chunk_idx: usize,
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
    use crate::{chunking::test_chunker::FixedSizeChunkerExtension, extension::{ActiveExtension}};
    use crate::storage2::fs_test_utils::{TempTree, fs_tree};

    #[tokio::test]
    async fn helper_open_document() {
        let metadata: DocumentMetadata = DocumentMetadata::new("dummy".as_ref()); // Path not used atm (TODO: refactor to not require path?)
        let metadata_str = serde_yaml_ng::to_string(&metadata).unwrap();

        let dir = TempTree::new(fs_tree! {
            "doc.md" => "foo",
            "doc2.md" => { format!("---\n{}---\nfoo", &metadata_str) },
        }).await.unwrap();
        let ws = Workspace::open(&dir).await.unwrap();
        let doc = open_document(&dir.join("doc.md"), ws.clone()).await.unwrap();

        // Document without metadata
        assert_eq!(doc.path(), "doc.md");
        assert_eq!(doc.workspace(), &ws);
        assert_eq!(doc.text().as_deref(), Some("foo"));

        // Document with metadata
        let doc = open_document(&dir.join("doc2.md"), ws.clone()).await.unwrap();
        assert_eq!(doc.path(), "doc2.md");
        assert_eq!(doc.workspace(), &ws);
        assert_eq!(doc.metadata.id, metadata.id);
        assert_eq!(doc.text().as_deref(), Some("\nfoo"));
    }

    #[tokio::test]
    async fn extension_cache() {
        let dir = TempTree::new(fs_tree! {
            "doc.md" => "hello world",
            ATTACHMENTS_DIR => {
                "doc.md" => {
                    ".cache.yaml" => r#"
extensions:
  foo:
    data: 42"#,
                },
            },
        }).await.unwrap();
        let ws = Workspace::open(&dir).await.unwrap();
        let doc = open_document(&dir.join("doc.md"), ws.clone()).await.unwrap();

        assert_eq!(doc.path(), "doc.md");
        assert_eq!(doc.workspace(), &ws);
        assert_eq!(doc.text().as_deref(), Some("hello world"));
        assert_eq!(doc.extension_cache("foo"), Some(&ExtensionCache {
            hash: None,
            data: 42.into(),
        }));
    }

    #[tokio::test]
    async fn iter_chunks() {
        let dir = TempTree::new(fs_tree! {
            "doc.md" => "hello world",
        }).await.unwrap();
        let ws = Workspace::open(&dir).await.unwrap();
        let mut doc = open_document(&dir.join("doc.md"), ws.clone()).await.unwrap();

        assert_eq!(doc.text().as_deref(), Some("hello world"));

        let chunker = ActiveExtension::new(FixedSizeChunkerExtension::new(5), Default::default())
            .chunkers().next().unwrap();
        let mut chunks: Vec<_> = doc.chunks_mut(&chunker).await.unwrap().collect();
        
        let chunk_texts: Vec<&str> = chunks.iter().map(|chunk| chunk.text()).collect();
        assert_eq!(chunk_texts, vec!["hello", " worl", "d"]);

        for (n, chunk) in chunks.iter_mut().enumerate() {
            assert!(chunk.embedding("embedder").is_none());
            chunk.embedding_entry(String::from("embedder")).or_insert(Embedding::from(vec![n as f32]));
        }

        let chunks: Vec<_> = doc.chunks(&chunker).await.unwrap().collect();
        for (n, chunk) in chunks.iter().enumerate() {
            let embedding = chunk.embedding("embedder").unwrap();
            assert_eq!(embedding.as_ref(), &[n as f32]);
        }
    }
}
