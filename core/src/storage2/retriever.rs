use std::{collections::{HashMap, HashSet, hash_map::Entry}, path::PathBuf, sync::Arc};

use thiserror::Error;
use uuid::Uuid;

use crate::{chunking::{Chunker, ChunkerError}, embedding::{Embedder, Embedding, EmbeddingError}, extension::F11y, storage2::{AccessStorageError, ChunkIdx, Document, Scope, TextHash, Workspace}, vector_store::{DocVersionId, VectorView}};




pub struct Retriever {
    workspace: Workspace,
    scope: Scope,
    docs: HashMap<Uuid, IncludedDoc>,
    chunker: F11y<dyn Chunker>,
    embedder: F11y<dyn Embedder>,
    embeddings: VectorView,
}

impl Retriever {
    pub async fn new(workspace: Workspace, scope: Scope, chunker: F11y<dyn Chunker>, embedder: F11y<dyn Embedder>) -> Result<Self, InitRetrieverError> {
        let vector_store = workspace.vector_store(&embedder);
        let utils = Arc::new((chunker, embedder));

        // Collect complete list of documents and identify which are not yet included in the 
        // vector store.
        let mut docs = Vec::new();
        let mut doc_stream = scope.docs(&workspace).await?;
        while let Some(doc) = doc_stream.next_doc().await? {
            docs.push(doc);
        }
        let mut doc_ids = docs.iter().map(|doc| (*doc.id(), doc.doc_hash()));

        let view = match vector_store.select(doc_ids).await {
            Ok(view) => view,
            Err(missing_vec) => {
                // Retrieve/generate vectors concurrently
                let missing_set = missing_vec.into_iter().collect::<HashSet<_>>();
                let mut doc_ids = Vec::new();
                let (tx, mut rx) = tokio::sync::mpsc::channel(100);
                let mut tasks = Vec::new();
                for doc in docs {
                    doc_ids.push((*doc.id(), doc.doc_hash()));
                    if missing_set.contains(&(*doc.id(), doc.doc_hash())) {
                        let task = tokio::spawn(
                            Self::send_doc_vectors(doc, utils.clone(), tx.clone())
                        );
                        tasks.push(task);
                    }
                }

                // Insert vectors into vector store
                vector_store.insert_docs(rx).await;

                // Check for embedding errors
                for task in tasks {
                    task.await.unwrap()?;
                }

                // TODO: consider adjusting type or signature to reduce monomorphization
                vector_store.select(doc_ids).await.unwrap()
            },
        };

        let (chunker, embedder) = Arc::into_inner(utils).unwrap();

        Ok(Self {
            workspace,
            scope,
            docs: HashMap::new(),
            chunker,
            embedder,
            embeddings: view,
        })
    }

    async fn send_doc_vectors(mut doc: Document, utils: Arc<(F11y<dyn Chunker>, F11y<dyn Embedder>)>, sender: tokio::sync::mpsc::Sender<(DocVersionId, Vec<(ChunkIdx, TextHash, Embedding)>)>) -> Result<(), InitRetrieverError> {
        let chunker = &utils.0;
        let embedder = &utils.1;

        let id = DocVersionId::from(&doc);
        let chunks = doc.chunks_mut(chunker).await?;
        let mut vectors = Vec::new();
        for mut chunk in chunks {
            if let Some(emb) = chunk.embedding(&embedder.metadata_id()) {
                vectors.push((chunk.chunk_idx().clone(), *chunk.hash(), emb.clone()));
                continue;
            }
            let mut embs = embedder.embed(&[chunk.text()]).await?;
            let emb = embs.pop().unwrap();
            chunk.embedding_entry(embedder.metadata_id()).insert_entry(emb.clone());
            vectors.push((chunk.chunk_idx().clone(), *chunk.hash(), emb));
        }
        sender.send((id, vectors)).await.unwrap();
        Ok(())
    }
}


enum IncludedDoc {
    InMemory(Arc<Document>),
    OnDisk(TextHash, PathBuf),
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
