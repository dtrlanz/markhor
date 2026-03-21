
use std::{collections::{HashMap}, hash::{Hash, Hasher}, sync::Arc};

use tokio::sync::{RwLock, mpsc::Receiver};
use uuid::Uuid;

use crate::{embedding::Embedding, storage2::{ChunkIdx, Document, PrelimFilter, TextHash}};

#[derive(Debug, Default, Clone)]
pub struct VectorStore {
    data: Arc<RwLock<VectorData>>,
}

impl VectorStore {
    pub fn new() -> Self {
        Self::default()
    }

    pub async fn insert_docs(&self, mut docs: Receiver<(DocVersionId, Vec<(ChunkIdx, TextHash, Embedding)>)>) {
        // Insert vectors as they come in. Write in bursts to reduce contention on the write lock.
        let mut buffer = Vec::new();
        while let Some((doc, chunks)) = docs.recv().await {
            buffer.push((doc, chunks));
            // Acquire write lock
            let mut data = self.data.write().await;
            // Check if further vectors are ready, and if so add them to the buffer
            while let Ok(chunk) = docs.try_recv() {
                buffer.push(chunk);
            }
            // Insert all buffered vectors into the store
            for (doc, chunks) in buffer.drain(..) {
                data.insert_doc(doc, chunks.into_iter());
            }
            // Release write lock
            drop(data);
        }
    }

    pub async fn retain_missing_docs(&self, docs: &mut Vec<Document>) {
        let data = self.data.read().await;
        docs.retain(|doc| {
            let doc_id = DocVersionId {
                id: doc.id().clone(),
                hash: doc.doc_hash(),
                filter: PrelimFilter {},    // dummy value
            };
            !data.contains_doc(&doc_id)
        });
    }

    // pub async fn include_docs<I: Iterator<Item = DocVersionId>, F: AsyncFn(&DocVersionId) -> Vec<(ChunkIdx, TextHash, Embedding)>>(&self, docs: I, chunk_fn: F) {
    //     // create list of documents not already included
    //     let data = self.data.read().await;
    //     let mut to_include = Vec::new();
    //     for doc in docs {
    //         if !data.contains_doc(&doc) {
    //             to_include.push(doc);
    //         }
    //     }
    //     std::mem::drop(data);

    //     // Retrieve/generate vectors concurrently, sending them through shared channel
    //     let (tx, mut rx) = tokio::sync::mpsc::channel(100);
    //     let chunk_fn = Arc::new(chunk_fn);
    //     for doc in to_include {
    //         let tx = tx.clone();
    //         tokio::spawn(async move {
    //             let chunks = chunk_fn(&doc).await;
    //             if let Err(e) = tx.send((doc, chunks)).await {
    //                 eprintln!("Failed to send chunks for doc {}: {}", doc.id, e);
    //             }
    //         });
    //     }

    //     // Insert vectors as they come in. Write in bursts to avoid too much contention on the 
    //     // write lock.
    //     let mut buffer = Vec::new();
    //     while let Some((doc, chunks)) = rx.recv().await {
    //         buffer.push((doc, chunks));
    //         // Acquire write lock
    //         let mut data = self.data.write().await;
    //         // Check if further vectors are ready, and if so add them to the buffer
    //         while let Ok(chunk) = rx.try_recv() {
    //             buffer.push(chunk);
    //         }
    //         // Insert all buffered vectors into the store
    //         for (doc, chunks) in buffer.drain(..) {
    //             data.insert_doc(doc, chunks.into_iter());
    //         }
    //         // Release write lock
    //         drop(data);
    //     }

    // }

    // pub async fn include_doc<I: Iterator<Item = (ChunkIdx, TextHash, Embedding)>>(&self, doc: DocEntry, chunks: I) -> bool {
    //     let data = self.data.read().await;
    //     if data.contains_doc(&doc) {
    //         return false;
    //     }
    //     std::mem::drop(data);
    //     let mut data = self.data.write().await;
    //     data.insert_doc(doc, chunks);
    //     true
    // }

    pub async fn all(&self) -> VectorView {
        let data = self.data.read().await;
        let docs = data.docs.iter().map(|(doc, idx)| (doc.id, doc.hash, *idx)).collect();
        VectorView { 
            data: self.data.clone(), 
            docs, 
            min_threshold: 0.4 
        }
    }

    pub async fn filter<F: AsyncFn(&DocVersionId) -> bool>(&self, filter_fn: F) -> VectorView {
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

    pub async fn select<I: IntoIterator<Item = (Uuid, TextHash)>>(&self, doc_ids: I) -> Result<VectorView, Vec<(Uuid, TextHash)>> {
        let data = self.data.read().await;
        let mut results = Vec::new();
        let mut missing = Vec::new();
        for (id, hash) in doc_ids {
            let doc_id = DocVersionId {
                id,
                hash,
                filter: PrelimFilter {},    // dummy value
            };
            if let Some(idx) = data.docs.get(&doc_id) {
                results.push((id, hash, *idx));
            } else {
                missing.push((id, hash));
            }
        }
        if missing.is_empty() {
            Ok(VectorView { 
                data: self.data.clone(), 
                docs: results, 
                min_threshold: 0.4 
            })
        } else {
            Err(missing)
        }
    }
}

#[derive(Debug, Clone, Default)]
struct VectorData {
    docs: HashMap<DocVersionId, usize>,
    doc_chunks: Vec<Vec<ChunkEntry>>,
    vectors: Vec<Embedding>,
    vector_hashes: HashMap<TextHash, usize>,
}

impl VectorData {
    fn contains_doc(&self, doc: &DocVersionId) -> bool {
        self.docs.contains_key(doc)
    }

    fn insert_doc<I: Iterator<Item = (ChunkIdx, TextHash, Embedding)>>(&mut self, doc: DocVersionId, chunks: I) {
        let chunks = chunks.map(|(idx, vector_hash, vector)| {
            let vector_idx = self.insert_vector(vector_hash, vector);
            ChunkEntry { idx, vector_idx }
        }).collect();
        let doc_idx = self.doc_chunks.len();
        self.doc_chunks.push(chunks);
        self.docs.insert(doc, doc_idx);
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
pub struct DocVersionId {
    // Uniquely identifies document.
    pub id: Uuid,
    // Uniquely identifies version of document. This important when the vector store contains 
    // multiple versions of the same document.
    pub hash: TextHash,
    // Useful for filtering by scope.
    pub filter: PrelimFilter,
}

impl PartialEq for DocVersionId {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id && self.hash == other.hash
    }
}

impl Eq for DocVersionId {}

impl Hash for DocVersionId {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.id.hash(state);
        self.hash.hash(state);
    }
}

impl From<&Document> for DocVersionId {
    fn from(document: &Document) -> Self {
        DocVersionId {
            id: document.id().clone(),
            hash: document.doc_hash(),
            filter: PrelimFilter::from(document),
        }
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
