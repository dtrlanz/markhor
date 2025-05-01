
use markhor_core::chunking::{ChunkData, Chunker, ChunkerError};
use pulldown_cmark::{Event, Options, Parser, Tag, TagEnd};
use std::{fmt::Debug, ops::Range};
use tiktoken_rs::{cl100k_base, CoreBPE};
use tracing::{debug, instrument, trace, warn};

// Configuration for the Markdown Chunker
#[derive(Debug, Clone)]
pub struct MarkdownChunkerConfig {
    /// Target tokenizer model name (e.g., "cl100k_base") used for optional token counting.
    pub tokenizer_model: String,
    // Add other config if needed, e.g., pulldown_cmark options
    // pub cmark_options: Option<Options>,
}

impl Default for MarkdownChunkerConfig {
    fn default() -> Self {
        MarkdownChunkerConfig {
            tokenizer_model: "cl100k_base".to_string(), // Default to OpenAI standard
        }
    }
}

// The Chunker implementation
pub struct MarkdownChunker {
    config: MarkdownChunkerConfig,
    // Pre-initialized tokenizer for efficiency if used multiple times
    tokenizer: Option<CoreBPE>,
}

impl Debug for MarkdownChunker {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MarkdownChunker")
            .field("config", &self.config)
            .finish()
    }
}

impl MarkdownChunker {
    /// Creates a new MarkdownChunker, potentially pre-initializing the tokenizer.
    pub fn new(config: MarkdownChunkerConfig) -> Result<Self, ChunkerError> {
        // Eagerly initialize tokenizer to catch errors early
        let tokenizer = Some(Self::get_tokenizer(&config.tokenizer_model)?);
        Ok(MarkdownChunker { config, tokenizer })
    }

    /// Helper to get the tokenizer instance.
    fn get_tokenizer(model: &str) -> Result<CoreBPE, ChunkerError> {
        // Currently only supports cl100k_base, extend if needed
        if model == "cl100k_base" {
            cl100k_base().map_err(|e| {
                ChunkerError::Configuration(format!(
                    "Failed to initialize tokenizer '{}': {}",
                    model, e
                ))
            })
        } else {
            Err(ChunkerError::Configuration(format!(
                "Unsupported tokenizer model: {}",
                model
            )))
        }
    }

    /// Helper to count tokens for a text slice, returns None on error.
    #[instrument(skip(self, text), fields(text_len = text.len()))]
    fn count_tokens_optional(&self, text: &str) -> Option<usize> {
        self.tokenizer
            .as_ref() // Borrow the tokenizer
            .and_then(|tokenizer| {
                // Assuming encode_with_special_tokens is infallible for now
                let count = tokenizer.encode_with_special_tokens(text).len();
                trace!(token_count = count, "Counted tokens");
                Some(count)
            })
            // Log if tokenizer wasn't available (shouldn't happen with current new())
            .or_else(|| {
                warn!("Tokenizer unavailable for token counting.");
                None
            })
    }
}

