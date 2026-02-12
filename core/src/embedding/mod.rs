mod error;
mod embedder;
mod vector_store;

use std::ops::Deref;

pub use error::{EmbeddingError};
pub use embedder::{Embedder, EmbeddingUseCase};
pub use vector_store::{VectorStore, ChunkDataResult};

use serde::{Deserialize, Serialize};

/// Represents an embedding vector.
///
/// This struct simply wraps a `Vec<f32>`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Embedding(pub Vec<f32>);

impl Embedding {
    /// Computes the cosine similarity between two embeddings.
    ///
    /// Returns an error if the vectors are empty or have mismatched lengths.
    pub fn similarity(&self, other: &Embedding) -> Result<f32, EmbeddingError> {
        if self.0.is_empty() || other.0.is_empty() {
            // TODO: fix errors
            //return Err(EmbeddingError::ZeroLength);
            panic!("zero length vector");
        }

        if self.0.len() != other.0.len() {
            //return Err(EmbeddingError::MismatchedLengths);
            panic!("mismatched vector lengths");
        }

        let dot_product: f32 = self.0.iter().zip(&other.0).map(|(a, b)| a * b).sum();
        let norm_self: f32 = self.0.iter().map(|x| x * x).sum::<f32>().sqrt();
        let norm_other: f32 = other.0.iter().map(|x| x * x).sum::<f32>().sqrt();

        if norm_self == 0.0 || norm_other == 0.0 {
            //return Err(EmbeddingError::ZeroLength);
            panic!("zero length vector");
        }

        Ok(dot_product / (norm_self * norm_other))
    }
}

// Allow easy conversion from the raw Vec<Vec<f32>> for implementers.
impl From<Vec<f32>> for Embedding {
    fn from(vec: Vec<f32>) -> Self {
        Embedding(vec)
    }
}

impl From<Embedding> for Vec<f32> {
    fn from(embedding: Embedding) -> Self {
        embedding.0
    }
}

impl AsRef<[f32]> for Embedding {
    fn as_ref(&self) -> &[f32] {
        &self.0
    }
}

impl Deref for Embedding {
    type Target = [f32];

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}