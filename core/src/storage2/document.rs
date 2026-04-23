use mime::Mime;
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use sha2::{Digest, Sha256, digest::{OutputSizeUser, generic_array::GenericArray}};
use thiserror::Error;
use tokio::io::{AsyncWriteExt};
use uuid::Uuid;
use tokio::fs::{self, OpenOptions};
use tracing::{debug, error, info, instrument, trace, warn};
use std::{borrow::Borrow, collections::{HashMap, hash_map::Entry}, ffi::OsStr, path::{Path, PathBuf}, sync::Mutex};

use crate::{chunking::{Chunker, ChunkerError}, embedding::Embedding, extension::F11y, markdown::{ToMarkdown, WITH_MILESTONES, WITHOUT_XML}, storage2::{ATTACHMENTS_DIR, AccessStorageError, METADATA_EXTENSION, Tag, Workspace}};


#[derive(Debug)]
pub struct Document {
    /// Absolute path to the source file
    pub(crate) absolute_path: PathBuf,

    /// Workspace owning this document
    workspace: Workspace,
    metadata: DocumentMetadata,
    text_parts: Vec<(String, String)>,
    text_hash: Mutex<Option<TextHash>>,
    doc_hash: Mutex<Option<TextHash>>,
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

    pub fn text(&self) -> Option<String> {
        if self.text_parts.is_empty() {
            None
        } else if self.text_parts.len() == 1 && self.text_parts[0].0.is_empty() {
            Some(self.text_parts[0].1.clone())
        } else {
            let mut text = String::new();
            for (id, part) in self.text_parts.iter() {
                text.push_str(part.to_markdown(WITHOUT_XML)
                    .prepend_milestone("part".into(), id, vec![]).as_ref());
                text.push_str("\n");
            }
            Some(text)
        }
    }

    fn metadata_block(&self) -> Result<String, serde_yaml_ng::Error> {
        let yaml = serde_yaml_ng::to_string(&self.metadata)?;
        Ok(format!("---\n{}---\n", yaml))
    }

    pub fn text_parts(&self) -> impl Iterator<Item = (&str, &str)> {
        self.text_parts.iter().map(|(id, text)| (id.as_str(), text.as_str()))
    }

    pub fn text_parts_mut(&mut self) -> impl Iterator<Item = (&str, &mut String)> {
        // Content may change, invalidate text hash
        self.text_hash.get_mut().unwrap().take();
        self.doc_hash.get_mut().unwrap().take();
        self.text_parts.iter_mut().map(|(id, text)| (id.as_str(), text))
    }

    fn text_hash(&self) -> TextHash {
        let mut text_hash = self.text_hash.lock().unwrap();
        if text_hash.is_none() {
            let text_iter = self.text_parts.iter()
                .map(|(id, text)| [&**id, &**text].into_iter())
                .flatten();
            let r = TextHash::from_iter(text_iter);
            *text_hash = Some(r);
            r
        } else {
            text_hash.unwrap()
        }
    }

