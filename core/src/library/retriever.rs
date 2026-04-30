use std::{collections::{HashMap, HashSet}, mem, path::PathBuf, sync::Arc};

use thiserror::Error;
use tracing::{instrument, debug, trace};
use uuid::Uuid;

use crate::{chunking::{Chunker, ChunkerError}, embedding::{Embedder, Embedding, EmbeddingError}, extension::F11y, library::{AccessStorageError, ChunkIdx, Document, Scope, HashValue, Workspace}, vector_store::{DocVersionId, VectorView}};




pub struct Retriever {
    workspace: Workspace,
    scope: Scope,
    docs: HashMap<Uuid, IncludedDoc>,
    chunker: F11y<dyn Chunker>,
    embedder: F11y<dyn Embedder>,
    embeddings: VectorView,
}

impl Retriever {
    #[instrument(skip(chunker, embedder))]
    pub async fn new(workspace: Workspace, scope: Scope, chunker: F11y<dyn Chunker>, embedder: F11y<dyn Embedder>) -> Result<Self, InitRetrieverError> {
        debug!("Initializing retriever for scope {:?}", scope);
        let vector_store = workspace.vector_store(&embedder);
        let utils = Arc::new((chunker, embedder));

        // Collect complete list of documents and identify which are not yet included in the 
        // vector store.
        let mut loaded_docs = Vec::new();
        let mut doc_map = HashMap::new();
        let mut doc_stream = scope.docs(&workspace).await?;
        while let Some(doc) = doc_stream.next_doc().await.unwrap() {
            trace!("Found doc {} with hash {:?}", doc.path().display(), doc.doc_hash().await);
            doc_map.insert(*doc.id(), IncludedDoc::OnDisk(doc.doc_hash().await?, doc.path().to_path_buf()));
            loaded_docs.push(doc);
        }
        
        let mut doc_ids = Vec::new();
        for doc in &loaded_docs {
            doc_ids.push((*doc.id(), doc.doc_hash().await?));
        }

        let view = match vector_store.select(doc_ids).await {
            Ok(view) => {
                debug!("All documents found in vector store");
                view
            },
            Err(missing_vec) => {
                // Retrieve/generate vectors concurrently
                let missing_set = missing_vec.into_iter().collect::<HashSet<_>>();
                let mut doc_ids = Vec::new();
                let (tx, rx) = tokio::sync::mpsc::channel(100);
                let mut tasks = Vec::new();
                for doc in loaded_docs {
                    doc_ids.push((*doc.id(), doc.doc_hash().await?));
                    if missing_set.contains(&(*doc.id(), doc.doc_hash().await?)) {
                        debug!("Doc {} is missing from vector store, generating vectors", doc.path().display());
                        let task = tokio::spawn(
                            Self::send_doc_vectors(doc, utils.clone(), tx.clone())
                        );
                        tasks.push(task);
                    }
                }

                mem::drop(tx);  // Close the channel so the vector store knows when to stop waiting for more docs

                // Insert vectors into vector store
                vector_store.insert_docs(rx).await;

                // Check for embedding errors
                for task in tasks {
                    task.await.unwrap()?;
                }
                debug!("All missing document vectors generated and inserted into vector store");

                // TODO: consider adjusting type or signature to reduce monomorphization
                vector_store.select(doc_ids).await.unwrap()
            },
        };

        let (chunker, embedder) = Arc::into_inner(utils).unwrap();

        Ok(Self {
            workspace,
            scope,
            docs: doc_map,
            chunker,
            embedder,
            embeddings: view,
        })
    }

    #[instrument(skip_all, fields(doc_path = %doc.path().display()), level = "debug", err)]
    async fn send_doc_vectors(mut doc: Document, utils: Arc<(F11y<dyn Chunker>, F11y<dyn Embedder>)>, sender: tokio::sync::mpsc::Sender<(DocVersionId, Vec<(ChunkIdx, HashValue, Embedding)>)>) -> Result<(), InitRetrieverError> {
        let doc_path = doc.path().display().to_string();
        debug!("Processing doc {} for embedding generation", doc_path);
        let chunker = &utils.0;
        let embedder = &utils.1;

        let id = DocVersionId::from_document(&doc).await?;
        let chunks = doc.chunks_mut(chunker).await?;
        let mut vectors = Vec::new();
        for mut chunk in chunks {
            if let Some(emb) = chunk.embedding(&embedder.metadata_id()) {
                trace!("Found embedding for chunk {:?} in doc {}, reusing", chunk.chunk_idx(), doc_path);
                vectors.push((chunk.chunk_idx().clone(), *chunk.hash(), emb.clone()));
                continue;
            }
            trace!("Generating embedding for chunk {:?} in doc {}", chunk.chunk_idx(), doc_path);
            let mut embs = embedder.embed(&[chunk.text()]).await?;
            let emb = embs.pop().unwrap();
            chunk.embedding_entry(embedder.metadata_id()).insert_entry(emb.clone());
            vectors.push((chunk.chunk_idx().clone(), *chunk.hash(), emb));
        }
        sender.send((id, vectors)).await.unwrap();
        // Save document with new embeddings so they can be loaded from disk later
        doc.save().await?;
        Ok(())
    }

