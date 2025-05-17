use std::{collections::HashMap, sync::Arc};

use tracing::instrument;

use crate::{chunking::Chunk, embedding::{ChunkDataResult, VectorStore}, extension::UseExtensionError, storage::{self, ContentFile, Document}};

use super::{Assets, Job, RunJobError};


pub fn search_job(query: &str, limit: usize) -> Job<SearchResults, impl AsyncFnOnce(&mut Assets) -> Result<SearchResults, RunJobError> + Send> {
    Job::new(async move |assets| {
        let embedder = assets.embedders().into_iter().next()
            .ok_or(UseExtensionError::EmbeddingModelNotAvailable)?;

        let mut store = VectorStore::new(embedder);

        let chunker = assets.chunkers().into_iter().nth(0)
            .ok_or(UseExtensionError::ChunkerNotAvailable)?;

        let mut doc_ids = HashMap::with_capacity(assets.documents().len());

        for doc in assets.documents() {
            store.add_document(doc.clone(), &*chunker).await?;
            doc_ids.insert(doc.id().clone(), doc);
        }

        let query_embedding = store.embedder().embed(&[query]).await?;

        let chunk_results = store.search(&query_embedding[0], limit).await?;

        let results = chunk_results.into_iter()
            .map(|(doc_id, chunks_by_file)| {
                let &doc = doc_ids.get(&doc_id).unwrap();
                (doc.clone(), chunks_by_file)
            })
            .collect::<HashMap<_, _>>();

        Ok(SearchResults {
            docs: results,
        })
    })
}

pub struct SearchResults {
    docs: HashMap<Document, HashMap<String, Vec<ChunkDataResult>>>,
}

impl SearchResults {
    pub fn documents(&self) -> Documents {
        Documents {
            iter: self.docs.iter(),
        }
    }
}

pub struct Documents<'a> {
    iter: std::collections::hash_map::Iter<'a, Document, HashMap<String, Vec<ChunkDataResult>>>,
}

impl<'a> Iterator for Documents<'a> {
    type Item = DocResults<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        if let Some((doc, files)) = self.iter.next() {
            Some(DocResults {
                doc,
                files,
            })
        } else {
            None
        }
    }
}

pub struct DocResults<'a> {
    doc: &'a Document,
    files: &'a HashMap<String, Vec<ChunkDataResult>>,
}

impl<'a> DocResults<'a> {
    pub fn document(&self) -> &'a Document {
        self.doc
    }

    pub fn files(&self) -> Files<'a> {
        Files {
            doc: self.doc,
            iter: self.files.iter(),
        }
    }
}

pub struct Files<'a> {
    doc: &'a Document,
    iter: std::collections::hash_map::Iter<'a, String, Vec<ChunkDataResult>>,
}

impl<'a> Iterator for Files<'a> {
    type Item = FileResults<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        if let Some((file_name, chunks)) = self.iter.next() {
            Some(FileResults {
                doc: self.doc,
                file_name: file_name.clone(),
                chunks,
            })
        } else {
            None
        }
    }
}

pub struct FileResults<'a> {
    doc: &'a Document,
    file_name: String,
    chunks: &'a [ChunkDataResult],
}

impl<'a> FileResults<'a> {
    pub fn document(&self) -> &'a Document {
        self.doc
    }

    pub fn file_name(&self) -> &str {
        &self.file_name
    }

    #[instrument(skip(self))]
    pub async fn file(&self) -> Result<ContentFile<'a>, storage::Error> {
        match self.doc.file(&self.file_name).await {
            Ok(file) => Ok(file),
            Err(e) => {
                tracing::error!("Error retrieving source file: {}", e);
                Err(e)
            }
        }
    }

    #[instrument(skip(self))]
    pub async fn chunks(&self) -> Result<Chunks<'a>, storage::Error> {
        let text = self.file().await?.read_string().await?;
        Ok(Chunks {
            source_text: Arc::new(text),
            iter: self.chunks.iter(),
        })
    }
}

pub struct Chunks<'a> {
    source_text: Arc<String>,
    iter: std::slice::Iter<'a, ChunkDataResult>,
}

impl<'a> Iterator for Chunks<'a> {
    type Item = ChunkResult;

    fn next(&mut self) -> Option<Self::Item> {
        if let Some(chunk_data) = self.iter.next() {
            Some(ChunkResult {
                data: chunk_data.clone(),
                source_text: self.source_text.clone(),
            })
        } else {
            None
        }
    }
}

pub struct ChunkResult {
    data: ChunkDataResult,
    source_text: Arc<String>,
}

impl ChunkResult {
    pub fn chunk(&self) -> Chunk {
        Chunk {
            data: &self.data.chunk,
            source_text: &self.source_text,
        }
    }

    pub fn similarity(&self) -> f32 {
        self.data.similarity
    }

    pub fn rank(&self) -> usize {
        self.data.rank
    }

    pub fn percentile(&self) -> u32 {
        self.data.percentile
    }
}