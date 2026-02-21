
use std::{collections::{HashMap}, hash::{Hash, Hasher}, sync::Arc};

use tokio::{sync::{RwLock}};
use uuid::Uuid;

use crate::{embedding::Embedding, storage2::{ChunkIdx, PrelimFilter, TextHash}};

#[derive(Debug, Default)]
pub struct VectorStore {
    data: Arc<RwLock<VectorData>>,
}

impl VectorStore {
    pub fn new() -> Self {
        Self::default()
    }

    pub async fn insert_doc<I: Iterator<Item = (ChunkIdx, TextHash, Embedding)>>(&self, id: Uuid, hash: TextHash, filter: PrelimFilter, chunks: I) {
        let mut data = self.data.write().await;
        data.insert_doc(id, hash, filter, chunks);
    }

    pub async fn all(&self) -> VectorView {
        let data = self.data.read().await;
        let docs = data.docs.iter().map(|(doc, idx)| (doc.id, doc.hash, *idx)).collect();
        VectorView { 
            data: self.data.clone(), 
            docs, 
            min_threshold: 0.4 
        }
    }

    pub async fn filter<F: AsyncFn(&DocEntry) -> bool>(&self, filter_fn: F) -> VectorView {
        let data = self.data.read().await;
        let mut results = Vec::new();
        // TODO: filter concurrently
        for (doc, idx) in data.docs.iter() {
            if filter_fn(doc).await {
                results.push((doc.id, doc.hash, *idx));
            }
        }
        VectorView { 
            data: self.data.clone(), 
            docs: results, 
            min_threshold: 0.4 
        }
    }
}

#[derive(Debug, Clone, Default)]
struct VectorData {
    docs: HashMap<DocEntry, usize>,
    doc_chunks: Vec<Vec<ChunkEntry>>,
    vectors: Vec<Embedding>,
    vector_hashes: HashMap<TextHash, usize>,
}

impl VectorData {
    fn insert_doc<I: Iterator<Item = (ChunkIdx, TextHash, Embedding)>>(&mut self, id: Uuid, hash: TextHash, filter: PrelimFilter, chunks: I) {
        let chunks = chunks.map(|(idx, vector_hash, vector)| {
            let vector_idx = self.insert_vector(vector_hash, vector);
            ChunkEntry { idx, vector_idx }
        }).collect();
        let doc_idx = self.doc_chunks.len();
        self.doc_chunks.push(chunks);
        self.docs.insert(DocEntry { id, hash, filter }, doc_idx);
    }

    fn insert_vector(&mut self, hash: TextHash, vector: Embedding) -> usize {
        if let Some(idx) = self.vector_hashes.get(&hash) {
            return *idx;
        }
        let idx = self.vectors.len();
        self.vectors.push(vector);
        self.vector_hashes.insert(hash, idx);
        idx
    }
}

pub struct VectorView {
    data: Arc<RwLock<VectorData>>,
    docs: Vec<(Uuid, TextHash, usize)>,
    min_threshold: f32,
}

impl VectorView {
    pub async fn top_k(&self, query_vector: Embedding, k: usize) -> Vec<SearchResult> {
        // lots of ways to improve efficiency here, but this is the simplest to implement for now
        let data = self.data.read().await;
        let mut results = Vec::new();
        for (doc_id, doc_hash, doc_idx) in self.docs.iter() {
            for chunk in data.doc_chunks[*doc_idx].iter() {
                let vector = &data.vectors[chunk.vector_idx];
                let similarity = query_vector.similarity(vector).unwrap();
                if similarity >= self.min_threshold {
                    results.push(SearchResult {
                        doc_id: *doc_id,
                        doc_hash: *doc_hash,
                        chunk_idx: chunk.idx.clone(),
                        similarity,
                    });
                }
            }
        }
        results.sort_by(|a, b| b.similarity.partial_cmp(&a.similarity).unwrap());
        results.truncate(k);
        results
    }

    pub async fn top_percent(&self, query_vector: Embedding, percent: f32) -> Vec<SearchResult> {
        let k = ((self.chunk_count().await as f32) * (percent / 100.0)).ceil() as usize;
        self.top_k(query_vector, k).await
    }

    pub async fn doc_count(&self) -> usize {
        self.docs.len()
    }

    pub async fn chunk_count(&self) -> usize {
        let data = self.data.read().await;
        let mut count = 0;
        for (_, _, doc_idx) in self.docs.iter() {
            count += data.doc_chunks[*doc_idx].len();
        }
        count
    }

    pub fn min_threshold(&self) -> f32 {
        self.min_threshold
    }

    pub fn set_min_threshold(&mut self, threshold: f32) -> &mut Self {
        self.min_threshold = threshold;
        self
    }
}


#[derive(Debug, Clone)]
pub struct DocEntry {
    pub id: Uuid,
    pub hash: TextHash,
    pub filter: PrelimFilter,
}

impl PartialEq for DocEntry {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id && self.hash == other.hash
    }
}

impl Eq for DocEntry {}

impl Hash for DocEntry {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.id.hash(state);
        self.hash.hash(state);
    }
}

#[derive(Debug, Clone)]
struct ChunkEntry {
    idx: ChunkIdx,
    vector_idx: usize,
}

#[derive(Debug, Clone)]
pub struct SearchResult {
    pub doc_id: Uuid,
    pub doc_hash: TextHash,
    pub chunk_idx: ChunkIdx,
    pub similarity: f32,
}
