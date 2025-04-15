use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::extension::Functionality;

use super::{error::EmbeddingError, Embedding};

/// Trait for asynchronous text embedding generation.
///
/// An implementor of this trait represents a specific configured embedding model
/// (e.g., a connection to OpenAI's 'text-embedding-3-small' or a loaded
/// local sentence-transformer model), potentially specialized for a particular use case.
#[async_trait]
pub trait Embedder: Functionality + Send + Sync {
    /// Generates embeddings for a batch of text chunks asynchronously.
    ///
    /// # Arguments
    ///
    /// * `texts`: A slice of string slices (`&str`) to be embedded.
    ///
    /// # Returns
    ///
    /// A `Result` containing either:
    /// * `Ok(Vec<Embedding>)`: Wrapper containing vectors for each input text chunk.
    /// * `Err(EmbeddingError)`: An error encountered during the process. Implementations
    ///   should use specific variants like `InputTooLong` or `BatchTooLarge` when
    ///   input limits are exceeded.
    async fn embed(&self, texts: &[&str]) -> Result<Vec<Embedding>, EmbeddingError>;

    /// Returns the number of dimensions in the embedding vectors produced by this embedder.
    ///
    /// Returns `None` if the dimensionality is not fixed or cannot be determined reliably.
    fn dimensions(&self) -> Option<usize>;

    /// Returns an identifier for the specific embedding model configured for this embedder instance.
    /// (e.g., "text-embedding-3-small", "all-MiniLM-L6-v2").
    fn model_name(&self) -> &str;

    /// Returns the primary intended use case for which this `Embedder` instance is configured.
    ///
    /// This helps select the appropriate embedder when multiple are available.
    /// If the model/configuration is general-purpose, it should return `EmbeddingUseCase::General`.
    fn intended_use_case(&self) -> EmbeddingUseCase;

    /// Returns a hint for the maximum recommended number of items (text chunks) in a single batch
    /// passed to the `embed` method.
    ///
    /// Returns `None` if no simple limit applies or can be determined.
    /// Exceeding this hint *may* result in `EmbeddingError::BatchTooLarge`.
    fn max_batch_size_hint(&self) -> Option<usize>;

    /// Returns a hint for the maximum recommended length of a single text chunk (in characters)
    /// passed to the `embed` method.
    ///
    /// This is intended as a conservative guideline for pre-splitting text. The actual limit
    /// might be based on tokens and vary with text content.
    /// Returns `None` if no simple limit applies or can be determined.
    /// Exceeding this hint *may* result in `EmbeddingError::InputTooLong` or silent truncation,
    /// depending on the implementation.
    fn max_chunk_length_hint(&self) -> Option<usize>; // Unit: Characters
}


/// Represents common intended use cases for embeddings.
///
/// This helps classify an Embedder instance based on its configuration,
/// aiding in selecting the appropriate embedder for a downstream task.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum EmbeddingUseCase {
    /// Optimized for finding semantically similar documents/texts.
    Similarity,
    /// Optimized for representing documents to be retrieved (e.g., database population).
    RetrievalDocument,
    /// Optimized for representing queries used for retrieval.
    RetrievalQuery,
    /// Optimized for tasks like classification.
    Classification,
    /// Optimized for tasks like clustering.
    Clustering,
    /// Optimized for question answering contexts.
    QuestionAnswering,
    /// Optimized for fact verification tasks.
    FactVerification, // Added based on Gemini's list
    /// Optimized for code retrieval query tasks.
    CodeRetrievalQuery, // Added based on Gemini's list
    /// General-purpose embeddings, not specifically optimized for one task.
    General,
    /// Represents a use case not covered by the standard variants.
    /// The string should provide a descriptive identifier (e.g., "provider:task_name").
    Other(String),
}
