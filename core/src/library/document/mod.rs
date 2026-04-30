use mime::Mime;
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use sha2::{Digest, Sha256};
use tokio::{io::AsyncWriteExt};
use uuid::Uuid;
use tokio::fs::{self, OpenOptions};
use tracing::{debug, error, info, instrument, trace, warn};
use std::{collections::{HashMap, hash_map::Entry}, ffi::OsStr, path::{Path, PathBuf}};

use crate::{chunking::{Chunker, ChunkerError}, extension::F11y, markdown::{ToMarkdown, WITH_MILESTONES}, library::{ATTACHMENTS_DIR, AccessLibraryError, HashValue, METADATA_EXTENSION, Tag, Workspace}};

pub mod chunks;
pub mod text;
use chunks::{Chunk, ChunkCache, ChunkIdx, ChunkMut, Chunks, GetChunkError};
pub use text::{Text, Part};

#[derive(Debug, Clone)]
pub struct Document {
    /// Absolute path to the source file
    pub(crate) absolute_path: PathBuf,

    /// Workspace owning this document
    workspace: Workspace,
    text: tokio::sync::OnceCell<Text>,
    metadata: DocumentMetadata,
    metadata_hash: std::sync::OnceLock<HashValue>,
    cache: DocCache,
    chunker_cache: HashMap<String, HashMap<String, Vec<ChunkCache>>>,
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

    pub async fn text(&self) -> Result<&Text, AccessLibraryError> {
        Ok(self.text.get_or_init(|| async { 
            unimplemented!()
        }).await)
    }

    pub async fn text_mut(&mut self) -> Result<&mut Text, AccessLibraryError> {
        Ok(self.text.get_mut().unwrap())
    }

    pub fn metadata(&self) -> &DocumentMetadata {
        &self.metadata
    }

    fn metadata_block(&self) -> Result<String, serde_yaml_ng::Error> {
        let yaml = serde_yaml_ng::to_string(&self.metadata)?;
        Ok(format!("---\n{}---\n", yaml))
    }

    pub async fn doc_hash(&self) -> Result<HashValue, AccessLibraryError> {
        let md_hash = self.metadata_hash.get_or_init(|| {
            let mut hasher = Sha256::new();
            hasher.update(serde_yaml_ng::to_string(&self.metadata).unwrap());
            // Path is included because it affects which scopes the document belongs in the same
            // way that metadata tags do.
            hasher.update(self.absolute_path.as_os_str().as_encoded_bytes());
            HashValue { value: hasher.finalize() }
        });
        Ok(self.text().await?.hash() ^ *md_hash)
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
        // Get reference to text with lifetime tied to `self.text` only, not to `self` as a whole
        if let Err(e) = self.text().await {
            error!("Failed to load text for document {:?} when trying to get chunks for chunker '{}': {}", self.absolute_path, chunker_id, e);
            // Since chunks depend on the text, relevant methods should be refactored accordingly.
            // That will make error handling trivial. Until then, let's just punt.
            // Side note: The solution is probably not simply to move these methods into `text`, 
            // since they also depend on the chunk cache stored in `Document`. (We don't want to
            // the logic around locating and loading cache files across two modules.) Instead,
            // consider something like a struct `Chunked` that wraps access to `Text` alongside
            // a reference to the data associated with a specific chunker.
            todo!("Handle error when loading text for chunks");
        }
        let text = self.text.get().unwrap();

        let text_hash = text.hash();
        let chunker_cache_is_valid = self.cache.extensions
            .get(&chunker_id)
            .map(|ext_cache| ext_cache.hash == Some(text_hash))
            .unwrap_or(false);


        let read_cache = self.read_cache_file(format!(".chunks_{}.yaml", chunker_id).as_str());
        let chunker_cache = Self::load_chunker_cache(
            &mut self.chunker_cache, 
            &chunker_id, 
            read_cache, 
            |e| {
                warn!("Failed to load chunk cache for chunker '{}': {}, creating new cache", chunker.metadata_id(), e);
                Ok(HashMap::new())
            }
        ).await.unwrap();

        if !chunker_cache_is_valid {
            // Delete cache for any text parts that no longer exist
            let text_part_ids: Vec<_> = text.parts().iter().map(|p| p.id()).collect();
            chunker_cache.retain(|text_id, _| text_part_ids.contains(&text_id.as_str()));

            // Re-chunk existing text parts
            for part in text.parts() {
                let cached_chunks = chunker_cache.entry(part.id().into()).or_default();
                part.validate_cached_chunks(chunker, cached_chunks)?;
            }

            // Update extension cache hash, indicating extension cache overall is now valid
            self.cache.extensions.entry(chunker.metadata_id()).or_default().hash = Some(text_hash);
        }

        Ok(Chunks {
            chunker_id: chunker.metadata_id(),
            text_parts: text.parts(),
            data: chunker_cache,
            chunk_idx: 0,
        })
    }

    /// Returns a mutable iterator over all chunks in the document for the given chunker.
    #[instrument(skip(self, chunker))]
    pub async fn chunks_mut(&mut self, chunker: &F11y<dyn Chunker>) -> Result<impl Iterator<Item = ChunkMut<'_>>, ChunkerError> {
        // Ensure chunk cache is valid
        let chunker_id = chunker.metadata_id();
        self.chunks(chunker).await?;

        // Get reference to text parts with lifetime tied to `self.text` only, not to `self` as a whole
        if let Err(e) = self.text().await {
            error!("Failed to load text for document {:?} when trying to get mutable chunks for chunker '{}': {}", self.absolute_path, chunker_id, e);
            // Same issue and solution as in `chunks()`
            todo!("Handle error when loading text for chunks");
        }
        let text_parts = self.text.get().unwrap().parts();

