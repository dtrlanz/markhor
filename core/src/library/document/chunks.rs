use std::collections::{HashMap, hash_map::Entry};

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::{embedding::Embedding, library::{HashValue, document::text::Part}};

pub struct Chunks<'a> {
    pub(crate) chunker_id: String,
    pub(crate) text_parts: &'a [Part],
    pub(crate) data: &'a HashMap<String, Vec<ChunkCache>>,
    pub(crate) chunk_idx: usize,
}

impl<'a> Iterator for Chunks<'a> {
    type Item = Chunk<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.text_parts.is_empty() {
            return None;
        }
        let part = &self.text_parts[0];
        let text_id = part.id();
        let chunks = self.data.get(text_id)?;
        if self.chunk_idx >= chunks.len() {
            self.text_parts = &self.text_parts[1..];
            self.chunk_idx = 0;
            return self.next();
        }
        let chunk_data = &chunks[self.chunk_idx];
        let chunk = Chunk {
            data: chunk_data,
            text: part.as_str(),
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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct ChunkCache {
    #[serde(flatten)]
    pub(crate) chunk: crate::chunking::ChunkData,
    pub(crate) hash: HashValue,
    pub(crate) embeddings: HashMap<String, Embedding>,
}

#[derive(Debug, Clone)]
pub struct Chunk<'a> {
    pub(crate) data: &'a ChunkCache,
    pub(crate) text: &'a str,
    pub(crate) idx: ChunkIdx,
}

impl<'a> Chunk<'a> {
    pub fn text(&self) -> &'a str {
        &self.text[self.data.chunk.text_range.clone()]
    }

    pub fn hash(&self) -> &HashValue {
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
    pub(crate) data: &'a mut ChunkCache,
    pub(crate) text: &'a str,
    pub(crate) idx: ChunkIdx,
}

impl<'a> ChunkMut<'a> {
    pub fn text(&self) -> &'a str {
        &self.text[self.data.chunk.text_range.clone()]
    }

    pub fn hash(&self) -> &HashValue {
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
    pub(crate) chunker_id: String,
    pub(crate) text_part_id: String,
    pub(crate) chunk_idx: usize,
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
