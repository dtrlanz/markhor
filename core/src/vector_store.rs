
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
        }
    }

    pub async fn retain_missing_docs(&self, docs: &mut Vec<Document>) {
        let data = self.data.read().await;
        let mut idx = 0;
        while idx < docs.len() {
            let doc_id = DocVersionId {
                id: docs[idx].id().clone(),
                hash: docs[idx].doc_hash().await,
                filter: PrelimFilter {},    // dummy value
            };
            if data.contains_doc(&doc_id) {
                idx += 1;
            } else {
                docs.remove(idx);
            }
        }
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

impl DocVersionId {
    pub async fn from_document(document: &Document) -> Self {
        DocVersionId {
            id: document.id().clone(),
            hash: document.doc_hash().await,
            filter: PrelimFilter::from(document),
        }
    }
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

#[derive(Debug, Clone)]
struct ChunkEntry {
    idx: ChunkIdx,
    vector_idx: usize,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SearchResult {
    pub doc_id: Uuid,
    pub doc_hash: TextHash,
    pub chunk_idx: ChunkIdx,
    pub similarity: f32,
}


#[cfg(test)]
mod tests {
    use std::mem;

    use crate::embedding::test_utils::MockEmbedder;
    use crate::embedding::Embedder;

    use super::*;

    #[tokio::test]
    async fn vector_store() {
        // Simple example texts
        let text1 = vec![
            "The cat sat on the big mat.",
            "The dog sat on the big mat.",
        ];
        let text2 = vec![
            "The one bug is big and fat.",
            "The other bug is small and skinny.",
        ];

        // Generate embeddings
        let embedder = MockEmbedder::new(
                // Our 3-letter "anchor" words for predictable similarity
                vec!["the", "and", "cat", "dog", "bug", "big", "mat", "sat", "fat", "bad"]
        );
        let embs1 = embedder.embed(&text1).await.unwrap();
        let embs2 = embedder.embed(&text2).await.unwrap();
        let sample = embs1[0].clone();

        // Prepare data for insertion
        let vec1 = text1.iter()
            .map(|t| TextHash::from(t))
            .zip(embs1.into_iter()).enumerate()
            .map(|(i, (hash, emb))| (ChunkIdx::new("test", "text1", i), hash, emb))
            .collect();
        let vec2 = text2.iter()
            .map(|t| TextHash::from(t))
            .zip(embs2.into_iter()).enumerate()
            .map(|(i, (hash, emb))| (ChunkIdx::new("test", "text2", i), hash, emb))
            .collect();

        // Create channel and send documents
        let (tx, rx) = tokio::sync::mpsc::channel(2);
        tx.send((DocVersionId {
            id: Uuid::new_v4(),
            hash: TextHash::from("doc1"),
            filter: PrelimFilter {},
        }, vec1)).await.unwrap();
        tx.send((DocVersionId {
            id: Uuid::new_v4(),
            hash: TextHash::from("doc2"),
            filter: PrelimFilter {},
        }, vec2)).await.unwrap();
        mem::drop(tx);  // Close the channel so the vector store knows when to stop waiting for more docs

        // Create vector store and insert documents
        let vector_store = VectorStore::new();
        vector_store.insert_docs(rx).await;

        // Verify counts
        let view = vector_store.all().await;
        assert_eq!(view.doc_count().await, 2);
        assert_eq!(view.chunk_count().await, 4);

        // Top-k search
        let results = view.top_k(sample.clone(), 4).await;
        assert_eq!(results.len(), 4);
        assert_eq!(results[0].doc_hash, TextHash::from("doc1"));
        assert_eq!(results[0].chunk_idx, ChunkIdx::new("test", "text1", 0));
        assert!((results[0].similarity - 1.0).abs() < 1e-6);  // Exact match
        assert_eq!(results[1].doc_hash, TextHash::from("doc1"));
        assert_eq!(results[1].chunk_idx, ChunkIdx::new("test", "text1", 1));
        assert!((results[1].similarity - 1.0).abs() > 0.1);  // Similar but not exact
        assert!((results[1].similarity - 1.0).abs() < 0.2);
        assert_eq!(results[2].doc_hash, TextHash::from("doc2"));
        assert_eq!(results[2].chunk_idx, ChunkIdx::new("test", "text2", 0));
        assert!((results[2].similarity - 1.0).abs() < 0.6);
        assert_eq!(results[3].doc_hash, TextHash::from("doc2"));
        assert_eq!(results[3].chunk_idx, ChunkIdx::new("test", "text2", 1));
        assert!((results[3].similarity - 1.0).abs() < 0.6);

        assert_eq!(&results[0..1], &view.top_k(sample.clone(), 1).await);
        assert_eq!(&results[0..2], &view.top_k(sample.clone(), 2).await);
        assert_eq!(&results[0..3], &view.top_k(sample.clone(), 3).await);
        assert_eq!(&results, &view.top_k(sample.clone(), 5).await);
    }
}