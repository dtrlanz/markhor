use std::ops::Range;
use std::vec::Vec;
use std::result::Result;

use markhor_core::chunking::{ChunkData, Chunker, ChunkerError};



/// A simple chunker that splits text into chunks based on a maximum character count,
/// with optional character overlap between consecutive chunks.
#[derive(Debug, Clone)]
pub struct PlainTextChunker {
    chunk_size: usize,   // Maximum characters per chunk
    overlap_size: usize, // Characters of overlap between chunks
}

impl PlainTextChunker {
    /// Creates a new `PlainTextChunker` instance.
    ///
    /// # Arguments
    ///
    /// * `chunk_size`: The maximum number of characters per chunk. Must be greater than 0.
    /// * `overlap_size`: The number of characters to overlap between consecutive chunks.
    ///                   Must be less than `chunk_size`.
    ///
    /// # Returns
    ///
    /// * `Ok(PlainTextChunker)`: The configured chunker.
    /// * `Err(ChunkerError::Configuration)`: If `chunk_size` is 0 or `overlap_size`
    ///                                      is greater than or equal to `chunk_size`.
    pub fn new(chunk_size: usize, overlap_size: usize) -> Result<Self, ChunkerError> {
        if chunk_size == 0 {
            return Err(ChunkerError::Configuration(
                "Chunk size must be greater than 0".to_string(),
            ));
        }
        if overlap_size >= chunk_size {
            return Err(ChunkerError::Configuration(format!(
                "Overlap size ({}) must be less than chunk size ({})",
                overlap_size, chunk_size
            )));
        }

        Ok(Self {
            chunk_size,
            overlap_size,
        })
    }

    /// Returns the configured maximum chunk size (in characters).
    pub fn chunk_size(&self) -> usize {
        self.chunk_size
    }

    /// Returns the configured overlap size (in characters).
    pub fn overlap_size(&self) -> usize {
        self.overlap_size
    }
}

// Implement the Chunker trait for PlainTextChunker
impl Chunker for PlainTextChunker {
    fn chunk(&self, source_text: &str) -> Result<Vec<ChunkData>, ChunkerError> {
        let mut chunks = Vec::new();
        let mut current_byte_start = 0;
        let text_len = source_text.len();

        let stride = self.chunk_size.saturating_sub(self.overlap_size);
        // The new constructor guarantees stride > 0 if chunk_size > 0 and overlap < chunk_size

        // Iterate while there's potentially text left to form a chunk starting at current_byte_start
        // We check against text_len - stride because if the remaining text is shorter than the stride,
        // the char_indices().nth(stride) calculation will yield text_len - current_byte_start,
        // making the next_byte_start jump directly to text_len and terminate the loop correctly
        // after processing the last possible chunk.
        while current_byte_start < text_len {
            // Find the byte index for the end of the current chunk: self.chunk_size characters from current_byte_start.
            // We use char_indices().nth(self.chunk_size) to get the byte offset relative to current_byte_start
            // where the (chunk_size + 1)-th character begins.
            let chunk_end_relative_offset = source_text[current_byte_start..]
                .char_indices()
                .nth(self.chunk_size)
                .map(|(idx, _)| idx)
                .unwrap_or(text_len - current_byte_start); // If fewer than chunk_size chars remain, end is text_len relative to start

            let chunk_end_byte_exclusive = current_byte_start + chunk_end_relative_offset;

            // If the calculated end is the same as the start, it means no characters were
            // successfully included in a potential chunk (e.g., remaining slice is empty).
            // This check also handles the case where current_byte_start is already >= text_len
            // but the while condition (due to the - stride part) allowed one more entry.
            if chunk_end_byte_exclusive == current_byte_start {
                break;
            }

            // Add the chunk data for the identified range
            chunks.push(ChunkData {
                text_range: current_byte_start..chunk_end_byte_exclusive,
                heading_path: None, // Simple plain text doesn't have headings
                token_count: None,   // No tokenization done here
            });

            // Calculate the start of the *next* chunk. This is `stride` characters from current_byte_start.
            // Use char_indices().nth(stride) to get the byte offset relative to current_byte_start
            // where the (stride + 1)-th character begins.
            let next_start_relative_offset = source_text[current_byte_start..]
                .char_indices()
                .nth(stride)
                .map(|(idx, _)| idx)
                .unwrap_or(text_len - current_byte_start); // If fewer than stride chars remain, next start is text_len relative to start

            let next_byte_start = current_byte_start + next_start_relative_offset;

            // If the remaining chars are only overlap chars, we're done.
            let beyond_overlap = source_text[next_byte_start..]
                .char_indices()
                .nth(self.overlap_size);
            if beyond_overlap.is_none() {
                break;
            }

            // Move the starting point for the next iteration
            current_byte_start = next_byte_start;
        }

        Ok(chunks)
    }
}

// --- Tests ---
#[cfg(test)]
mod tests {
    use super::*;