// Implement the core Chunker trait
impl Chunker for MarkdownChunker {
    #[instrument(skip(self, source_text), fields(source_len = source_text.len()))]
    fn chunk<'a>(&self, source_text: &'a str) -> Result<Vec<ChunkData>, ChunkerError> {
        let mut results: Vec<ChunkData> = Vec::new();

        // --- State Initialization ---
        let mut heading_stack: Vec<(u32, String)> = Vec::new();
        let mut current_heading_path: Option<String> = None;
        let mut current_block_start: Option<usize> = None; // Start offset of the block being processed
        let mut last_event_end: usize = 0; // End offset of the last processed event
        let mut heading_text_buffer = String::new(); // Temp buffer just for heading text

        // Use into_offset_iter to get byte ranges for each event
        let parser = Parser::new_ext(source_text, Options::empty()); // Add options if needed
        let offset_iter = parser.into_offset_iter();

        trace!("Starting Markdown offset iteration");

        for (event, range) in offset_iter {
            trace!(event = ?event, range = ?range, "Processing event");

            // Track the start of a potential text-containing block
            match &event {
                // Events that typically contain renderable text content often mark the start
                Event::Text(_) | Event::Code(_) | Event::Start(Tag::Item) => {
                    if current_block_start.is_none() {
                        trace!(offset = range.start, "Setting current block start");
                        current_block_start = Some(range.start);
                    }
                }
                // Start of heading/code block might implicitly start a block if not already started
                Event::Start(Tag::Heading { .. }) | Event::Start(Tag::CodeBlock(_)) => {
                    if current_block_start.is_none() {
                        // The actual content starts with Text/Code events inside, but the
                        // *logical* block including markers might start here.
                        // Let's use the range.start of the *first inner Text/Code* for accuracy.
                        // However, if a heading is empty, we need a fallback.
                        // Let's refine: Set start on first Text/Code, finalize on End tags.
                    }
                }
                _ => {}
            }

            // Accumulate heading text when inside a heading
            if !heading_stack.is_empty() {
                 if let Event::Text(text) | Event::Code(text) = &event {
                    heading_text_buffer.push_str(text);
                 }
            }


            // --- Finalize Block on Ending Tags or Structure Changes ---
            let mut finalize_range: Option<Range<usize>> = None;

            match event {
                // Block-level closing tags are primary finalization points
                Event::End(TagEnd::Paragraph)
                | Event::End(TagEnd::CodeBlock)
                | Event::End(TagEnd::Item) // Finalize each list item individually
                | Event::End(TagEnd::Table)
                | Event::End(TagEnd::BlockQuote(..)) => {
                     if let Some(start) = current_block_start.take() {
                        // The range ends at the end of the *last* event within the block
                        let end = last_event_end;
                        if start < end { // Ensure non-empty range
                            finalize_range = Some(start..end);
                            trace!(range = ?finalize_range, tag = ?event, "Finalizing block on End tag");
                        } else {
                            trace!("Skipping empty block finalization on End tag");
                        }
                    }
                }

                // Headings: Finalize the *previous* block *before* processing the heading.
                // Then, the heading itself becomes a block to be finalized on its End tag.
                Event::Start(Tag::Heading { level, .. }) => {
                    let level = level as u32;

                    // 1. Finalize any preceding block
                    if let Some(start) = current_block_start.take() {
                    let end = range.start; // End previous block *before* the heading starts
                        if start < end {
                        finalize_range = Some(start..end);
                        trace!(range = ?finalize_range, "Finalizing block before Heading start");
                        } else {
                            trace!("Skipping empty block finalization before Heading start");
                        }
                    }

                    // 2. Prepare for heading processing
                    heading_text_buffer.clear();

                    // Pop lower/equal level headings
                    while let Some(&(stack_level, _)) = heading_stack.last() {
                    if stack_level >= level { heading_stack.pop(); } else { break; }
                    }
                    heading_stack.push((level, String::new())); // Placeholder title
                }

                // Heading End: Finalize the heading block itself
                Event::End(TagEnd::Heading(level)) => {
                    let level = level as u32;

                    // Update heading title in stack
                    let title = heading_text_buffer.trim().to_string();
                    if let Some((_, stack_title)) = heading_stack.last_mut() {
                        *stack_title = title.clone();
                    } else {
                        warn!(level, "Heading stack empty on End(Heading)");
                    }

                    // Update current path string
                    current_heading_path = Some(
                        heading_stack.iter()
                            .map(|(_, t)| t.as_str())
                            .collect::<Vec<&str>>()
                            .join(" > ")
                    );
                    debug!(path=?current_heading_path, "Updated heading path");

                // The heading's content range is from its start (tricky to get precisely
                // without tracking start event offset) to the end of the last Text/Code event inside it.
                // Let's approximate the heading block range from the *Start(Heading)* event's start
                // to *this End(Heading)* event's end for simplicity, OR use the range of text inside.
                // Simpler: Use the range of the contained text events if possible.
                if let Some(start) = current_block_start.take() {
                        let end = last_event_end; // End of last text/code within heading
                        if start < end {
                        // Finalize the heading text as its own chunk
                        finalize_range = Some(start..end);
                        trace!(range = ?finalize_range, "Finalizing Heading block");
                        } else {
                            trace!("Skipping empty heading block finalization");
                        }
                }
                // Reset buffer after use
                heading_text_buffer.clear();
                }

                // Rules (Thematic Breaks) act as separators
                Event::Rule => {
                    if let Some(start) = current_block_start.take() {
                        let end = range.start; // End block before the rule
                        if start < end {
                            finalize_range = Some(start..end);
                             trace!(range = ?finalize_range, "Finalizing block before Rule");
                        } else {
                            trace!("Skipping empty block finalization before Rule");
                        }
                    }
                    // Don't include the rule itself as a chunk in this logic
                }

                _ => {} // Other events don't trigger finalization
            }

            // --- Perform Finalization if Triggered ---
            if let Some(final_range) = finalize_range {
                // Get the text slice corresponding to the finalized range
                if let Some(block_text) = source_text.get(final_range.clone()) {
                    if !block_text.trim().is_empty() {
                        // Calculate token count (optional)
                        let token_count = self.count_tokens_optional(block_text);

                        debug!(range=?final_range, heading=?current_heading_path, tokens=?token_count, "Creating ChunkData");
                        results.push(ChunkData {
                            text_range: final_range,
                            // Use the heading path *active when the block ended*
                            heading_path: current_heading_path.clone(),
                            token_count,
                        });
                    } else {
                        trace!(range=?final_range, "Skipping ChunkData for whitespace-only block");
                    }
                } else {
                    // This indicates a logic error in range calculation
                    warn!(range=?final_range, "Invalid range calculated, skipping block.");
                    // Return error? Or just log and continue? Let's log for now.
                    // return Err(ChunkerError::Processing(format!("Invalid text range calculated: {:?}", final_range)));
                }

                // We just finalized a block (e.g., before a heading).
                // Reset the block start marker so the next text event starts a new one.
                current_block_start = None;

            }

            // Update the end offset of the last processed event
            last_event_end = range.end;

            // Special case: If the event itself should start a new block immediately after finalization
            // (e.g. the text right after a heading end tag), reset start offset tracking
            // This is implicitly handled by `current_block_start = None` above and setting it on Text/Code events.

        } // End event loop

        // --- Final Cleanup ---
        // Check if there's a pending block after the last event
        trace!("Processing any remaining block after loop");
        if let Some(start) = current_block_start {
            let end = last_event_end; // Use the end of the very last event processed
            if start < end {
                let final_range = start..end;
                if let Some(block_text) = source_text.get(final_range.clone()) {
                    if !block_text.trim().is_empty() {
                        let token_count = self.count_tokens_optional(block_text);
                        debug!(range=?final_range, heading=?current_heading_path, tokens=?token_count, "Creating final ChunkData");
                        results.push(ChunkData {
                            text_range: final_range,
                            heading_path: current_heading_path.clone(),
                            token_count,
                        });
                    } else {
                         trace!(range=?final_range, "Skipping final ChunkData for whitespace-only block");
                    }
                } else {
                    warn!(range=?final_range, "Invalid final range calculated, skipping block.");
                }
            }
        }

        debug!(num_chunks = results.len(), "Markdown chunking finished");
        Ok(results)
    }
}


