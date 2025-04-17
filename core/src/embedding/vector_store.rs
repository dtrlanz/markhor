use std::{collections::HashMap, ops::Range, path::PathBuf};

use tracing::error;
use uuid::Uuid;

use crate::{extension::FunctionalityId, storage::Document};

use super::{Chunker, Embedder, Embedding, EmbeddingError};

const MINIMUM_SIMILARITY: f32 = 0.6;


struct VectorStore {
    embedder: Box<dyn Embedder>,
    documents: HashMap<Uuid, DocumentEmbeddings>,
}

impl VectorStore {
    fn new(embedder: Box<dyn Embedder>) -> Self {
        VectorStore {
            embedder,
            documents: HashMap::new(),
        }
    }

    async fn add_documents<C: Chunker>(&mut self, doc: Document, chunker: C) -> Result<(), EmbeddingError> {
        // Check if embeddings for this document are already cached
        if self.documents.contains_key(doc.id()) {
            return Ok(());
        }

        // TODO: fix error handling
        let md_files = doc.files_by_extension("md").await.map_err(|e| EmbeddingError::Provider(Box::new(e)))?;

        let doc_embeddings = doc.with_metadata::<_, Result<DocumentEmbeddings, EmbeddingError>>(async |metadata| {
            let mut doc_embeddings = DocumentEmbeddings::new();

            // TODO: process files concurrently instead
            for file in md_files {
                let existing = metadata.file(file.file_name())
                    .and_then(|md| md.embeddings(&*self.embedder));

                let embeddings = if let Some(embeddings) = existing {
                    embeddings
                } else {
                    // Generate chunks
                    // TODO: fix error handling
                    let text = file.read_string().await.map_err(|e| EmbeddingError::Provider(Box::new(e)))?;
                    let chunk_ranges = chunker.chunk(&text)?;
                    let chunk_texts = chunk_ranges.iter()
                        .map(|range| chunker.get_chunk_text(&text, range.clone()))
                        .collect::<Vec<_>>();
                    let chunk_texts_str = chunk_texts.iter().map(|s| s.as_ref()).collect::<Vec<_>>();

                    // Generate embeddings
                    let chunk_embeddings = self.embedder.embed(&chunk_texts_str).await?;
                    let embeddings = chunk_embeddings.into_iter()
                        .zip(chunk_ranges.into_iter())
                        .collect::<Vec<_>>();

                    // Update metadata file
                    let new_embeddings = metadata.to_mut()
                        .file_mut(file.file_name())
                        .embeddings_mut(&*self.embedder);
                    *new_embeddings = embeddings;

                    metadata.file(file.file_name())
                        .and_then(|md| md.embeddings(&*self.embedder))
                        .unwrap()
                };

                let file_idx = doc_embeddings.file_names.len();
                doc_embeddings.file_names.push(file.file_name().to_string());
                doc_embeddings.embeddings.extend(embeddings.iter().map(|(embedding, _)| embedding.clone()));
                doc_embeddings.chunks.extend(embeddings.iter().map(|(_, range)| (file_idx, range.clone())));
            }
            Ok(doc_embeddings)
                // TODO: fix error handling
                // handle storage errors (outer) and embedding errors (inner)
        }).await.map_err(|e| EmbeddingError::Provider(Box::new(e)))??; 

        // Add document to embeddings map
        self.documents.insert(*doc.id(), doc_embeddings);
        Ok(())
    }

    async fn search(&self, embedding: Embedding, limit: usize) -> Result<HashMap<Uuid, HashMap<String, Vec<ChunkResult>>>, EmbeddingError> {
        let mut count: usize = 0;

        // Collect all embeddings above the minimum similarity threshold
        let mut results = Vec::with_capacity(limit);
        for (doc_id, doc) in self.documents.iter() {
            for (idx, chunk_embedding) in doc.embeddings.iter().enumerate() {
                count += 1;
                let similarity = embedding.similarity(&chunk_embedding)?;
                if similarity > MINIMUM_SIMILARITY {
                    let file_name = &*doc.file_names[doc.chunks[idx].0];
                    let range = &doc.chunks[idx].1;
                    let chunk_result = (doc_id, file_name, range, similarity, 0usize);
                    results.push(chunk_result);
                }
            }
        }

        // Sort by similarity
        results.sort_by(|a, b| b.3.partial_cmp(&a.3).unwrap());

        // Truncate
        results.shrink_to(limit);

        // Assign ranks
        for (idx, result) in results.iter_mut().enumerate()  {
            result.4 = idx;
        }

        // Group by documents
        let mut grouped_by_doc = HashMap::new();
        for (doc_id, file_name, range, similarity, rank) in results {
            grouped_by_doc.entry(doc_id).or_insert_with(Vec::new).push((file_name, range, similarity, rank));
        }

        // Group by files
        let mut grouped_by_doc_and_file = HashMap::new();
        for (doc_id, chunks) in grouped_by_doc {
            let mut grouped_by_file = HashMap::new();
            for (file_name, range, similarity, rank) in chunks {
                let percentile = u32::try_from((rank + 1) * 100 / count).unwrap();
                grouped_by_file.entry(file_name.to_string()).or_insert_with(Vec::new).push(
                    ChunkResult {
                        range: range.clone(),
                        similarity,
                        rank,
                        percentile,
                    }
                );
            }
            grouped_by_doc_and_file.insert(doc_id.clone(), grouped_by_file);
        }

        Ok(grouped_by_doc_and_file)
    }

}

#[derive(Debug, Clone)]
struct DocumentEmbeddings {
    file_names: Vec<String>,
    chunks: Vec<(usize, Range<usize>)>, // Tuple fields: (file_name_idx, chunk_range)
    embeddings: Vec<Embedding>,         // Elements correspond to those in `chunks`
}

impl DocumentEmbeddings {
    fn new() -> DocumentEmbeddings {
        DocumentEmbeddings {
            file_names: Vec::new(),
            chunks: Vec::new(),
            embeddings: Vec::new(),
        }
    }
}

struct ChunkResult {
    range: Range<usize>,
    similarity: f32,
    rank: usize,
    percentile: u32,
}