    pub fn doc_hash(&self) -> TextHash {
        let mut doc_hash = self.doc_hash.lock().unwrap();
        if doc_hash.is_none() {
            // hash of text and metadata
            let text_hash = self.text_hash();
            let mut hasher = sha2::Sha256::new();
            hasher.update(text_hash.value);
            hasher.update(serde_yaml_ng::to_string(&self.metadata).unwrap());
            let r = TextHash { value: hasher.finalize() };
            *doc_hash = Some(r);
            r
        } else {
            doc_hash.unwrap()
        }
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
            let text_part_ids: Vec<_> = self.text_parts.iter().map(|(id, _)| id).collect();
            chunker_cache.retain(|text_id, _| text_part_ids.contains(&text_id));

            // Re-chunk the document
            for (id, text) in self.text_parts.iter() {
                let cached_chunks = chunker_cache.entry(id.clone()).or_default();
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

        // Create iterator over all chunks with text parts
        let text_parts = &self.text_parts;
        let with_text_part = self.chunker_cache.get_mut(&chunker_id).unwrap()
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

    #[instrument(skip(self), level = "trace", err)]
    pub(crate) fn chunk(&self, idx: ChunkIdx) -> Result<Chunk<'_>, GetChunkError> {
        let text_part = self.text_parts.iter().find(|(id, _)| id == &idx.text_part_id)
            .ok_or_else(|| GetChunkError::NoSuchTextPart(idx.text_part_id.clone()))?;
        self.chunker_cache.get(&idx.chunker_id)
            .ok_or_else(|| GetChunkError::CacheNotLoaded(idx.chunker_id.clone()))?
            .get(&idx.text_part_id)
            .ok_or_else(|| GetChunkError::TextPartNotChunked(idx.chunker_id.clone(), idx.text_part_id.clone()))?
            .get(idx.chunk_idx)
            .ok_or_else(|| GetChunkError::ChunkIdxOutOfBounds(idx.chunker_id.clone(), idx.text_part_id.clone(), idx.chunk_idx))
            .and_then(|chunk_cache| {
                Ok(Chunk {
                    text: text_part.1.as_str(),
                    data: chunk_cache,
                    idx,
                })
            })
    }

    #[instrument(skip(self), level = "trace", err)]
    pub(crate) fn chunk_mut(&mut self, idx: ChunkIdx) -> Result<ChunkMut<'_>, GetChunkError> {
        let text_part = self.text_parts.iter().find(|(id, _)| id == &idx.text_part_id)
            .ok_or_else(|| GetChunkError::NoSuchTextPart(idx.text_part_id.clone()))?;
        self.chunker_cache.get_mut(&idx.chunker_id)
            .ok_or_else(|| GetChunkError::CacheNotLoaded(idx.chunker_id.clone()))?
            .get_mut(&idx.text_part_id)
            .ok_or_else(|| GetChunkError::TextPartNotChunked(idx.chunker_id.clone(), idx.text_part_id.clone()))?
            .get_mut(idx.chunk_idx)
            .ok_or_else(|| GetChunkError::ChunkIdxOutOfBounds(idx.chunker_id.clone(), idx.text_part_id.clone(), idx.chunk_idx))
            .and_then(|chunk_cache| {
                Ok(ChunkMut {
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
                    AccessStorageError::MetadataFormat(e)
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

    async fn chunker_cache(&self, chunker_id: &str) -> Result<&HashMap<String, Vec<ChunkCache>>, AccessStorageError> {
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
    pub(crate) async fn chunker_cache_mut(&mut self, chunker_id: &str) -> Result<&mut HashMap<String, Vec<ChunkCache>>, AccessStorageError> {
        let read_cache = self.read_cache_file(format!(".chunks_{}.yaml", chunker_id).as_str());
        Self::load_chunker_cache(
            &mut self.chunker_cache, 
            &chunker_id, 
            read_cache, 
            |e| Err(e),
        ).await
    }

    #[instrument(skip(chunker_cache, read_cache, handle_err), level = "debug", err)]
    async fn load_chunker_cache<'a, F: FnOnce(AccessStorageError) -> Result<HashMap<String, Vec<ChunkCache>>, AccessStorageError> + Sized>(
        chunker_cache: &'a mut HashMap<String, HashMap<String, Vec<ChunkCache>>>,
        chunker_id: &str,
        read_cache: impl Future<Output = Result<Option<HashMap<String, Vec<ChunkCache>>>, AccessStorageError>>,
        handle_err: F,
    ) -> Result<&'a mut HashMap<String, Vec<ChunkCache>>, AccessStorageError> {
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
    async fn write_cache_file<T: Serialize + ?Sized>(&self, name: &str, data: &T) -> Result<(), AccessStorageError> {
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

    #[instrument(skip(self), level = "debug", err)]
    async fn load(&mut self) -> Result<(), AccessStorageError> {
        // Attempt to load cache from cache file
        let read_cache = self.read_cache_file(".cache.yaml");
        let cache = tokio::spawn(async move {
            read_cache.await
        });

        self.update_text(None);

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
                                self.update_text(Some(text));
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
                        return Err(AccessStorageError::MetadataLocation(MetadataLocation::MetadataFile, "Metadata file not found".into()));
                    }
                    // Metadata file not found, try source file
                    debug!("Metadata file not found for document {:?}, trying source file", self.absolute_path);
                },
                Err(e) => {
                    return Err(AccessStorageError::Io(e));
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
                        return Err(AccessStorageError::MetadataFormat(e));
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
            self.update_text(Some(text));
        } else {
            self.metadata.metadata_location.update_unknown(MetadataLocation::None);
            if self.metadata.source_is_text() {
                // Read text from source file
                let text = fs::read_to_string(&self.absolute_path).await?;
                self.update_text(Some(text));
            }
        }

        self.cache = cache.await.unwrap().ok().flatten().unwrap_or_default();

        Ok(())
    }

    #[instrument(skip(self), fields(doc_path = %self.path().display()), err)]
    pub async fn save(&mut self) -> Result<(), AccessStorageError> {
        info!("Saving document {:?}", self.absolute_path);
        // Save text and metadata to appropriate location
        match (self.metadata.metadata_location) {
            MetadataLocation::SourceFile => {
                if !self.metadata.source_is_markdown() {
                    return Err(AccessStorageError::MetadataLocation(
                        self.metadata.metadata_location, 
                        "Cannot save metadata in source file that is not markdown".into(),
                    ));
                }
                // Save both in source file
                debug!("Saving text and metadata in source file for document {:?}", self.absolute_path);
                let mut content = self.metadata_block()?;
                content.push_str(&self.text().unwrap_or_default());
                fs::write(&self.absolute_path, content).await?;
            },
            MetadataLocation::MetadataFile => {
                if self.metadata.source_is_text() {
                    // Save to separate files
                    debug!("Saving text in source file and metadata in metadata file for document {:?}", self.absolute_path);
                    fs::write(&self.absolute_path, self.text().unwrap_or_default()).await?;
                    trace!("Metadata file path: {:?}", self.metadata_path());
                    fs::write(&self.metadata_path(), self.metadata_block()?).await?;
                } else {
                    // Source is not a text file. Save text in metadata file.
                    debug!("Saving text and metadata in metadata file for document {:?}", self.absolute_path);
                    let mut content = self.metadata_block()?;
                    if let Some(text) = self.text() {
                        content.push_str(&text);
                    }
                    fs::write(&self.metadata_path(), content).await?;
                }
            },
            MetadataLocation::None => {
                if let Some(text) = self.text() {
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
                return Err(AccessStorageError::MetadataLocation(MetadataLocation::Unknown, "Invalid when saving".into()));
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

    fn update_text(&mut self, new_text: Option<String>) {
        self.text_hash.get_mut().unwrap().take();
        self.doc_hash.get_mut().unwrap().take();
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

    let metadata = DocumentMetadata::from_path(&absolute_path);
    let mut doc = Document {
        absolute_path,
        workspace,
        metadata,
        text_parts: Vec::new(),
        text_hash: Default::default(),
        doc_hash: Default::default(),
        cache: Default::default(),
        chunker_cache: Default::default(),
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

// #[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
// #[serde(rename_all = "kebab-case")]
// pub enum TextLocation {
//     SourceFile,
//     MetadataFile,
//     Inferred,
//     None,
// }

// impl Default for TextLocation {
//     fn default() -> Self {
//         TextLocation::Inferred
//     }
// }

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

// Used in unit tests to create chunk indices without needing a chunker or document
#[cfg(test)]
impl ChunkIdx {
    pub(crate) fn new(chunker_id: impl Into<String>, text_part_id: impl Into<String>, chunk_idx: usize) -> Self {
        Self { chunker_id: chunker_id.into(), text_part_id: text_part_id.into(), chunk_idx }
    }
}

#[derive(Debug, Error)]
pub enum GetChunkError {
    #[error("No such text part in document: '{}'", .0)]
    NoSuchTextPart(String),   // text_part_id

    #[error("Cache not loaded for chunker '{}'", .0)]
    CacheNotLoaded(String),    // chunker_id
    
    #[error("Text part not found for chunker '{}' and text part '{}'", .0, .1)]
    TextPartNotChunked(String, String),   // chunker_id, text_part_id

    #[error("Chunk index {} out of bounds for chunker '{}' and text part '{}'", .2, .0, .1)]
    ChunkIdxOutOfBounds(String, String, usize),   // chunker_id, text_part_id, chunk_idx
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
        let metadata: DocumentMetadata = DocumentMetadata::new("text/markdown".parse().unwrap());
        let metadata_str = serde_yaml_ng::to_string(&metadata).unwrap();

        let dir = TempTree::new(fs_tree! {
            "doc.md" => "foo",
            "doc2.md" => { format!("---\n{}---\nfoo", &metadata_str) },
        }).await.unwrap();
        let ws = Workspace::open(&dir).await.unwrap();

        // Document without metadata
        let doc = open_document(&dir.join("doc.md"), ws.clone()).await.unwrap();
        assert_eq!(doc.path(), "doc.md");
        assert_eq!(doc.workspace(), &ws);
        assert_eq!(doc.text().as_deref(), Some("foo"));

        // 2nd time
        let doc = open_document(&dir.join("doc.md"), ws.clone()).await.unwrap();
        assert_eq!(doc.path(), "doc.md");
        assert_eq!(doc.workspace(), &ws);
        assert_eq!(doc.text().as_deref(), Some("foo"));

        // Document with metadata
        let doc = open_document(&dir.join("doc2.md"), ws.clone()).await.unwrap();
        assert_eq!(doc.path(), "doc2.md");
        assert_eq!(doc.workspace(), &ws);
        assert_eq!(doc.metadata.id, metadata.id);
        assert_eq!(doc.text().as_deref(), Some("foo"));
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

    #[tokio::test]
    async fn doc_text_hash() {
        let dir = TempTree::new(fs_tree! {
            "doc.md" => "hello world",
        }).await.unwrap();
        let ws = Workspace::open(&dir).await.unwrap();
        let mut doc = open_document(&dir.join("doc.md"), ws.clone()).await.unwrap();

        let initial_text_hash = doc.text_hash();
        let initial_doc_hash = doc.doc_hash();

        // Update text and check that hashes change
        doc.text_parts_mut().next().unwrap().1.push_str("!");
        let updated_text_hash = doc.text_hash();
        let updated_doc_hash = doc.doc_hash();

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
        let mut doc = open_document(&dir.join("doc.md"), ws.clone()).await.unwrap();

        // Set locations
        doc.metadata.metadata_location = MetadataLocation::SourceFile;
        // Set text
        doc.update_text(Some("Hello world".to_string()));

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
        let mut doc = open_document(&dir.join("doc.md"), ws.clone()).await.unwrap();

        // Set locations
        doc.metadata.metadata_location = MetadataLocation::MetadataFile;
        // Set text
        doc.update_text(Some("Hello world".to_string()));

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
        let mut doc = open_document(&dir.join("doc.pdf"), ws.clone()).await.unwrap();

        // Set locations
        doc.metadata.metadata_location = MetadataLocation::MetadataFile;
        // Set text
        doc.update_text(Some("Hello world".to_string()));

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
        let mut doc = open_document(&dir.join("doc.md"), ws.clone()).await.unwrap();

        // Update cache
        doc.cache.extensions.insert("foo".to_string(), ExtensionCache {
            hash: Some(TextHash::from("hello world")),
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
        let mut doc = open_document(&dir.join("doc.md"), ws.clone()).await.unwrap();

        // Create chunk cache for a chunker
        let chunker_id = "test_chunker".to_string();
        let text_part_id = String::new();
        doc.chunker_cache.insert(chunker_id.clone(), HashMap::from([
            (text_part_id.clone(), vec![
                ChunkCache {
                    chunk: crate::chunking::ChunkData { text_range: 0..5, heading_path: None, token_count: None },
                    hash: TextHash::from("hello"),
                    embeddings: HashMap::new(),
                },
                ChunkCache {
                    chunk: crate::chunking::ChunkData { text_range: 5..11, heading_path: None, token_count: None },
                    hash: TextHash::from(" world"),
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
        let mut doc = open_document(&dir.join("doc.md"), ws.clone()).await.unwrap();

        // Ensure metadata is saved
        doc.metadata.metadata_location = MetadataLocation::SourceFile;

        // Set metadata to have multiple text parts
        doc.metadata.doc_parts = Some("not-none".to_string());
        // Set text parts
        doc.text_parts = vec![
            ("part1".to_string(), "Hello".to_string()),
            ("part2".to_string(), "world".to_string()),
        ];
        println!("Text: {:?}", doc.text());

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
        let mut doc = open_document(&dir.join("doc.md"), ws.clone()).await.unwrap();
        assert_eq!(doc.path(), "doc.md");
        assert_eq!(doc.text().as_deref(), Some("hello world"));

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
        let mut doc = open_document(&dir.join("doc.md"), ws.clone()).await.unwrap();
        assert_eq!(doc.path(), "doc.md");
        assert_eq!(doc.text().as_deref(), Some("hello world"));
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
        let mut doc = open_document(&dir.join("doc.md"), ws.clone()).await.unwrap();
        assert_eq!(doc.path(), "doc.md");
        assert_eq!(doc.text().as_deref(), Some("hello world"));
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
        let mut doc = open_document(&dir.join("doc.csv"), ws.clone()).await.unwrap();
        assert_eq!(doc.path(), "doc.csv");
        assert_eq!(doc.text().as_deref(), Some("a,b,c\n1,2,3"));

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
        let mut doc = open_document(&dir.join("doc.csv"), ws.clone()).await.unwrap();
        assert_eq!(doc.path(), "doc.csv");
        assert_eq!(doc.text().as_deref(), Some("a,b,c\n1,2,3"));
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
        let mut doc = open_document(&dir.join("doc.pdf"), ws.clone()).await.unwrap();
        assert_eq!(doc.path(), "doc.pdf");
        assert_eq!(doc.text(), None);

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
        let mut doc = open_document(&dir.join("doc.pdf"), ws.clone()).await.unwrap();
        assert_eq!(doc.path(), "doc.pdf");
        assert_eq!(doc.text(), None);
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
        let mut doc = open_document(&dir.join("doc.pdf"), ws.clone()).await.unwrap();
        assert_eq!(doc.path(), "doc.pdf");
        assert_eq!(doc.text().as_deref(), Some("Hello world"));
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