// --- Unit Tests ---
#[cfg(test)]
mod tests {
    use super::*;
    use tracing_subscriber::{fmt, EnvFilter};

    fn setup_tracing() {
        let _ = fmt()
            .with_env_filter(EnvFilter::from_default_env())
            .with_test_writer()
            .try_init();
    }

    // Helper to get text from ChunkData for assertion checking
    fn get_text<'a>(source: &'a str, data: &ChunkData) -> &'a str {
        source.get(data.text_range.clone()).expect("Invalid range in test")
    }

    #[test]
    fn simple_paragraphs() {
        setup_tracing();
        let markdown = "First paragraph.\n\nSecond paragraph.";
        // Expect two chunks, one for each paragraph.
        let config = MarkdownChunkerConfig::default();
        let chunker = MarkdownChunker::new(config).unwrap();
        let chunks = chunker.chunk(markdown).unwrap();

        assert_eq!(chunks.len(), 2);
        assert_eq!(get_text(markdown, &chunks[0]), "First paragraph.");
        assert_eq!(get_text(markdown, &chunks[1]), "Second paragraph.");
        assert!(chunks[0].token_count.is_some());
        assert!(chunks[1].token_count.is_some());
        assert!(chunks[0].token_count.unwrap() > 0);
        assert!(chunks[1].token_count.unwrap() > 0);
        assert!(chunks[0].heading_path.is_none());
        assert!(chunks[1].heading_path.is_none());
    }

    #[test]
    fn paragraphs_with_breaks() {
        setup_tracing();
        // Soft breaks should be part of the paragraph block
        let markdown = "Line one.\nLine two.\n\nSecond paragraph.";
        let config = MarkdownChunkerConfig::default();
        let chunker = MarkdownChunker::new(config).unwrap();
        let chunks = chunker.chunk(markdown).unwrap();

        assert_eq!(chunks.len(), 2);
        // pulldown_cmark usually converts soft line breaks to spaces or newlines depending on context
        // Check the raw range content
        assert_eq!(get_text(markdown, &chunks[0]), "Line one.\nLine two."); // pulldown_cmark preserves internal newlines from soft breaks by default
        assert_eq!(get_text(markdown, &chunks[1]), "Second paragraph.");
    }

    #[test]
    fn heading_and_paragraphs() {
        setup_tracing();
        let markdown = "# Title\n\nIntro paragraph.\n\n## Section 1\n\nDetails here.";
        // Expect 4 chunks: Title, Intro, Section 1, Details
        let config = MarkdownChunkerConfig::default();
        let chunker = MarkdownChunker::new(config).unwrap();
        let chunks = chunker.chunk(markdown).unwrap();

        assert_eq!(chunks.len(), 4);

        // Chunk 0: Title heading text
        assert_eq!(get_text(markdown, &chunks[0]), "Title");
        assert_eq!(chunks[0].heading_path.as_deref(), Some("Title"));
        assert!(chunks[0].token_count.unwrap() > 0);

        // Chunk 1: Intro paragraph (under Title)
        assert_eq!(get_text(markdown, &chunks[1]), "Intro paragraph.");
        assert_eq!(chunks[1].heading_path.as_deref(), Some("Title")); // Path active before Section 1
        assert!(chunks[1].token_count.unwrap() > 0);

        // Chunk 2: Section 1 heading text
        assert_eq!(get_text(markdown, &chunks[2]), "Section 1");
        assert_eq!(chunks[2].heading_path.as_deref(), Some("Title > Section 1"));
        assert!(chunks[2].token_count.unwrap() > 0);

        // Chunk 3: Details paragraph (under Section 1)
        assert_eq!(get_text(markdown, &chunks[3]), "Details here.");
        assert_eq!(chunks[3].heading_path.as_deref(), Some("Title > Section 1"));
        assert!(chunks[3].token_count.unwrap() > 0);
    }

    #[test]
    fn code_block_chunk() {
        setup_tracing();
        let markdown = "Some text.\n\n```rust\nfn main() {\n  // comment\n}\n```\n\nMore text.";
        // Expect 3 chunks: Some text, the code block content, More text
        let config = MarkdownChunkerConfig::default();
        let chunker = MarkdownChunker::new(config).unwrap();
        let chunks = chunker.chunk(markdown).unwrap();

        assert_eq!(chunks.len(), 3);
        assert_eq!(get_text(markdown, &chunks[0]), "Some text.");
        // The code block content itself, excluding the fences
        assert_eq!(get_text(markdown, &chunks[1]), "fn main() {\n  // comment\n}\n");
        assert_eq!(get_text(markdown, &chunks[2]), "More text.");
        assert!(chunks[1].token_count.is_some());
    }

     #[test]
    fn list_item_chunks() {
        setup_tracing();
        let markdown = "* Item 1\n* Item 2\n  * Sub Item 2.1\n* Item 3";
        // Expect 4 chunks, one for each item's content.
        let config = MarkdownChunkerConfig::default();
        let chunker = MarkdownChunker::new(config).unwrap();
        let chunks = chunker.chunk(markdown).unwrap();

        assert_eq!(chunks.len(), 4);
        // Note: pulldown_cmark includes trailing newline in list item text events usually
        assert!(get_text(markdown, &chunks[0]).contains("Item 1"));
        assert!(get_text(markdown, &chunks[1]).contains("Item 2"));
        assert!(get_text(markdown, &chunks[2]).contains("Sub Item 2.1"));
        assert!(get_text(markdown, &chunks[3]).contains("Item 3"));
     }

    #[test]
    fn preserves_inline_markdown() {
        setup_tracing();
        let markdown = "This is **bold** and `code`.";
        // Expect one chunk for the paragraph
        let config = MarkdownChunkerConfig::default();
        let chunker = MarkdownChunker::new(config).unwrap();
        let chunks = chunker.chunk(markdown).unwrap();

        assert_eq!(chunks.len(), 1);
        // The raw text range includes the markdown characters
        assert_eq!(get_text(markdown, &chunks[0]), "This is **bold** and `code`.");
    }

    #[test]
    fn handles_empty_input() {
        setup_tracing();
        let markdown = "";
        let config = MarkdownChunkerConfig::default();
        let chunker = MarkdownChunker::new(config).unwrap();
        let chunks = chunker.chunk(markdown).unwrap();
        assert!(chunks.is_empty());
    }

    #[test]
    fn handles_whitespace_input() {
        setup_tracing();
        let markdown = "\n  \n\t\n";
        let config = MarkdownChunkerConfig::default();
        let chunker = MarkdownChunker::new(config).unwrap();
        let chunks = chunker.chunk(markdown).unwrap();
        // Should not produce chunks for only whitespace blocks
        assert!(chunks.is_empty());
    }
}