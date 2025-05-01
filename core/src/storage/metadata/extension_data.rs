use std::ops::Range;

use serde::{Deserialize, Serialize};

use crate::{chunking::ChunkData, embedding::Embedding, extension::FunctionalityId};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ExtensionData {
    Embeddings(Vec<(Embedding, ChunkData)>),
}
