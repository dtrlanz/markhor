use std::{ops::Range, fmt::Display};
use serde::{Deserialize, Serialize};
use thiserror::Error;


/// Trait for algorithms that split source text into logical chunks.
///
/// Implementations of this trait define specific strategies for determining
/// chunk boundaries (e.g., based on Markdown structure, fixed size, sentences).
///
/// The trait returns `ChunkData` instances, which reference ranges within the
/// original text, rather than containing the text itself, to improve efficiency.
/// The calling library is responsible for managing source document IDs and
/// materializing `ChunkData` into usable chunks with actual text content.
pub trait Chunker: Send + Sync {
    /// Chunks the provided source text according to the implementation's strategy.
    ///
    /// # Arguments
    ///
    /// * `source_text`: The source text document to be chunked.
    ///
    /// # Returns
    ///
    /// A `Result` containing either:
    /// * `Ok(Vec<ChunkData>)`: A vector of chunk data representing the identified chunks.
    ///                        The ranges in `ChunkData` refer to byte offsets in `source_text`.
    /// * `Err(ChunkerError)`: An error that occurred during chunking.
    ///
    /// # Note on `ChunkData.text_range`
    ///
    /// Each `ChunkData` instance is expected to represent a single, contiguous
    /// byte range (`start..end`) from the `source_text`. Implementations should
    /// strive to produce non-overlapping or minimally overlapping ranges based
    /// on their strategy (though overlap logic might be handled by the calling library
    /// during materialization if not done here).
    fn chunk(&self, source_text: &str) -> Result<Vec<ChunkData>, ChunkerError>;
}



/// Represents the raw output of a `Chunker` implementation,
/// referencing a contiguous slice of the original source text.
/// The calling library is responsible for materializing this into
/// a usable chunk with actual text content and associating it
/// with source document identifiers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChunkData {
    /// The byte range (`start..end`) within the original source text
    /// that corresponds to this chunk's primary content.
    pub text_range: Range<usize>,

    /// An optional string representing the semantic location of this chunk,
    /// typically derived from document headings (e.g., "Section 1 > Subsection A").
    /// Provided by chunkers that understand document structure (like Markdown).
    pub heading_path: Option<String>,

    /// An optional token count for the text within `text_range`,
    /// calculated using a specific tokenizer. Not all chunkers may
    /// compute or provide this. If `None`, the calling library may
    /// need to calculate it after materializing the text.
    pub token_count: Option<usize>,

    // --- Potential Future Metadata ---
    // /// Optional start/end line numbers in the original source
    // pub line_range: Option<Range<usize>>,
    // /// Additional simple key-value metadata specific to the chunker type
    // pub custom_metadata: Option<std::collections::HashMap<String, String>>,
}

impl ChunkData {
    /// Creates a new `ChunkData` instance with the specified source text.
    /// 
    /// The text passed as `source_text` must be the source text used when creating this 
    /// `ChunkData` instance. Otherwise the result is meaningless.
    pub fn to_chunk<'a>(&'a self, source_text: &'a str) -> Chunk<'a> {
        Chunk {
            data: self,
            source_text,
        }
    }
}

/// Errors that can occur during the chunking process.
#[derive(Debug, Error)]
pub enum ChunkerError {
    /// A general error occurred during chunk processing logic.
    #[error("Chunker processing failed: {0}")]
    Processing(String),

    /// The configuration provided or inherent to the chunker is invalid.
    #[error("Invalid chunker configuration: {0}")]
    Configuration(String),
    // Add other generic error types applicable across different chunkers if identified
}

#[derive(Debug, Clone, PartialEq, Eq)]
/// Represents a chunk of text materialized from `ChunkData`.
pub struct Chunk<'a> {
    pub(crate) data: &'a ChunkData,
    pub(crate) source_text: &'a str,
}

impl<'a> Chunk<'a> {
    pub fn text(&self) -> &'a str {
        &self.source_text[self.data.text_range.clone()]
    }

    pub fn heading_path(&self) -> Option<&'a str> {
        self.data.heading_path.as_deref()
    }
}

impl<'a> AsRef<ChunkData> for Chunk<'a> {
    fn as_ref(&self) -> &ChunkData {
        self.data
    }
}

impl<'a> Display for Chunk<'a> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if let Some(heading) = self.heading_path() {
            write!(f, "{}:\n\n{}", heading, self.text())
        } else {
            write!(f, "{}", self.text())
        }
    }
}

