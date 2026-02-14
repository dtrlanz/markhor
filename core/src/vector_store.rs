
use uuid::Uuid;

use crate::{embedding::Embedding, storage2::{ChunkIdx, PrelimFilter, TextHash}};

pub struct VectorStore {
    
}

struct VectorData {
    docs: Vec<DocEntry>,
}

struct DocEntry {
    id: Uuid,
    hash: TextHash,
    filter: PrelimFilter,
    chunk_indices: Vec<ChunkEntry>,
}

struct ChunkEntry {
    idx: ChunkIdx,
    vector: Embedding,
}