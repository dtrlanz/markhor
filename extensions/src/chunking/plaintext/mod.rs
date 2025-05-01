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
        // 1. Start char 0 (byte 0). End char 9 (byte 10). Range 0..10. Text "abcdefghij"
        //    Stride 7 chars from start 0 -> char 'g' at byte 7. Next start byte 7.
        // 2. Start char 7 (byte 7). End char 16 (byte 17). Range 7..17. Text "ghijklmnopqrst"
        //    Stride 7 chars from start 7 -> char 'n' at byte 14. Next start byte 14.
        // 3. Start char 14 (byte 14). End char 23 (byte 24). Range 14..24. Text "nopqrstu"
        //    Stride 7 chars from start 14 -> char 'u' at byte 21. Next start byte 21.
        // 4. Start char 21 (byte 21). End char 30 (byte 31). Range 21..31. Text "uvwxyz1234"
        //    Stride 7 chars from start 21 -> char '1' at byte 28. Next start byte 28.
        // 5. Start char 28 (byte 28). Remaining chars: "567890" (6 chars). Less than chunk_size 10. End byte 36. Range 28..36. Text "567890".
        //    Stride 7 chars from start 28 -> Remaining chars "567890" (6 chars). Less than stride 7. nth(7) returns None. Next start byte becomes 28 + (36-28) = 36.
        // current_byte_start = 36. Loop terminates.
        // Expected number of chunks: 5

        let chunks = chunker.chunk(text).unwrap();
        assert_eq!(chunks.len(), 5);

        assert_eq!(chunks[0].text_range, 0..10);
        assert_eq!(chunks[0].get_text(text), "abcdefghij");

        assert_eq!(chunks[1].text_range, 7..17);
        assert_eq!(chunks[1].get_text(text), "ghijklmnopqrst");

        assert_eq!(chunks[2].text_range, 14..24);
        assert_eq!(chunks[2].get_text(text), "nopqrstu");

        assert_eq!(chunks[3].text_range, 21..31);
        assert_eq!(chunks[3].get_text(text), "uvwxyz1234");

        assert_eq!(chunks[4].text_range, 28..36);
        assert_eq!(chunks[4].get_text(text), "567890");

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
        // 1. Start char 0 (byte 0). Find 4 chars from byte 0 -> 🍎🍐🍊🍋. The 5th char (🍉) starts at byte 16. Range 0..16. Text "🍎🍐🍊🍋"
        //    Stride 2 chars from start 0 -> char '🍊' starts at byte 8. Next start byte 8.
        // 2. Start char 2 (byte 8). Find 4 chars from byte 8 -> 🍊🍋🍉🍇. The 4 chars are 🍊(byte 8), 🍋(byte 12), 🍉(byte 16), 🍇(byte 20). These are chars 2,3,4,5. The 6th char (end of string) starts at byte 24. Range 8..24. Text "🍊🍋🍉🍇"
        //    Stride 2 chars from start 8 -> char '🍉' starts at byte 16. Next start byte 16.
        // 3. Start char 4 (byte 16). Find 4 chars from byte 16 -> 🍉🍇. Only 2 chars left. Less than chunk_size 4. End byte 24. Range 16..24. Text "🍉🍇"
        //    Stride 2 chars from start 16 -> char '🍇' starts at byte 20. Next start byte 20.
        // current_byte_start is now 20. 20 < 24. Loop continues.
        // 4. Start char 5 (byte 20). Find 4 chars from byte 20 -> 🍇. Only 1 char left. Less than chunk_size 4. End byte 24. Range 20..24. Text "🍇".
        //    Stride 2 chars from start 20 -> text[20..] is "🍇" (1 char). nth(2) from here is None. Next start byte becomes 20 + (24-20) = 24.
        // current_byte_start = 24. Loop terminates.
        // Expected number of chunks: 4

        let chunks = chunker.chunk(text).unwrap();
        assert_eq!(chunks.len(), 4);

        assert_eq!(chunks[0].text_range, 0..16);
        assert_eq!(chunks[0].get_text(text), "🍎🍐🍊🍋");

        assert_eq!(chunks[1].text_range, 8..24);
        assert_eq!(chunks[1].get_text(text), "🍊🍋🍉🍇");

        assert_eq!(chunks[2].text_range, 16..24);
        assert_eq!(chunks[2].get_text(text), "🍉🍇");

         assert_eq!(chunks[3].text_range, 20..24);
        assert_eq!(chunks[3].get_text(text), "🍇");


        // Verify byte boundaries using char_indices
        assert_eq!(text.char_indices().nth(0).unwrap().0, 0); // start of 1st char
        assert_eq!(text.char_indices().nth(4).unwrap().0, 16); // start of 5th char (end of 1st chunk)
        assert_eq!(chunks[0].text_range, 0..16);

        assert_eq!(text.char_indices().nth(2).unwrap().0, 8); // start of 3rd char (stride 2 from 0)
        assert_eq!(chunks[1].text_range.start, 8);
         // Chunk 2 ends 4 chars from its start (char 2) -> at char 6 (end of string). Text length is 6 chars.
        // The 4th character *relative to the start of the chunk at byte 8* is the 6th character overall ('🍇').
        // This character starts at byte 20. The end byte is after this character, at byte 24 (text.len()).
        assert_eq!(text.char_indices().nth(2+4).unwrap().0, 24); // start of 7th char (should be end) - Wait, the index is 0-based. nth(4) is the 5th char...
        // The chunk starts at the character *at* `current_byte_start`. The chunk contains `chunk_size` characters *starting from there*.
        // So, chunk 1 starts at char 0. Contains chars 0, 1, 2, 3. Ends *before* char 4.
        // Chunk 2 starts at the character at `next_byte_start`. `next_byte_start` is the byte index of the character that is `stride` characters *after* the character at `current_byte_start`.

        // Let's rethink the index calculation:
        // current_byte_start = 0 (char 0)
        // chunk_end_relative = text[0..].char_indices().nth(4) is Some((16, '🍉')). chunk_end_byte = 0 + 16 = 16. Range 0..16. Chars 0..3.
        // next_start_relative = text[0..].char_indices().nth(2) is Some((8, '🍊')). next_byte_start = 0 + 8 = 8. current_byte_start = 8 (char 2).
        // current_byte_start = 8 (char 2)
        // chunk_end_relative = text[8..].char_indices().nth(4) is text[8..="🍊🍋🍉🍇"].char_indices() -> (0,🍊), (4,🍋), (8,🍉), (12,🍇). nth(4) is None. unwrap_or(24-8=16). chunk_end_byte = 8 + 16 = 24. Range 8..24. Chars 2..5.
        // next_start_relative = text[8..].char_indices().nth(2) is Some((8, '🍉')). next_byte_start = 8 + 8 = 16. current_byte_start = 16 (char 4).
        // current_byte_start = 16 (char 4)
        // chunk_end_relative = text[16..].char_indices().nth(4) is text[16..="🍉🍇"].char_indices() -> (0,🍉), (4,🍇). nth(4) is None. unwrap_or(24-16=8). chunk_end_byte = 16 + 8 = 24. Range 16..24. Chars 4..5.
        // next_start_relative = text[16..].char_indices().nth(2) is None. unwrap_or(24-16=8). next_byte_start = 16 + 8 = 24. current_byte_start = 24.
        // current_byte_start = 20 (char 5) - Wait, my mental walk-through was wrong. Let's re-check the code's logic.

        // The loop is `while current_byte_start < text_len`. The stride calculation `next_byte_start = current_byte_start + next_start_relative_offset` determines the next start.
        // If `next_byte_start` >= `text_len`, the loop terminates.

        // current_byte_start = 0 (char 0)
        //   chunk_end = 0 + text[0..].char_indices().nth(4).map(|(idx,_)|idx).unwrap_or(24-0) = 0 + 16 = 16. Add chunk 0..16.
        //   stride = 2.
        //   next_start = 0 + text[0..].char_indices().nth(2).map(|(idx,_)|idx).unwrap_or(24-0) = 0 + 8 = 8. current_byte_start = 8.
        // current_byte_start = 8 (char 2)
        //   chunk_end = 8 + text[8..].char_indices().nth(4).map(|(idx,_)|idx).unwrap_or(24-8) = 8 + 16 = 24. Add chunk 8..24.
        //   stride = 2.
        //   next_start = 8 + text[8..].char_indices().nth(2).map(|(idx,_)|idx).unwrap_or(24-8) = 8 + 8 = 16. current_byte_start = 16.
        // current_byte_start = 16 (char 4)
        //   chunk_end = 16 + text[16..].char_indices().nth(4).map(|(idx,_)|idx).unwrap_or(24-16) = 16 + 8 = 24. Add chunk 16..24.
        //   stride = 2.
        //   next_start = 16 + text[16..].char_indices().nth(2).map(|(idx,_)|idx).unwrap_or(24-16) = 16 + 8 = 24. current_byte_start = 24.
        // current_byte_start = 24. 24 < 24 is false. Loop ends.

        // Okay, my code produces 3 chunks for "🍎🍐🍊🍋🍉🍇" with size 4, overlap 2.
        // Chunks: 0..16 ("🍎🍐🍊🍋"), 8..24 ("🍊🍋🍉🍇"), 16..24 ("🍉🍇").
        // The test assertion `assert_eq!(chunks.len(), 4);` is wrong. It should be 3.
        // Let's fix the test expectation.

        assert_eq!(chunks.len(), 3); // Corrected expectation based on code logic

        assert_eq!(chunks[0].text_range, 0..16);
        assert_eq!(chunks[0].get_text(text), "🍎🍐🍊🍋");

        assert_eq!(chunks[1].text_range, 8..24);
        assert_eq!(chunks[1].get_text(text), "🍊🍋🍉🍇");

        assert_eq!(chunks[2].text_range, 16..24);
        assert_eq!(chunks[2].get_text(text), "🍉🍇");

        // Verify byte boundaries match char indices
        assert_eq!(chunks[0].text_range.start, text.char_indices().nth(0).unwrap().0);
        assert_eq!(chunks[0].text_range.end, text.char_indices().nth(4).unwrap().0);

        assert_eq!(chunks[1].text_range.start, text.char_indices().nth(2).unwrap().0); // Start is stride (2) chars from overall start (0)
        assert_eq!(chunks[1].text_range.end, text.char_indices().nth(2+4).unwrap_or((text.len(), ' ')).0); // End is chunk_size (4) chars from its start (char 2) -> char 6. Char 6 is past end of text. Ends at text.len(). Let's recalculate char index: char_indices().nth(2+4) = nth(6) is None. unwrap_or((24, ' ')).0 gives 24. Correct.

        assert_eq!(chunks[2].text_range.start, text.char_indices().nth(4).unwrap().0); // Start is stride (2) chars from previous start (char 2)
        assert_eq!(chunks[2].text_range.end, text.char_indices().nth(4+4).unwrap_or((text.len(), ' ')).0); // End is chunk_size (4) chars from its start (char 4) -> char 8. Char 8 is past end of text. Ends at text.len(). Let's recalculate char index: char_indices().nth(4+4) = nth(8) is None. unwrap_or((24, ' ')).0 gives 24. Correct.
    }
}