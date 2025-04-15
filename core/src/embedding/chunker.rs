use std::ops::Range;

use crate::extension::Functionality;

use super::error::EmbeddingError;

/// A trait for chunking text into smaller segments for embedding.
pub trait Chunker: Functionality {
    /// Chunk the input text into a range of indices.
    fn chunk(&self, text: &str) -> Result<Vec<Range<usize>>, EmbeddingError>;

    /// Get the text corresponding to the chunk.
    /// 
    /// The default implementation should be adequate for most use cases. It is equivalent to
    /// `text[range]`. However, the method allows customization, for example in case some headers
    /// are to be include alongside the chunked text.
    fn get_chunk_text(&self, input_text: &str, range: Range<usize>) -> String {
        String::from(&input_text[range])
    }
}