        let with_text_part = self.chunker_cache.get_mut(&chunker_id).unwrap()
            .iter_mut()
            .map(|(text_id, chunks)| {
                let part = text_parts.iter().find(|p| p.id() == text_id).unwrap();
                (text_id, part.as_str(), chunks)
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

    #[instrument(skip(self), fields(doc_path = %self.path().display()), level = "trace", err)]
    pub(crate) fn chunk(&self, idx: ChunkIdx) -> Result<Chunk<'_>, GetChunkError> {
        let text_part = self.text.get()
            .ok_or_else(|| {
                // TODO: come up with a more elegant solution here
                // It might be cleaner if this was a method of `Text` instead of `Document`.
                // Not a big deal for now because the method is not public.
                error!("Text not loaded for document {:?} when trying to get chunk", self.absolute_path);
                GetChunkError::NoSuchTextPart(idx.text_part_id.clone())
            })?
            .parts().iter().find(|p| p.id() == &idx.text_part_id)
            .ok_or_else(|| GetChunkError::NoSuchTextPart(idx.text_part_id.clone()))?;
        self.chunker_cache.get(&idx.chunker_id)
            .ok_or_else(|| GetChunkError::CacheNotLoaded(idx.chunker_id.clone()))?
            .get(&idx.text_part_id)
            .ok_or_else(|| GetChunkError::TextPartNotChunked(idx.chunker_id.clone(), idx.text_part_id.clone()))?
            .get(idx.chunk_idx)
            .ok_or_else(|| GetChunkError::ChunkIdxOutOfBounds(idx.chunker_id.clone(), idx.text_part_id.clone(), idx.chunk_idx))
            .and_then(|chunk_cache| {
                Ok(Chunk {
                    text: text_part.as_str(),
                    data: chunk_cache,
                    idx,
                })
            })
    }

    #[instrument(skip(self), fields(doc_path = %self.path().display()), level = "trace", err)]
    pub(crate) fn chunk_mut(&mut self, idx: ChunkIdx) -> Result<ChunkMut<'_>, GetChunkError> {
        let text_part = self.text.get()
            .ok_or_else(|| {
                // TODO: see above
                error!("Text not loaded for document {:?} when trying to get chunk", self.absolute_path);
                GetChunkError::NoSuchTextPart(idx.text_part_id.clone())
            })?
            .parts().iter().find(|p| p.id() == &idx.text_part_id)
            .ok_or_else(|| GetChunkError::NoSuchTextPart(idx.text_part_id.clone()))?;
        self.chunker_cache.get_mut(&idx.chunker_id)
            .ok_or_else(|| GetChunkError::CacheNotLoaded(idx.chunker_id.clone()))?
            .get_mut(&idx.text_part_id)
            .ok_or_else(|| GetChunkError::TextPartNotChunked(idx.chunker_id.clone(), idx.text_part_id.clone()))?
            .get_mut(idx.chunk_idx)
            .ok_or_else(|| GetChunkError::ChunkIdxOutOfBounds(idx.chunker_id.clone(), idx.text_part_id.clone(), idx.chunk_idx))
            .and_then(|chunk_cache| {
                Ok(ChunkMut {
                    text: text_part.as_str(),
                    data: chunk_cache,
                    idx,
                })
            })
    }

