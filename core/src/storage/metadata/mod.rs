use crate::{embedding::{Embedder, Embedding}, extension::{Functionality, FunctionalityId}};

use std::{collections::{hash_map::Entry, HashMap}, ops::Range};

use clap::crate_version;
use extension_data::ExtensionData;
use serde::{Deserialize, Serialize};
use tracing::error;
use uuid::Uuid;

mod extension_data;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct DocumentMetadata {
    markhor_version: String,
    pub(crate) id: Uuid,
    
    files: HashMap<String, FileMetadata>,
    
}

impl DocumentMetadata {
    pub fn new() -> Self {
        DocumentMetadata { 
            markhor_version: crate_version!().to_string(),
            id: Uuid::new_v4(),
            files: HashMap::new(),
        }
    }

    pub fn file(&self, filename: &str) -> Option<&FileMetadata> {
        self.files.get(filename)
    }

    pub fn file_mut(&mut self, filename: &str) -> &mut FileMetadata {
        self.files.entry(filename.to_string()).or_default()
    }
}


#[derive(Debug, Clone, Serialize, Deserialize)]
struct ChunkIdx(usize);

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct FileMetadata {
    extension_data: HashMap<FunctionalityId, ExtensionData>,
}

impl FileMetadata {
    pub fn embeddings(&self, embedder: &(impl Embedder + ?Sized)) -> Option<&Vec<(Embedding, Range<usize>)>> {
        let data = self.extension_data.get(&embedder.into());
        match data {
            Some(ExtensionData::Embeddings(embeddings)) => Some(embeddings),
            None => None,
            Some(unexpected_data) => {
                error!("Found unexpected extension data for file. Expected embeddings, got: {:?}", unexpected_data);
                None
            }
        }
    }

    pub fn embeddings_mut(&mut self, embedder: &(impl Embedder + ?Sized)) -> &mut Vec<(Embedding, Range<usize>)> {
        let data = self.extension_data.entry(embedder.into())
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