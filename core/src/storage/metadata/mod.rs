use crate::{chunking::ChunkData, embedding::{Embedder, Embedding}, extension::F11y};

use std::{collections::{hash_map::Entry, HashMap}, ops::Range};

use clap::crate_version;
use extension_data::ExtensionData;
use serde::{Deserialize, Serialize};
use tracing::error;
use uuid::Uuid;

mod extension_data;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DocumentMetadata {
    markhor_version: String,
    pub(crate) id: Uuid,
    
    files: HashMap<String, FileMetadata>,
}

impl DocumentMetadata {
    /// Creates a new `DocumentMetadata` instance with a unique ID and the current version of 
    /// Markhor.
    pub fn new() -> Self {
        DocumentMetadata { 
            markhor_version: crate_version!().to_string(),
            id: Uuid::new_v4(),
            files: HashMap::new(),
        }
    }

    pub fn markhor_version(&self) -> &str {
        &self.markhor_version
    }

    /// Returns the metadata for the file with the given filename, or None if it doesn't exist.
    pub fn file(&self, filename: &str) -> Option<&FileMetadata> {
        self.files.get(filename)
    }

    /// Returns the metadata for the file with the given filename, creating it if it doesn't exist.
    pub fn file_mut(&mut self, filename: &str) -> &mut FileMetadata {
        self.files.entry(filename.to_string()).or_default()
    }

    /// Returns the names of files for which metadata is available.
    pub fn files_with_metadata(&self) -> impl Iterator<Item = &str> {
        self.files.keys().map(|s| s.as_str())
    }
}


// #[derive(Debug, Clone, Serialize, Deserialize)]
// struct ChunkIdx(usize);

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct FileMetadata {
    extension_data: HashMap<String, ExtensionData>,
}

impl FileMetadata {
    pub fn embeddings(&self, embedder: &F11y<dyn Embedder>) -> Option<&Vec<(Embedding, ChunkData)>> {
        let data = self.extension_data.get(&embedder.metadata_id());
        match data {
            Some(ExtensionData::Embeddings(embeddings)) => Some(embeddings),
            None => None,
            Some(unexpected_data) => {
                error!("Found unexpected extension data for file. Expected embeddings, got: {:?}", unexpected_data);
                None
            }
        }
    }

    pub fn embeddings_mut(&mut self, embedder: &F11y<dyn Embedder>) -> &mut Vec<(Embedding, ChunkData)> {
        let data = self.extension_data.entry(embedder.metadata_id())
            .and_modify(|data| match data {
                ExtensionData::Embeddings(_) => {},
                _ => {
                    error!("Found unexpected extension data for file. Expected embeddings, got: {:?}", data);
                    *data = ExtensionData::Embeddings(Vec::new());
                }
            })
            .or_insert_with(|| ExtensionData::Embeddings(Vec::new()));
            
        match data {
            ExtensionData::Embeddings(embeddings) => embeddings,
            _ => unreachable!(),
        }
    }
}