    // Helper to easily get text slice for testing
    trait GetChunkText {
        fn get_text<'a>(&self, source_text: &'a str) -> &'a str;
    }

    impl GetChunkText for ChunkData {
        fn get_text<'a>(&self, source_text: &'a str) -> &'a str {
            &source_text[self.text_range.clone()]
        }
    }

    #[test]
    fn test_new_valid_config() {
        assert!(PlainTextChunker::new(100, 0).is_ok());
        assert!(PlainTextChunker::new(100, 50).is_ok());
        assert!(PlainTextChunker::new(100, 99).is_ok());
        assert!(PlainTextChunker::new(1, 0).is_ok());
    }

    #[test]
    fn test_new_invalid_config() {
        let err1 = PlainTextChunker::new(0, 0).unwrap_err();
        assert!(matches!(err1, ChunkerError::Configuration(_)));
        assert_eq!(err1.to_string(), "Invalid chunker configuration: Chunk size must be greater than 0");

        let err2 = PlainTextChunker::new(100, 100).unwrap_err();
        assert!(matches!(err2, ChunkerError::Configuration(_)));
        assert_eq!(err2.to_string(), "Invalid chunker configuration: Overlap size (100) must be less than chunk size (100)");

         let err3 = PlainTextChunker::new(100, 101).unwrap_err();
        assert!(matches!(err3, ChunkerError::Configuration(_)));
        assert_eq!(err3.to_string(), "Invalid chunker configuration: Overlap size (101) must be less than chunk size (100)");
    }

    #[test]
    fn test_empty_text() {
        let chunker = PlainTextChunker::new(100, 10).unwrap();
        let text = "";
        let chunks = chunker.chunk(text).unwrap();
        assert!(chunks.is_empty());
    }

    #[test]
    fn test_text_shorter_than_chunk_size_no_overlap() {
        let chunker = PlainTextChunker::new(100, 0).unwrap();
        let text = "Short text."; // 11 chars, 11 bytes
        let chunks = chunker.chunk(text).unwrap();
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0].text_range, 0..11);
        assert_eq!(chunks[0].get_text(text), "Short text.");
    }

     #[test]
    fn test_text_shorter_than_chunk_size_with_overlap() {
        // With overlap, if text is shorter than chunk size, there's still just one chunk
        let chunker = PlainTextChunker::new(100, 10).unwrap();
        let text = "Short text."; // 11 chars, 11 bytes
        let chunks = chunker.chunk(text).unwrap();
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0].text_range, 0..11);
        assert_eq!(chunks[0].get_text(text), "Short text.");
    }


    #[test]
    fn test_text_exactly_chunk_size_no_overlap() {
        let chunker = PlainTextChunker::new(11, 0).unwrap();
        let text = "Hello world"; // 11 chars, 11 bytes
        let chunks = chunker.chunk(text).unwrap();
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0].text_range, 0..11);
        assert_eq!(chunks[0].get_text(text), "Hello world");
    }

    #[test]
    fn test_text_exactly_chunk_size_with_overlap() {
         let chunker = PlainTextChunker::new(11, 5).unwrap();
        let text = "Hello world"; // 11 chars, 11 bytes
        let chunks = chunker.chunk(text).unwrap();
        assert_eq!(chunks.len(), 1); // Still one chunk
        assert_eq!(chunks[0].text_range, 0..11);
        assert_eq!(chunks[0].get_text(text), "Hello world");
    }


    #[test]
    fn test_multiple_chunks_no_overlap() {
        let chunker = PlainTextChunker::new(5, 0).unwrap();
        let text = "abcde12345vwxyz"; // 15 chars, 15 bytes
        let chunks = chunker.chunk(text).unwrap();
        assert_eq!(chunks.len(), 3);
        assert_eq!(chunks[0].text_range, 0..5);
        assert_eq!(chunks[1].text_range, 5..10);
        assert_eq!(chunks[2].text_range, 10..15);
        assert_eq!(chunks[0].get_text(text), "abcde");
        assert_eq!(chunks[1].get_text(text), "12345");
        assert_eq!(chunks[2].get_text(text), "vwxyz");
    }

    #[test]
    fn test_multiple_chunks_with_overlap() {
        let chunker = PlainTextChunker::new(10, 3).unwrap(); // Chunk=10 chars, Overlap=3 chars, Stride=7 chars
        let text = "abcdefghijklmnopqrstuvwxyz1234567890"; // 36 chars, 36 bytes
        // Expected Chunks (start byte, end byte, text):
        // 1. Range  0..10. Text "abcdefghij"
        // 2. Range  7..17. Text "hijklmnopq"
        // 3. Range 14..24. Text "opqrstuvwx"
        // 4. Range 21..31. Text "vwxyz12345"
        // 5. Range 28..36  Text "34567890"
        // Expected number of chunks: 5

        let chunks = chunker.chunk(text).unwrap();
        println!("Chunks: {:?}", chunks);
        assert_eq!(chunks.len(), 5);

        assert_eq!(chunks[0].text_range, 0..10);
        assert_eq!(chunks[0].get_text(text), "abcdefghij");

        assert_eq!(chunks[1].text_range, 7..17);
        assert_eq!(chunks[1].get_text(text), "hijklmnopq");

        assert_eq!(chunks[2].text_range, 14..24);
        assert_eq!(chunks[2].get_text(text), "opqrstuvwx");

        assert_eq!(chunks[3].text_range, 21..31);
        assert_eq!(chunks[3].get_text(text), "vwxyz12345");

        assert_eq!(chunks[4].text_range, 28..36);
        assert_eq!(chunks[4].get_text(text), "34567890");

        // Verify specific byte boundaries using char_indices
         assert_eq!(text.char_indices().nth(0).unwrap().0, chunks[0].text_range.start); // start of 1st char
         assert_eq!(text.char_indices().nth(10).unwrap().0, chunks[0].text_range.end); // start of 11th char

         assert_eq!(text.char_indices().nth(7).unwrap().0, chunks[1].text_range.start); // start of 8th char (stride 7 from 0)
         assert_eq!(text.char_indices().nth(17).unwrap().0, chunks[1].text_range.end); // start of 18th char (10 chars from 8th char)

         assert_eq!(text.char_indices().nth(14).unwrap().0, chunks[2].text_range.start); // start of 15th char (stride 7 from 7)
         assert_eq!(text.char_indices().nth(24).unwrap().0, chunks[2].text_range.end); // start of 25th char (10 chars from 15th char)

         assert_eq!(text.char_indices().nth(21).unwrap().0, chunks[3].text_range.start); // start of 22nd char (stride 7 from 14)
         assert_eq!(text.char_indices().nth(31).unwrap().0, chunks[3].text_range.end); // start of 32nd char (10 chars from 22nd char)

         assert_eq!(text.char_indices().nth(28).unwrap().0, chunks[4].text_range.start); // start of 29th char (stride 7 from 21)
         assert_eq!(chunks[4].text_range.end, text.len()); // last chunk ends at string end
    }

    #[test]
    fn test_text_with_multibyte_chars_no_overlap() {
        let chunker = PlainTextChunker::new(3, 0).unwrap(); // Chunk size 3 characters
        let text = "你好世界👋🌍"; // 你=3 bytes, 好=3, 世=3, 界=3, 👋=4, 🌍=4. Total 20 bytes. 6 characters.
        let chunks = chunker.chunk(text).unwrap();
        // Expected chunks: "你好世" (3 chars), "界👋🌍" (3 chars)
        assert_eq!(chunks.len(), 2);

        // "你好世" -> chars 0,1,2. Starts char 0 (byte 0). Ends before char 3 ('界'). Char 3 ('界') starts at byte 9. Range 0..9
        assert_eq!(chunks[0].text_range, 0..9);
        assert_eq!(chunks[0].get_text(text), "你好世");

        // Next chunk starts at byte 9. "界👋🌍" -> chars 3,4,5. Starts char 3 (byte 9). Ends before char 6 (end of string). String ends at byte 20. Range 9..20
        assert_eq!(chunks[1].text_range, 9..20);
        assert_eq!(chunks[1].get_text(text), "界👋🌍");
    }

     #[test]
    fn test_text_with_multibyte_chars_with_overlap() {
        let chunker = PlainTextChunker::new(4, 2).unwrap(); // Chunk size 4 chars, Overlap 2 chars. Stride = 2 chars.
        let text = "🍎🍐🍊🍋🍉🍇"; // 6 characters. Each emoji is 4 bytes. Total 24 bytes.
        // Chars: 🍎(0) 🍐(1) 🍊(2) 🍋(3) 🍉(4) 🍇(5)
        // Bytes: 🍎(0-3) 🍐(4-7) 🍊(8-11) 🍋(12-15) 🍉(16-19) 🍇(20-23)
        // text.len() = 24

        // Expected Chunks (start byte, end byte, text):
        // 1. Range 0..16. Text "🍎🍐🍊🍋"
        // 2. Range 8..24. Text "🍊🍋🍉🍇"
        // Expected number of chunks: 2

        let chunks = chunker.chunk(text).unwrap();
        assert_eq!(chunks.len(), 2);

        assert_eq!(chunks[0].text_range, 0..16);
        assert_eq!(chunks[0].get_text(text), "🍎🍐🍊🍋");

        assert_eq!(chunks[1].text_range, 8..24);
        assert_eq!(chunks[1].get_text(text), "🍊🍋🍉🍇");

        // Verify byte boundaries using char_indices
        assert_eq!(text.char_indices().nth(0).unwrap().0, 0); // start of 1st char
        assert_eq!(text.char_indices().nth(4).unwrap().0, 16); // start of 5th char (end of 1st chunk)

        assert_eq!(text.char_indices().nth(2).unwrap().0, 8); // start of 3rd char (stride 2 from 0)

    }
}