    fn read_cache_file<T: DeserializeOwned>(&self, name: &str) -> impl Future<Output = Result<Option<T>, AccessLibraryError>> + use<T> {
        let cache_path = self.attachment_path_raw(name);
        // Use async block instead of async fn to avoid capturing lifetime of `&self`
        async move {
            match fs::read_to_string(&cache_path).await {
                Ok(content) => serde_yaml_ng::from_str(&content).map_err(|e| {
                    warn!("Failed to parse cache file: {:?}", cache_path.file_name());
                    AccessLibraryError::MetadataFormat(e)
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

    async fn chunker_cache(&self, chunker_id: &str) -> Result<&HashMap<String, Vec<ChunkCache>>, AccessLibraryError> {
        if self.chunker_cache.contains_key(chunker_id) {
            return Ok(self.chunker_cache.get(chunker_id).unwrap());
        }
        let read_cache = self.read_cache_file(format!(".chunks_{}.yaml", chunker_id).as_str());
        let _chunker_cache = Self::load_chunker_cache(
            &mut HashMap::new(), 
            &chunker_id, 
            read_cache, 
            |e| Err(e),
        ).await?;
        // May just remove the method instead of implementing this. It's not clear lazy loading 
        // through a shared reference is even useful. The method `chunk()` couldn't call it
        // regardless because it's not `async`. It seems more straightforward simply to load
        // the cache once through a mutable reference before accessing chunks.
        unimplemented!("Lazy loading of chunk cache not implemented; would need interior mutability");
    }

    #[instrument(skip(self), level = "debug", err)]
    pub(crate) async fn chunker_cache_mut(&mut self, chunker_id: &str) -> Result<&mut HashMap<String, Vec<ChunkCache>>, AccessLibraryError> {
        let read_cache = self.read_cache_file(format!(".chunks_{}.yaml", chunker_id).as_str());
        Self::load_chunker_cache(
            &mut self.chunker_cache, 
            &chunker_id, 
            read_cache, 
            |e| Err(e),
        ).await
    }

    #[instrument(skip(chunker_cache, read_cache, handle_err), level = "debug", err)]
    async fn load_chunker_cache<'a, F: FnOnce(AccessLibraryError) -> Result<HashMap<String, Vec<ChunkCache>>, AccessLibraryError> + Sized>(
        chunker_cache: &'a mut HashMap<String, HashMap<String, Vec<ChunkCache>>>,
        chunker_id: &str,
        read_cache: impl Future<Output = Result<Option<HashMap<String, Vec<ChunkCache>>>, AccessLibraryError>>,
        handle_err: F,
    ) -> Result<&'a mut HashMap<String, Vec<ChunkCache>>, AccessLibraryError> {
        match chunker_cache.entry(chunker_id.to_string()) {
            Entry::Occupied(occupied) => Ok(occupied.into_mut()),
            Entry::Vacant(vacant) => {
                let chunker_cache = read_cache.await
                    .map(|opt| opt.unwrap_or_else(|| {
                        debug!("Cache not found for chunker '{}', creating new cache", chunker_id);
                        HashMap::new()
                    }))
                    .or_else(handle_err)?;
                Ok(vacant.insert(chunker_cache))
            },
        }
    }

    #[instrument(skip(self, data), level = "debug", err)]
    async fn write_cache_file<T: Serialize + ?Sized>(&self, name: &str, data: &T) -> Result<(), AccessLibraryError> {
        let cache_path = self.attachment_path_raw(name);
        debug!("Writing cache file {:?}", cache_path);
        let content = serde_yaml_ng::to_string(data)?;
        trace!("Serialized content length: {} bytes", content.len());
        // Ensure directory exists
        fs::create_dir_all(cache_path.parent().unwrap()).await?;
        let mut file = OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(cache_path).await?;
        file.write_all(content.as_bytes()).await?;
        trace!("Finished writing cache file");
        Ok(())
    }

    pub(crate) async fn create(workspace: Workspace, absolute_path: &Path) -> Result<Self, AccessLibraryError> {
        // TODO: Add unit tests
        // Check if path is valid and does not already exist
        let absolute_path = fs::canonicalize(absolute_path.parent().unwrap()).await
            .map_err(|e| if e.kind() == std::io::ErrorKind::NotFound {
                AccessLibraryError::DirectoryNotFound(absolute_path.parent().unwrap().to_path_buf())
            } else {
                AccessLibraryError::Io(e)
            })?;
        if absolute_path.exists() {
            todo!("Handle error when creating document with path that already exists");
            // return Err(AccessStorageError::FileAlreadyExists(absolute_path.to_path_buf()));
        }
        let metadata = DocumentMetadata::from_path(&absolute_path);
        let mut doc = Self {
            absolute_path,
            workspace,
            text: Default::default(),
            metadata,
            metadata_hash: Default::default(),
            cache: Default::default(),
            chunker_cache: Default::default(),
        };
        doc.save().await?;
        Ok(doc)
    }

    pub(crate) async fn open(workspace: Workspace, absolute_path: &Path) -> Result<Self, AccessLibraryError> {
        // Check if path exists and all that
        let absolute_path = fs::canonicalize(&absolute_path).await
            .map_err(|e| if e.kind() == std::io::ErrorKind::NotFound {
                AccessLibraryError::FileNotFound(absolute_path.to_path_buf())
            } else {
                AccessLibraryError::Io(e)
        })?;
        // Check if path is inside of workspace
        if !absolute_path.starts_with(&workspace.path()) {
            return Err(AccessLibraryError::NotInWorkspace(absolute_path));
        }

        let metadata = DocumentMetadata::from_path(&absolute_path);
        let mut doc = Document {
            absolute_path,
            workspace,
            text: Default::default(),
            metadata,
            metadata_hash: Default::default(),
            cache: Default::default(),
            chunker_cache: Default::default(),
        };
        doc.load().await?;
        Ok(doc)
    }

    #[instrument(skip(self), level = "debug", err)]
    async fn load(&mut self) -> Result<(), AccessLibraryError> {
        // Attempt to load cache from cache file
        let read_cache = self.read_cache_file(".cache.yaml");
        let cache = tokio::spawn(async move {
            read_cache.await
        });

        let mut text_import = None;

        if self.metadata.metadata_location == MetadataLocation::MetadataFile 
            || self.metadata.metadata_location == MetadataLocation::Unknown
        {
            // Attempt to load metadata from metadata file
            match read_markdown_file::<DocumentMetadata>(&self.metadata_path()).await {
                Ok((text, Ok(metadata))) => {
                    debug!("Loaded metadata from metadata file {:?}", self.metadata_path());
                    if metadata.metadata_location == MetadataLocation::MetadataFile {
                        if metadata.source_is_text() {
                            // Source is text file. Ignore any text suffixed to metadata.
                            if text.trim_end() != "" {
                                warn!("Ignoring text content in metadata file {:?}.", self.metadata_path());
                            }
                        } else {
                            // Source is not a text file. Use text suffixed to metadata if available.
                            if text.trim_start() != "" {
                                text_import = Some(text);
                            }
                        }
                        self.metadata = metadata;
                        self.metadata.metadata_location.update_unknown(MetadataLocation::MetadataFile);
                    } else {
                        warn!("{:?} appears to be a metadata file and contains valid metadata, but does not declare metadata location as `metadata_location: metadata-file`; treating it as an independent file unrelated to {:?}", self.metadata_path(), self.path());
                    }
                },
                Ok((_, Err(e))) => {
                    warn!("{:?} does not contain valid metadata ({}); treating it as an independent file unrelated to {:?}", self.metadata_path(), e, self.path());
                },
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                    if self.metadata.metadata_location == MetadataLocation::MetadataFile {
                        return Err(AccessLibraryError::MetadataLocation(MetadataLocation::MetadataFile, "Metadata file not found".into()));
                    }
                    // Metadata file not found, try source file
                    debug!("Metadata file not found for document {:?}, trying source file", self.absolute_path);
                },
                Err(e) => {
                    return Err(AccessLibraryError::Io(e));
                },
            }
        }

        if self.metadata.metadata_location == MetadataLocation::SourceFile 
            || self.metadata.metadata_location == MetadataLocation::Unknown
            && self.metadata.source_is_markdown()
        {
            // Read text and metadata from source file
            let (text, metadata_result) = read_markdown_file::<DocumentMetadata>(&self.absolute_path).await?;
            let metadata = match metadata_result {
                Ok(metadata) => metadata,
                Err(e) => {
                    if self.metadata.metadata_location == MetadataLocation::SourceFile {
                        // Metadata was expected. Return error.
                        return Err(AccessLibraryError::MetadataFormat(e));
                    }
                    // Not an error. Metadata blocks are not required.
                    debug!("Could not read metadata from source file {:?}: {}", self.absolute_path, e);
                    // Create default metadata
                    let mut metadata = DocumentMetadata::from_path(&self.absolute_path);
                    metadata.metadata_location = MetadataLocation::None;
                    metadata
                }
            };
            self.metadata = metadata;
            text_import = Some(text);
        } else {
            self.metadata.metadata_location.update_unknown(MetadataLocation::None);
            if self.metadata.source_is_text() {
                // Read text from source file
                let text = fs::read_to_string(&self.absolute_path).await?;
                text_import = Some(text);
            }
        }

        // The API is meant to support accessing only a document's metadata and loading everything
        // else lazily. This is helpful when you're filtering documents by metadata and don't want 
        // to load more data than necessary.
        // However, the current implementation still loads the text content and some of the cache 
        // immediately, so we're getting the worst of both worlds.
        // TODO: Be lazy
        let mut text = Text::new();
        text.import(text_import.as_deref(), self.metadata.doc_parts.as_deref());
        self.text = text.into();

        self.cache = cache.await.unwrap().ok().flatten().unwrap_or_default();

        Ok(())
    }

    #[instrument(skip(self), fields(doc_path = %self.path().display()), err)]
    pub async fn save(&mut self) -> Result<(), AccessLibraryError> {
        info!("Saving document {:?}", self.absolute_path);
        let mut doc_parts = self.metadata.doc_parts.clone();
        let text_export = self.text().await?.export(&mut doc_parts);
        self.metadata.doc_parts = doc_parts;
        // Save text and metadata to appropriate location
        match self.metadata.metadata_location {
            MetadataLocation::SourceFile => {
                if !self.metadata.source_is_markdown() {
                    return Err(AccessLibraryError::MetadataLocation(
                        self.metadata.metadata_location, 
                        "Cannot save metadata in source file that is not markdown".into(),
                    ));
                }
                // Save both in source file
                debug!("Saving text and metadata in source file for document {:?}", self.absolute_path);
                let mut output = self.metadata_block()?;
                output.push_str(&text_export.unwrap_or_default());
                fs::write(&self.absolute_path, output).await?;
            },
            MetadataLocation::MetadataFile => {
                if self.metadata.source_is_text() {
                    // Save to separate files
                    debug!("Saving text in source file and metadata in metadata file for document {:?}", self.absolute_path);
                    fs::write(&self.absolute_path, text_export.unwrap_or_default()).await?;
                    trace!("Metadata file path: {:?}", self.metadata_path());
                    fs::write(&self.metadata_path(), self.metadata_block()?).await?;
                } else {
                    // Source is not a text file. Save text in metadata file.
                    debug!("Saving text and metadata in metadata file for document {:?}", self.absolute_path);
                    let mut content = self.metadata_block()?;
                    content.push_str(&text_export.unwrap_or_default());
                    fs::write(&self.metadata_path(), content).await?;
                }
            },
            MetadataLocation::None => {
                if let Some(text) = text_export {
                    if self.metadata.source_is_text() {
                        debug!("Saving text in source file for document {:?}", self.absolute_path);
                        fs::write(&self.absolute_path, text).await?;
                    } else {
                        warn!("Text will not be saved for document {:?} because metadata_location is None and source is not a text file.", self.absolute_path);
                    }
                }
                if !self.metadata.inferrable(&self.absolute_path) {
                    warn!("Metadata will not be saved for document {:?} because metadata_location is None.", self.absolute_path);
                }
            },
            MetadataLocation::Unknown => {
                return Err(AccessLibraryError::MetadataLocation(MetadataLocation::Unknown, "Invalid when saving".into()));
            }
        }

        // Save cache to cache file
        self.write_cache_file(".cache.yaml", &self.cache).await?;

        // Save chunk cache to chunk cache file for each chunker
        for (chunker_id, chunk_cache) in &self.chunker_cache {
            self.write_cache_file(format!(".chunks_{}.yaml", chunker_id).as_str(), chunk_cache).await?;
        }

        Ok(())
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

async fn read_markdown_file<T: DeserializeOwned>(path: &Path) -> Result<(String, Result<T, serde_yaml_ng::Error>), std::io::Error> {
    let content = fs::read_to_string(path).await?;
    let markdown = content.to_markdown(WITH_MILESTONES);
    let metadata_result = markdown.metadata::<T>();
    let text = String::from(markdown.skip_metadata().as_ref());
    Ok((text, metadata_result))
}

async fn write_markdown_file<T: Serialize + ?Sized>(path: &Path, text: &str, metadata: &T) -> Result<(), AccessLibraryError> {
    let metadata = serde_yaml_ng::to_string(metadata)?;
    let markdown = format!("---\n{}\n---\n{}", metadata, text);
    let mut file = OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(path)
        .await?;
    file.write_all(markdown.as_bytes()).await?;
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum MetadataLocation {
    SourceFile,
    MetadataFile,
    None,
    Unknown,    // Used temporarily when opening a document, then replaced with
                // the actual location
}

impl MetadataLocation {
    fn update_unknown(&mut self, location: MetadataLocation) {
        if *self == MetadataLocation::Unknown {
            *self = location;
        }
    }
}

impl Default for MetadataLocation {
    fn default() -> Self {
        // Default is not `Unknown` because when you're deserializing metadata, you know where it
        // is stored. Using `SourceFile` as default has the advantage that you can shave 1 line
        // off metadata blocks in source files (since default values do not need to be serialized).
        // The assumption is that in most cases, you'd rather have the source file slightly less 
        // cluttered than the metadata file.
        MetadataLocation::SourceFile
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DocumentMetadata {
    pub id: Uuid,
    #[serde(with = "mime_serde")]
    pub mime: Mime,
    // #[serde(default)] #[serde(skip_serializing_if = "is_default")]
    // pub text_location: TextLocation,
    #[serde(default)] #[serde(skip_serializing_if = "is_default")]
    pub metadata_location: MetadataLocation,
    #[serde(default)] #[serde(skip_serializing_if = "is_default")]
    pub doc_parts: Option<String>,
    #[serde(flatten)]
    other_fields: serde_yaml_ng::Mapping,
}

impl DocumentMetadata {
    pub fn new(mime: Mime) -> Self {
        Self {
            id: Uuid::new_v4(),
            mime,
            // text_location: Default::default(),
            metadata_location: MetadataLocation::Unknown,
            doc_parts: Default::default(),
            other_fields: Default::default(),
        }
    }

    pub fn from_path(path: &Path) -> Self {
        let mime = mime_guess2::from_path(path).first_or_octet_stream();
        Self::new(mime)
    }

    pub fn source_is_text(&self) -> bool {
        self.mime.type_() == mime::TEXT 
            || self.mime.suffix() == Some(mime::JSON) 
            || self.mime.suffix() == Some(mime::XML)
    }

    pub fn source_is_markdown(&self) -> bool {
        self.mime.type_() == mime::TEXT && self.mime.subtype() == "markdown"
    }

    /// Checks if the document metadata that could be inferred correctly.
    /// 
    /// Tests whether the metadata that would be inferred given the provided path (e.g., when 
    /// creating a new document) matches the existing metadata. The document ID is ignored since 
    /// this can never be inferred.
    /// 
    /// This method is useful when testing whether metadata needs to be saved to avoid data loss.
    pub fn inferrable(&self, path: &Path) -> bool {
        let mut inferred = Self::from_path(path);
        inferred.id = self.id;   // ID should not affect inferrability
        inferred == *self
    }
}

mod mime_serde {
    use super::*;
    use serde::{Serializer, Deserializer};

    pub fn serialize<S>(mime: &Mime, serializer: S) -> Result<S::Ok, S::Error> where S: Serializer {
        serializer.serialize_str(mime.as_ref())
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<Mime, D::Error> where D: Deserializer<'de> {
        let s = String::deserialize(deserializer)?;
        s.parse::<Mime>().map_err(serde::de::Error::custom)
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
    hash: Option<HashValue>,
    #[serde(default)] #[serde(skip_serializing_if = "is_default")]
    data: serde_yaml_ng::Value,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::library::document::text::Part;
    use crate::{chunking::test_chunker::FixedSizeChunkerExtension, embedding::Embedding, extension::ActiveExtension};
    use crate::library::fs_test_utils::{TempTree, fs_tree};

    #[tokio::test]
    async fn document_open() {
        let metadata: DocumentMetadata = DocumentMetadata::new("text/markdown".parse().unwrap());
        let metadata_str = serde_yaml_ng::to_string(&metadata).unwrap();

        let dir = TempTree::new(fs_tree! {
            "doc.md" => "foo",
            "doc2.md" => { format!("---\n{}---\nfoo", &metadata_str) },
        }).await.unwrap();
        let ws = Workspace::open(&dir).await.unwrap();

        // Document without metadata
        let doc = Document::open(ws.clone(), &dir.join("doc.md")).await.unwrap();
        assert_eq!(doc.path(), "doc.md");
        assert_eq!(doc.workspace(), &ws);
        assert_eq!(doc.text().await.unwrap().export(&mut None).as_deref(), Some("foo"));

        // Document with metadata
        let doc = Document::open(ws.clone(), &dir.join("doc2.md")).await.unwrap();
        assert_eq!(doc.path(), "doc2.md");
        assert_eq!(doc.workspace(), &ws);
        assert_eq!(doc.metadata.id, metadata.id);
        assert_eq!(doc.text().await.unwrap().export(&mut None).as_deref(), Some("foo"));
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
        let doc = Document::open(ws.clone(), &dir.join("doc.md")).await.unwrap();

        assert_eq!(doc.path(), "doc.md");
        assert_eq!(doc.workspace(), &ws);
        assert_eq!(doc.text().await.unwrap().export(&mut None).as_deref(), Some("hello world"));
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
        let mut doc = Document::open(ws.clone(), &dir.join("doc.md")).await.unwrap();

        assert_eq!(doc.text().await.unwrap().export(&mut None).as_deref(), Some("hello world"));

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

    #[tokio::test]
    async fn doc_text_hash() {
        let dir = TempTree::new(fs_tree! {
            "doc.md" => "hello world",
        }).await.unwrap();
        let ws = Workspace::open(&dir).await.unwrap();
        let mut doc = Document::open(ws.clone(), &dir.join("doc.md")).await.unwrap();
        let initial_doc_hash = doc.doc_hash().await.unwrap();
        let text = doc.text_mut().await.unwrap();
        let initial_text_hash = text.hash();

        // Update text and check that hashes change
        text.parts_mut()[0].push_str("!");
        let updated_text_hash = text.hash();
        let updated_doc_hash = doc.doc_hash().await.unwrap();

        assert_ne!(initial_text_hash, updated_text_hash);
        assert_ne!(initial_doc_hash, updated_doc_hash);
    }

    #[tokio::test]
    #[test_log::test]
    async fn save_both_in_source_file() {
        let dir: TempTree<'_> = TempTree::new(fs_tree! {
            "doc.md" => "",
        }).await.unwrap();
        let ws = Workspace::open(&dir).await.unwrap();
        let mut doc = Document::open(ws.clone(), &dir.join("doc.md")).await.unwrap();

        // Set locations
        doc.metadata.metadata_location = MetadataLocation::SourceFile;
        // Set text
        doc.text_mut().await.unwrap().import(Some("Hello world"), None);

        // Save
        doc.save().await.unwrap();

        // Check source file
        let content = fs::read_to_string(&dir.join("doc.md")).await.unwrap();
        let expected_metadata = serde_yaml_ng::to_string(&doc.metadata).unwrap();
        assert_eq!(content, format!("---\n{}---\nHello world", expected_metadata));
    }

    #[tokio::test]
    #[test_log::test]
    async fn save_text_in_source_metadata_in_metadata_file() {
        let dir = TempTree::new(fs_tree! {
            "doc.md" => "",
        }).await.unwrap();
        let ws = Workspace::open(&dir).await.unwrap();
        let mut doc = Document::open(ws.clone(), &dir.join("doc.md")).await.unwrap();

        // Set locations
        doc.metadata.metadata_location = MetadataLocation::MetadataFile;
        // Set text
        doc.text_mut().await.unwrap().import(Some("Hello world"), None);

        // Save
        doc.save().await.unwrap();

        // Check source file has text
        let source_content = fs::read_to_string(&dir.join("doc.md")).await.unwrap();
        assert_eq!(source_content, "Hello world");

        // Check metadata file
        let metadata_content = fs::read_to_string(&dir.join(format!("doc.md.{}", METADATA_EXTENSION))).await.unwrap();
        let expected_metadata = serde_yaml_ng::to_string(&doc.metadata).unwrap();
        assert_eq!(metadata_content, format!("---\n{}---\n", expected_metadata));
    }

    #[tokio::test]
    #[test_log::test]
    async fn save_both_in_metadata_file() {
        let dir = TempTree::new(fs_tree! {
            "doc.pdf" => "",
        }).await.unwrap();
        let ws = Workspace::open(&dir).await.unwrap();
        let mut doc = Document::open(ws.clone(), &dir.join("doc.pdf")).await.unwrap();

        // Set locations
        doc.metadata.metadata_location = MetadataLocation::MetadataFile;
        // Set text
        doc.text_mut().await.unwrap().import(Some("Hello world"), None);

        // Save
        doc.save().await.unwrap();

        // Check source file is empty or unchanged
        let source_content = fs::read_to_string(&dir.join("doc.pdf")).await.unwrap();
        assert_eq!(source_content, "");

        // Check metadata file has both
        let metadata_content = fs::read_to_string(&dir.join(format!("doc.pdf.{}", METADATA_EXTENSION))).await.unwrap();
        let expected_metadata = serde_yaml_ng::to_string(&doc.metadata).unwrap();
        assert_eq!(metadata_content, format!("---\n{}---\nHello world", expected_metadata));
    }

    #[tokio::test]
    #[test_log::test]
    async fn save_doc_cache() {
        let dir = TempTree::new(fs_tree! {
            "doc.md" => "hello world",
        }).await.unwrap();
        let ws = Workspace::open(&dir).await.unwrap();
        let mut doc = Document::open(ws.clone(), &dir.join("doc.md")).await.unwrap();

        // Update cache
        doc.cache.extensions.insert("foo".to_string(), ExtensionCache {
            hash: Some(HashValue::from("hello world")),
            data: 42.into(),
        });

        // Save
        doc.save().await.unwrap();

        // Check cache file
        let cache_content = fs::read_to_string(&dir.join(ATTACHMENTS_DIR).join("doc.md").join(".cache.yaml")).await.unwrap();
        let expected_cache = serde_yaml_ng::to_string(&doc.cache).unwrap();
        assert_eq!(cache_content, expected_cache);
    }

    #[tokio::test]
    #[test_log::test]
    async fn save_chunk_cache() {
        let dir = TempTree::new(fs_tree! {
            "doc.md" => "hello world",
        }).await.unwrap();
        let ws = Workspace::open(&dir).await.unwrap();
        let mut doc = Document::open(ws.clone(), &dir.join("doc.md")).await.unwrap();

        // Create chunk cache for a chunker
        let chunker_id = "test_chunker".to_string();
        let text_part_id = String::new();
        doc.chunker_cache.insert(chunker_id.clone(), HashMap::from([
            (text_part_id.clone(), vec![
                ChunkCache {
                    chunk: crate::chunking::ChunkData { text_range: 0..5, heading_path: None, token_count: None },
                    hash: HashValue::from("hello"),
                    embeddings: HashMap::new(),
                },
                ChunkCache {
                    chunk: crate::chunking::ChunkData { text_range: 5..11, heading_path: None, token_count: None },
                    hash: HashValue::from(" world"),
                    embeddings: HashMap::new(),
                },
            ])
        ]));

        // Save
        doc.save().await.unwrap();

        // Check chunk cache file
        let cache_content = fs::read_to_string(&dir.join(ATTACHMENTS_DIR).join("doc.md").join(format!(".chunks_{}.yaml", chunker_id))).await.unwrap();
        let expected_cache = serde_yaml_ng::to_string(&doc.chunker_cache.get(&chunker_id)).unwrap();
        assert_eq!(cache_content, expected_cache);
    }

    #[tokio::test]
    #[test_log::test]
    async fn save_multiple_text_parts() {
        let dir = TempTree::new(fs_tree! {
            "doc.md" => "",
        }).await.unwrap();
        let ws = Workspace::open(&dir).await.unwrap();
        let mut doc = Document::open(ws.clone(), &dir.join("doc.md")).await.unwrap();

        // Ensure metadata is saved
        doc.metadata.metadata_location = MetadataLocation::SourceFile;

        // Set metadata to have multiple text parts
        doc.metadata.doc_parts = Some("not-none".to_string());
        // Set text parts
        let text: Text = vec![
            Part::new("part1", "Hello"),
            Part::new("part2", "world"),
        ].into_iter().collect();
        doc.text = text.into();

        // Save
        doc.save().await.unwrap();

        // Check metadata file has both text parts
        let file_content = fs::read_to_string(&dir.join("doc.md")).await.unwrap();
        let expected_metadata = serde_yaml_ng::to_string(&doc.metadata).unwrap();
        assert_eq!(file_content, format!("---\n{}---\n<milestone unit=\"part\" n=\"part1\" />\nHello\n<milestone unit=\"part\" n=\"part2\" />\nworld\n", expected_metadata));
    }

    // The following 8 tests cover the different constellations of original content (UTF-8 or 
    // binary), text content (UTF-8, if any) and metadata (YAML, if any) that the current
    // implementation is designed to support. The possibilities may be grouped by source
    // content as follows:
    //
    // ## Source is Markdown
    // - no binary content
    // - text is always in source file
    // - metadata may be in source file, in separate metadata file, or missing
    //
    // ## Source is UTF-8 but not Markdown (e.g., CSV)
    // - no binary content
    // - text is always in source file
    // - metadata may be in separate metadata file or missing (metadata in source file is not 
    //   supported for non-Markdown files)
    //
    // ## Source is binary (or not guaranteed to be UTF-8)
    // - source file contains binary content, no text content
    // - text (e.g., OCRed or transcribed) may be in separate metadata file or missing
    // - metadata may be in separate metadata file or missing; if metadata file exists, it must 
    //   contain metadata and not text only
    //
    // Across all constellations, the implementation should infer which elements exist and where
    // they are located, even if metadata does not specify these details.
    //
    // Moreover, the implementation should not modify files unexpectedly. Metadata files should
    // not be created or updated implicitly when opening a document. When a document is loaded
    // and then saved without changes, files should remain unchanged.

    #[tokio::test]
    #[test_log::test]
    async fn markdown_without_metadata() {
        let dir = TempTree::new(fs_tree! {
            "doc.md" => "hello world",
        }).await.unwrap();
        let ws = Workspace::open(&dir).await.unwrap();

        // Load document
        let mut doc = Document::open(ws.clone(), &dir.join("doc.md")).await.unwrap();
        assert_eq!(doc.path(), "doc.md");
        assert_eq!(doc.text().await.unwrap().export(&mut None).as_deref(), Some("hello world"));

        // Save document
        doc.save().await.unwrap();

        // File(s) must be unchanged
        let content = fs::read_to_string(&dir.join("doc.md")).await.unwrap();
        assert_eq!(content, "hello world");
        let metadata_path = dir.join(format!("doc.md.{}", METADATA_EXTENSION));
        assert_eq!(fs::try_exists(metadata_path).await.unwrap(), false);
    }

    #[tokio::test]
    #[test_log::test]
    async fn markdown_with_integrated_metadata() {
        let dir = TempTree::new(fs_tree! {
            "doc.md" => "---\nid: 123e4567-e89b-12d3-a456-426614174000\nmime: text/markdown\n---\nhello world",
        }).await.unwrap();
        let ws = Workspace::open(&dir).await.unwrap();

        // Load document
        let mut doc = Document::open(ws.clone(), &dir.join("doc.md")).await.unwrap();
        assert_eq!(doc.path(), "doc.md");
        assert_eq!(doc.text().await.unwrap().export(&mut None).as_deref(), Some("hello world"));
        assert_eq!(doc.metadata.id, Uuid::parse_str("123e4567-e89b-12d3-a456-426614174000").unwrap());

        // Save document
        doc.save().await.unwrap();

        // File(s) must be unchanged
        let content = fs::read_to_string(&dir.join("doc.md")).await.unwrap();
        assert_eq!(content, "---\nid: 123e4567-e89b-12d3-a456-426614174000\nmime: text/markdown\n---\nhello world");
    }

    #[tokio::test]
    #[test_log::test]
    async fn markdown_with_separate_metadata_file() {
        let dir = TempTree::new(fs_tree! {
            "doc.md" => "hello world",
            format!("doc.md.{}", METADATA_EXTENSION) => "---\nid: 123e4567-e89b-12d3-a456-426614174000\nmime: text/markdown\nmetadata_location: metadata-file\n---\n",
        }).await.unwrap();
        let ws = Workspace::open(&dir).await.unwrap();

        // Load document
        let mut doc = Document::open(ws.clone(), &dir.join("doc.md")).await.unwrap();
        assert_eq!(doc.path(), "doc.md");
        assert_eq!(doc.text().await.unwrap().export(&mut None).as_deref(), Some("hello world"));
        assert_eq!(doc.metadata.id, Uuid::parse_str("123e4567-e89b-12d3-a456-426614174000").unwrap());

        // Save document
        doc.save().await.unwrap();

        // File(s) must be unchanged
        let content = fs::read_to_string(&dir.join("doc.md")).await.unwrap();
        assert_eq!(content, "hello world");
        let metadata = fs::read_to_string(&dir.join(format!("doc.md.{}", METADATA_EXTENSION))).await.unwrap();
        assert_eq!(metadata, "---\nid: 123e4567-e89b-12d3-a456-426614174000\nmime: text/markdown\nmetadata_location: metadata-file\n---\n");
    }

    #[tokio::test]
    #[test_log::test]
    async fn csv_without_metadata() {
        let dir = TempTree::new(fs_tree! {
            "doc.csv" => "a,b,c\n1,2,3",
        }).await.unwrap();
        let ws = Workspace::open(&dir).await.unwrap();

        // Load document
        let mut doc = Document::open(ws.clone(), &dir.join("doc.csv")).await.unwrap();
        assert_eq!(doc.path(), "doc.csv");
        assert_eq!(doc.text().await.unwrap().export(&mut None).as_deref(), Some("a,b,c\n1,2,3"));

        // Save document
        doc.save().await.unwrap();

        // File(s) must be unchanged
        let content = fs::read_to_string(&dir.join("doc.csv")).await.unwrap();
        assert_eq!(content, "a,b,c\n1,2,3");
        let metadata_path = dir.join(format!("doc.csv.{}", METADATA_EXTENSION));
        assert_eq!(fs::try_exists(metadata_path).await.unwrap(), false);
    }

    #[tokio::test]
    #[test_log::test]
    async fn csv_with_separate_metadata_file() {
        let dir = TempTree::new(fs_tree! {
            "doc.csv" => "a,b,c\n1,2,3",
            format!("doc.csv.{}", METADATA_EXTENSION) => "---\nid: 123e4567-e89b-12d3-a456-426614174000\nmime: text/csv\nmetadata_location: metadata-file\n---\n",
        }).await.unwrap();
        let ws = Workspace::open(&dir).await.unwrap();

        // Load document
        let mut doc = Document::open(ws.clone(), &dir.join("doc.csv")).await.unwrap();
        assert_eq!(doc.path(), "doc.csv");
        assert_eq!(doc.text().await.unwrap().export(&mut None).as_deref(), Some("a,b,c\n1,2,3"));
        assert_eq!(doc.metadata.id, Uuid::parse_str("123e4567-e89b-12d3-a456-426614174000").unwrap());

        // Save document
        doc.save().await.unwrap();

        // File(s) must be unchanged
        let content = fs::read_to_string(&dir.join("doc.csv")).await.unwrap();
        assert_eq!(content, "a,b,c\n1,2,3");
        let metadata = fs::read_to_string(&dir.join(format!("doc.csv.{}", METADATA_EXTENSION))).await.unwrap();
        assert_eq!(metadata, "---\nid: 123e4567-e89b-12d3-a456-426614174000\nmime: text/csv\nmetadata_location: metadata-file\n---\n");
    }

    #[tokio::test]
    #[test_log::test]
    async fn pdf_without_text_or_metadata() {
        let dir = TempTree::new(fs_tree! {
            "doc.pdf" => "%PDF-1.4\n%âãÏÓ\n1 0 obj\n<< /Type /Catalog >>\nendobj\ntrailer\n<< /Root 1 0 R >>\n%%EOF",
        }).await.unwrap();
        let ws = Workspace::open(&dir).await.unwrap();

        // Load document
        let mut doc = Document::open(ws.clone(), &dir.join("doc.pdf")).await.unwrap();
        assert_eq!(doc.path(), "doc.pdf");
        assert_eq!(doc.text().await.unwrap().export(&mut None), None);

        // Save document
        doc.save().await.unwrap();

        // File(s) must be unchanged
        let content = fs::read_to_string(&dir.join("doc.pdf")).await.unwrap();
        assert_eq!(content, "%PDF-1.4\n%âãÏÓ\n1 0 obj\n<< /Type /Catalog >>\nendobj\ntrailer\n<< /Root 1 0 R >>\n%%EOF");
        let metadata_path = dir.join(format!("doc.pdf.{}", METADATA_EXTENSION));
        assert_eq!(fs::try_exists(metadata_path).await.unwrap(), false);
    }

    #[tokio::test]
    #[test_log::test]
    async fn pdf_with_metadata_without_text() {
        let dir = TempTree::new(fs_tree! {
            "doc.pdf" => "%PDF-1.4\n%âãÏÓ\n1 0 obj\n<< /Type /Catalog >>\nendobj\ntrailer\n<< /Root 1 0 R >>\n%%EOF",
            format!("doc.pdf.{}", METADATA_EXTENSION) => "---\nid: 123e4567-e89b-12d3-a456-426614174000\nmime: application/pdf\nmetadata_location: metadata-file\n---\n",
        }).await.unwrap();
        let ws = Workspace::open(&dir).await.unwrap();

        // Load document
        let mut doc = Document::open(ws.clone(), &dir.join("doc.pdf")).await.unwrap();
        assert_eq!(doc.path(), "doc.pdf");
        assert_eq!(doc.text().await.unwrap().export(&mut None), None);
        assert_eq!(doc.metadata.id, Uuid::parse_str("123e4567-e89b-12d3-a456-426614174000").unwrap());

        // Save document
        doc.save().await.unwrap();

        // File(s) must be unchanged
        let content = fs::read_to_string(&dir.join("doc.pdf")).await.unwrap();
        assert_eq!(content, "%PDF-1.4\n%âãÏÓ\n1 0 obj\n<< /Type /Catalog >>\nendobj\ntrailer\n<< /Root 1 0 R >>\n%%EOF");
        let metadata = fs::read_to_string(&dir.join(format!("doc.pdf.{}", METADATA_EXTENSION))).await.unwrap();
        assert_eq!(metadata, "---\nid: 123e4567-e89b-12d3-a456-426614174000\nmime: application/pdf\nmetadata_location: metadata-file\n---\n");
    }

    #[tokio::test]
    #[test_log::test]
    async fn pdf_with_text_and_metadata() {
        let dir = TempTree::new(fs_tree! {
            "doc.pdf" => "%PDF-1.4\n%âãÏÓ\n1 0 obj\n<< /Type /Catalog >>\nendobj\ntrailer\n<< /Root 1 0 R >>\n%%EOF",
            format!("doc.pdf.{}", METADATA_EXTENSION) => "---\nid: 123e4567-e89b-12d3-a456-426614174000\nmime: application/pdf\nmetadata_location: metadata-file\n---\nHello world",
        }).await.unwrap();
        let ws = Workspace::open(&dir).await.unwrap();
        
        // Load document
        let mut doc = Document::open(ws.clone(), &dir.join("doc.pdf")).await.unwrap();
        assert_eq!(doc.path(), "doc.pdf");
        assert_eq!(doc.text().await.unwrap().export(&mut None).as_deref(), Some("Hello world"));
        assert_eq!(doc.metadata.id, Uuid::parse_str("123e4567-e89b-12d3-a456-426614174000").unwrap());

        // Save document
        doc.save().await.unwrap();

        // File(s) must be unchanged
        let content = fs::read_to_string(&dir.join("doc.pdf")).await.unwrap();
        assert_eq!(content, "%PDF-1.4\n%âãÏÓ\n1 0 obj\n<< /Type /Catalog >>\nendobj\ntrailer\n<< /Root 1 0 R >>\n%%EOF");
        let metadata = fs::read_to_string(&dir.join(format!("doc.pdf.{}", METADATA_EXTENSION))).await.unwrap();
        assert_eq!(metadata, "---\nid: 123e4567-e89b-12d3-a456-426614174000\nmime: application/pdf\nmetadata_location: metadata-file\n---\nHello world");
    }
}
