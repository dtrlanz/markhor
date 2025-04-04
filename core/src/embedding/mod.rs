mod error;
mod embedder;
mod chunker;

pub use error::EmbeddingError;
pub use embedder::{Embedder, EmbeddingUseCase};
pub use chunker::Chunker;

use serde::{Deserialize, Serialize};

/// Represents a batch of embedding vectors.
///
/// This struct wraps a `Vec<Vec<f32>>` where each inner vector is an embedding.
/// The order corresponds to the order of the input texts provided to the `Embedder`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Embeddings(pub Vec<Vec<f32>>);

impl Embeddings {
    /// Returns the number of embeddings in the batch.
    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// Returns true if the batch contains no embeddings.
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Returns an iterator over the embedding vectors.
    pub fn iter(&self) -> std::slice::Iter<'_, Vec<f32>> {
        self.0.iter()
    }

    /// Returns a slice of the embedding vectors.
    pub fn as_slice(&self) -> &[Vec<f32>] {
        &self.0
    }

    /// Consumes the `Embeddings` wrapper and returns the inner `Vec<Vec<f32>>`.
    pub fn into_inner(self) -> Vec<Vec<f32>> {
        self.0
    }
}

// Allow easy conversion from the raw Vec<Vec<f32>> for implementers.
impl From<Vec<Vec<f32>>> for Embeddings {
    fn from(vecs: Vec<Vec<f32>>) -> Self {
        Embeddings(vecs)
    }
}

// Optional: Allow borrowing the inner data directly if desired later
// impl std::ops::Deref for Embeddings {
//     type Target = Vec<Vec<f32>>;
//     fn deref(&self) -> &Self::Target {
//         &self.0
//     }
// }
// impl std::ops::DerefMut for Embeddings {
//     fn deref_mut(&mut self) -> &mut Self::Target {
//         &mut self.0
//     }
// }