    pub async fn top_k(&self, query: Embedding, k: usize) -> Result<Vec<(Arc<Document>, ChunkIdx, f32)>, AccessStorageError> {
        let results = self.embeddings.top_k(query, k).await;
        // TODO: load docs concurrently
        let mut output = Vec::new();
        for r in results {
            let doc = match self.docs.get(&r.doc_id).unwrap() {
                IncludedDoc::InMemory(doc) => {
                    assert_eq!(doc.doc_hash().await?, r.doc_hash);
                    doc.clone()
                },
                IncludedDoc::OnDisk(hash, path) => {
                    assert_eq!(hash, &r.doc_hash);
                    trace!("Loading doc {} from disk for top-k result", path.display());
                    let mut doc = self.workspace.document(path).await?;
                    doc.chunker_cache_mut(&self.chunker.metadata_id()).await?;
                    Arc::new(doc)
                },
            };
            output.push((doc, r.chunk_idx, r.similarity));
        }
        Ok(output)
    }
}


enum IncludedDoc {
    InMemory(Arc<Document>),
    OnDisk(HashValue, PathBuf),
}

#[derive(Debug, Error)]
pub enum InitRetrieverError {
    /// An error occurred while accessing the document or workspace storage.
    #[error("Storage error: {0}")]
    StorageError(#[from] AccessStorageError),

    /// An error occurred while chunking a document to prepare it for embedding.
    #[error("Chunker error: {0}")]
    ChunkerError(#[from] ChunkerError),

    /// An error occurred while generating or retrieving an embedding for a document chunk.
    #[error("Embedding error: {0}")]
    EmbeddingError(#[from] EmbeddingError),
}


#[cfg(test)]
mod tests {
    use super::*;
    use crate::embedding::test_utils::MockEmbedderExtension;
    use crate::{chunking::test_chunker::FixedSizeChunkerExtension, extension::ActiveExtension};
    use crate::library::fs_test_utils::{TempTree, fs_tree};

    #[tokio::test]
    async fn retriever_initialization() {
        let dir = TempTree::new(fs_tree! {
            //             /-- chunk 0 ---------------\/-- chunk 1 ---------------\/-- chunk 2 --\
            "doc1.txt" => "The cat sat on the big mat. The dog sat on the big mat.",
            "doc2.txt" => "The one bug is big and fat. The other bug is small and skinny.",
        }).await.unwrap();

        let ws = Workspace::open(&dir).await.unwrap();
        let scope = Scope::from(ws.root());

        let chunker = ActiveExtension::new(FixedSizeChunkerExtension::new(28), Default::default())
            .chunkers().next().unwrap();
        let embedder = ActiveExtension::new(
            MockEmbedderExtension::new(
                // Our 3-letter "anchor" words for predictable similarity
                vec!["the", "and", "cat", "dog", "bug", "big", "mat", "sat", "fat", "bad"]
            ), Default::default())
            .embedders().next().unwrap();
        let sample = embedder.embed(&["The cat sat on the big mat."]).await.unwrap().pop().unwrap();
        let retriever = Retriever::new(ws, scope, chunker, embedder).await.unwrap();

        // Verify that the retriever has initialized with the expected number of documents and chunks
        let view = retriever.embeddings;
        assert_eq!(view.doc_count().await, 2);
        assert_eq!(view.chunk_count().await, 5);

        // Top-k search
        let results = view.top_k(sample.clone(), 4).await;
        assert_eq!(results.len(), 4);
        assert!((results[0].similarity - 1.0).abs() < 1e-6);  // Exact match
        assert!((results[1].similarity - 1.0).abs() > 0.1);   // Similar but not exact
        assert!((results[1].similarity - 1.0).abs() < 0.2);
        assert!((results[2].similarity - 1.0).abs() < 0.6);   // Not very similar
        assert!((results[3].similarity - 1.0).abs() < 0.6);
    }

    #[tokio::test]
    #[test_log::test]
    async fn retriever_top_k() {
        let dir = TempTree::new(fs_tree! {
            //             /-- chunk 0 ---------------\/-- chunk 1 ---------------\/-- chunk 2 --\
            "doc1.txt" => "The cat sat on the big mat. The dog sat on the big mat.",
            "doc2.txt" => "The one bug is big and fat. The other bug is small and skinny.",
        }).await.unwrap();

        let ws = Workspace::open(&dir).await.unwrap();
        let scope = Scope::from(ws.root());

        let chunker = ActiveExtension::new(FixedSizeChunkerExtension::new(28), Default::default())
            .chunkers().next().unwrap();
        let embedder = ActiveExtension::new(
            MockEmbedderExtension::new(
                // Our 3-letter "anchor" words for predictable similarity
                vec!["the", "and", "cat", "dog", "bug", "big", "mat", "sat", "fat", "bad"]
            ), Default::default())
            .embedders().next().unwrap();
        let sample = embedder.embed(&["The cat sat on the big mat."]).await.unwrap().pop().unwrap();
        let retriever = Retriever::new(ws.clone(), scope, chunker, embedder).await.unwrap();

        let results = retriever.top_k(sample.clone(), 4).await.unwrap();
        assert_eq!(results.len(), 4);
        let chunks = results.iter().map(|(doc, chunk_idx, _sim)|
            doc.chunk(chunk_idx.clone()).unwrap()
        ).collect::<Vec<_>>();
        assert_eq!(chunks[0].text(), "The cat sat on the big mat. ");
        assert_eq!(chunks[1].text(), "The dog sat on the big mat.");
        assert_eq!(chunks[2].text(), "The one bug is big and fat. ");
        assert_eq!(chunks[3].text(), "The other bug is small and s");
        dir.iter();
    }
        
}