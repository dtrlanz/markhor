use std::ops::Range;


pub trait EmbeddingModel {
    fn embed(&self, text: &str) -> Result<Vec<f32>, EmbeddingError>;
    fn embed_batch(&self, texts: Vec<&str>) -> Result<Vec<Vec<f32>>, EmbeddingError>;
}


#[derive(Debug, thiserror::Error)]
pub enum EmbeddingError {
    #[error("Embedding Error: {0}")]
    EmbeddingError(String),
    #[error("Failed to deserialize chunk data: {0}")]
    DeserializationError(String),
}



/// A trait for chunking text into smaller segments for embedding.
pub trait Chunker {
    /// Chunk the input text into a range of indices.
    fn chunk(&self, text: &str) -> Result<Range<usize>, EmbeddingError>;

    /// Get the text corresponding to the chunk.
    /// 
    /// The default implementation should be adequate for most use cases. It is equivalent to
    /// `text[range]`. However, the method allows customization, for example in case some headers
    /// are to be include alongside the chunked text.
    fn get_chunk_text(&self, input_text: &str, range: Range<usize>) -> String {
        String::from(&input_text[range])
    }
}
