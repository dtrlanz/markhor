use pulldown_cmark::{Event, Parser, Tag, TagEnd};
use std::error::Error;
use std::fmt;
use tiktoken_rs::{cl100k_base, CoreBPE};
use tracing::{debug, error, info, instrument, span, trace, warn, Level}; // Import tracing macros

pub mod chunker;

// --- Data Structures (Chunk, ChunkerConfig, ChunkerError) ---
#[derive(Debug, Clone)]
pub struct ChunkerConfig {
    pub max_tokens: usize,
    pub token_overlap: usize,
}

impl Default for ChunkerConfig {
    fn default() -> Self {
        ChunkerConfig {
            max_tokens: 512,
            token_overlap: 50,
        }
    }
}

#[derive(Debug, Clone)]
pub struct Chunk {
    pub source_id: String,
    pub chunk_index: usize,
    pub chunk_text: String,
    pub token_count: usize,
    pub heading_path: String,
}

#[derive(Debug, thiserror::Error)]
pub enum ChunkerError {
    #[error("Tokenization operation failed: {0}")]
    TokenizationError(String),

    #[error("Markdown parsing issue: {0}")]
    MarkdownParsing(String),

    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Configuration error: {0}")]
    Config(String),
}



// --- Main Chunker Function ---

#[instrument(skip_all, fields(source_id = %source_id, max_tokens = config.max_tokens, overlap = config.token_overlap))]
pub fn chunk_markdown<'a>(
    source_id: &str,
    markdown_text: &'a str,
    config: &ChunkerConfig,
) -> Result<Vec<Chunk>, ChunkerError> {
    info!("Starting markdown chunking");

    if config.token_overlap >= config.max_tokens && config.max_tokens > 0 {
        let err_msg = "Token overlap cannot be larger than or equal to max tokens".to_string();
        error!(max_tokens = config.max_tokens, token_overlap = config.token_overlap, "{}", err_msg);
        return Err(ChunkerError::Config(err_msg));
    }
     if config.max_tokens == 0 {
        let err_msg = "Max tokens must be greater than 0".to_string();
        error!("{}", err_msg);
        return Err(ChunkerError::Config(err_msg));
    }

    // --- Initialization ---
    trace!("Initializing tokenizer (cl100k_base)");

    // Map the anyhow::Error to our ChunkerError::TokenizationError
    let tokenizer = cl100k_base().map_err(|e| {
        // Log the original anyhow error for details
        error!("Failed to initialize tokenizer: {:?}", e);
        // Convert to our library's error type, capturing the message
        ChunkerError::TokenizationError(format!("Tokenizer initialization failed: {}", e))
    })?;

    let mut chunks: Vec<Chunk> = Vec::new();
    let mut current_chunk_text = String::new();
    let mut current_chunk_tokens: usize = 0;

    let mut current_block_buffer = String::new();
    let mut heading_stack: Vec<(u32, String)> = Vec::new();
    let mut current_heading_path = String::new();

    let parser = Parser::new(markdown_text);
    let mut chunk_index = 0;
    let mut last_finalized_chunk_text: Option<String> = None;

    // --- Main Parsing Loop ---
    trace!("Starting pulldown-cmark event processing loop");
    for event in parser {
        // Create a span for processing each event type, can be verbose
        let event_span = span!(Level::TRACE, "markdown_event", event = ?event);
        let _enter = event_span.enter();

        match event {
            Event::Start(tag) => {
                 trace!(tag = ?tag, "Processing Start tag");
                match tag {
                    Tag::Heading { level, .. } => {
                        finalize_previous_block(
                            &mut current_block_buffer,
                            &mut current_chunk_text,
                            &mut current_chunk_tokens,
                            &mut chunks,
                            &mut chunk_index,
                            &mut last_finalized_chunk_text,
                            source_id,
                            &mut current_heading_path,
                            config,
                            &tokenizer,
                        )?;

                        let level = level as u32;

                        while let Some(&(stack_level, _)) = heading_stack.last() {
                            if stack_level >= level {
                                trace!(level, stack_level, "Popping heading from stack");
                                heading_stack.pop();
                            } else {
                                break;
                            }
                        }
                        heading_stack.push((level, String::new())); // Title added on Text event
                    }
                    Tag::CodeBlock(kind) => {
                        finalize_previous_block(
                            &mut current_block_buffer,
                            &mut current_chunk_text,
                            &mut current_chunk_tokens,
                            &mut chunks,
                            &mut chunk_index,
                            &mut last_finalized_chunk_text,
                            source_id,
                            &mut current_heading_path,
                            config,
                            &tokenizer,
                        )?;
                        let lang = match kind {
                            pulldown_cmark::CodeBlockKind::Fenced(lang) => lang.into_string(),
                            pulldown_cmark::CodeBlockKind::Indented => "".to_string(),
                        };
                        trace!(language=?lang, "Starting code block");
                        current_block_buffer.push_str(&format!("\n```{}\n", lang));
                    }
                     Tag::List(_) => {
                         finalize_previous_block(
                            &mut current_block_buffer,
                            &mut current_chunk_text,
                            &mut current_chunk_tokens,
                            &mut chunks,
                            &mut chunk_index,
                            &mut last_finalized_chunk_text,
                            source_id,
                            &mut current_heading_path,
                            config,
                            &tokenizer,
                        )?;
                        if !current_block_buffer.is_empty() && !current_block_buffer.ends_with('\n') {
                           trace!("Adding newline before list start");
                           current_block_buffer.push('\n');
                        }
                     }
                    Tag::Item => {
                         if !current_block_buffer.is_empty() && !current_block_buffer.ends_with('\n') {
                            trace!("Adding newline before list item start");
                           current_block_buffer.push('\n');
                        }
                    }
                    _ => {} // Other tags don't require immediate block finalization
                }
            }
            Event::End(tag_end) => {
                 trace!(tag = ?tag_end, "Processing End tag");
                match tag_end {
                    TagEnd::Heading(level) => {
                        let level = level as u32;
                        if let Some((_, title)) = heading_stack.last_mut() {
                           // Assign collected text (in buffer) as heading title
                            let trimmed_title = current_block_buffer.trim().to_string();
                            trace!(level, title=%trimmed_title, "Finalizing heading");
                            *title = trimmed_title;
                        }
                        current_heading_path = generate_heading_path_string(&heading_stack);
                        debug!(path=%current_heading_path, "Updated heading path");

                        // Finalize the heading text itself as a block
                        finalize_previous_block(
                            &mut current_block_buffer,
                            &mut current_chunk_text,
                            &mut current_chunk_tokens,
                            &mut chunks,
                            &mut chunk_index,
                            &mut last_finalized_chunk_text,
                            source_id,
                            &current_heading_path, // Use the *new* path for the heading chunk
                            config,
                            &tokenizer,
                        )?;
                        current_block_buffer.clear(); // Clear buffer after processing heading text
                    }
                    TagEnd::Paragraph | TagEnd::Item | TagEnd::CodeBlock | TagEnd::List(_) => {
                         if tag_end == TagEnd::CodeBlock {
                            trace!("Ending code block");
                            current_block_buffer.push_str("```\n");
                         } else if !current_block_buffer.is_empty() && !current_block_buffer.ends_with("\n\n") {
                            trace!("Adding standard block separation (newlines)");
                             if !current_block_buffer.ends_with('\n') { current_block_buffer.push('\n');}
                             current_block_buffer.push('\n');
                         }

                        finalize_previous_block(
                            &mut current_block_buffer,
                            &mut current_chunk_text,
                            &mut current_chunk_tokens,
                            &mut chunks,
                            &mut chunk_index,
                            &mut last_finalized_chunk_text,
                            source_id,
                            &current_heading_path, // Path associated with the block's content
                            config,
                            &tokenizer,
                        )?;
                        current_block_buffer.clear();
                    }
                    _ => {}
                }
            }
            Event::Text(text) => {
                trace!(text_len = text.len(), "Appending text to block buffer");
                current_block_buffer.push_str(&text);
            }
            Event::Code(text) => {
                 trace!(text_len = text.len(), "Appending inline code to block buffer");
                current_block_buffer.push('`');
                current_block_buffer.push_str(&text);
                current_block_buffer.push('`');
            }
             Event::HardBreak | Event::SoftBreak => {
                 trace!("Handling line break");
                 if current_block_buffer.ends_with(' ') || current_block_buffer.ends_with('\n') {
                     // Avoid double spacing/newlines
                 } else if matches!(event, Event::HardBreak) {
                     current_block_buffer.push('\n');
                 } else { // SoftBreak
                     current_block_buffer.push(' ');
                 }
             }
             Event::Rule => {
                 trace!("Handling thematic break (rule)");
                finalize_previous_block(
                    &mut current_block_buffer,
                    &mut current_chunk_text,
                    &mut current_chunk_tokens,
                    &mut chunks,
                    &mut chunk_index,
                    &mut last_finalized_chunk_text,
                    source_id,
                    &current_heading_path,
                    config,
                    &tokenizer,
                )?;
                 current_block_buffer.clear(); // Rule itself doesn't add text here
             }
            _ => {} // Ignore other events for now
        }
    } // End event loop

    // --- Final Cleanup ---
    trace!("Processing any remaining text in block buffer after loop");
    finalize_previous_block(
        &mut current_block_buffer,
        &mut current_chunk_text,
        &mut current_chunk_tokens,
        &mut chunks,
        &mut chunk_index,
        &mut last_finalized_chunk_text,
        source_id,
        &current_heading_path,
        config,
        &tokenizer,
    )?;

    trace!("Processing any remaining text in the current chunk text");
    if !current_chunk_text.is_empty() {
        let final_tokens = count_tokens(&current_chunk_text, &tokenizer).map_err(|e| {
            error!("Tokenization failed for final chunk: {}", e); // Log specific error context
            e
        })?;
        if final_tokens > 0 {
            let final_text = current_chunk_text.trim().to_string();
            debug!(chunk_index, token_count = final_tokens, text_len = final_text.len(), "Creating final chunk");
            chunks.push(Chunk {
                source_id: source_id.to_string(),
                chunk_index,
                chunk_text: final_text,
                token_count: final_tokens,
                heading_path: current_heading_path.clone(),
            });
        } else {
             trace!("Skipping final chunk creation as it became empty after trimming");
        }
    }

    info!(num_chunks = chunks.len(), "Markdown chunking finished");
    Ok(chunks)
}

// --- Helper Functions ---

#[instrument(skip_all, fields(block_len = block_buffer.len(), current_tokens = *current_chunk_tokens))]
fn finalize_previous_block(
    block_buffer: &mut String,
    current_chunk_text: &mut String,
    current_chunk_tokens: &mut usize,
    chunks: &mut Vec<Chunk>,
    chunk_index: &mut usize,
    last_finalized_chunk_text: &mut Option<String>,
    source_id: &str,
    heading_path: &str,
    config: &ChunkerConfig,
    tokenizer: &CoreBPE,
) -> Result<(), ChunkerError> {
    trace!("Entering finalize_previous_block");

    if block_buffer.is_empty() {
         trace!("Block buffer is empty, nothing to finalize.");
        return Ok(());
    }

    let block_text = block_buffer.trim_start();
    if block_text.is_empty() {
         trace!("Block buffer contained only whitespace, clearing.");
        block_buffer.clear();
        return Ok(());
    }

    trace!(block_text_preview = %ellipsize(block_text, 50), "Processing block");
    let block_tokens = count_tokens(block_text, tokenizer).map_err(|e| {
         error!("Tokenization failed for block: {}", e);
         e
    })?;
     trace!(block_tokens, "Calculated block token count");


    // Scenario 1: Block fits entirely into the current chunk
    if *current_chunk_tokens == 0 || (*current_chunk_tokens + block_tokens <= config.max_tokens) {
         trace!("Attempting to fit block into current chunk");
        // Handle potential overlap if starting a new chunk
        if *current_chunk_tokens == 0 && config.token_overlap > 0 {
            if let Some(last_text) = last_finalized_chunk_text {
                 trace!(overlap_tokens = config.token_overlap, "Calculating overlap from previous chunk");
                 let overlap_text = calculate_overlap_text(last_text, config.token_overlap, tokenizer)?;
                 if !overlap_text.is_empty() {
                     let overlap_tokens = count_tokens(&overlap_text, tokenizer)?;
                     trace!(overlap_tokens, overlap_text_preview = %ellipsize(&overlap_text, 50), "Applying overlap");
                     if overlap_tokens < config.max_tokens {
                         current_chunk_text.push_str(&overlap_text);
                         if !overlap_text.ends_with('\n') && !overlap_text.ends_with(' ') {
                             current_chunk_text.push('\n');
                         }
                         *current_chunk_tokens = overlap_tokens;
                         trace!(current_chunk_tokens = *current_chunk_tokens, "Chunk tokens after adding overlap");
                     } else {
                         warn!(overlap_tokens, max_tokens=config.max_tokens, "Calculated overlap exceeds max tokens, skipping overlap.");
                     }
                 } else {
                      trace!("Overlap calculation resulted in empty string.");
                 }
            } else {
                 trace!("No previous finalized chunk text to calculate overlap from.");
            }
        }

        // Check again if adding the block *with potential overlap* still fits
        if *current_chunk_tokens + block_tokens <= config.max_tokens {
            trace!("Block fits, appending to current chunk.");
            current_chunk_text.push_str(block_text);
            *current_chunk_tokens += block_tokens;
            block_buffer.clear();
            return Ok(());
        } else {
            trace!("Block does not fit even after handling overlap, proceeding to finalize/split.");
        }
    }

    // Scenario 2: Block does not fit, finalize the current chunk (if any)
    if *current_chunk_tokens > 0 {
        let final_text = current_chunk_text.trim().to_string();
        if !final_text.is_empty() {
             debug!(chunk_index = *chunk_index, token_count = *current_chunk_tokens, text_len = final_text.len(), heading_path=%heading_path, "Finalizing chunk before processing oversized block");
            chunks.push(Chunk {
                source_id: source_id.to_string(),
                chunk_index: *chunk_index,
                chunk_text: final_text.clone(),
                token_count: *current_chunk_tokens,
                heading_path: heading_path.to_string(),
            });
            *chunk_index += 1;
            *last_finalized_chunk_text = Some(final_text);
        } else {
             trace!("Current chunk text was empty or whitespace, not creating chunk.");
        }
    } else {
         trace!("Current chunk is empty, no chunk to finalize yet.");
    }

    // Clear current chunk state before processing the block that didn't fit
    trace!("Resetting current chunk text and token count.");
    current_chunk_text.clear();
    *current_chunk_tokens = 0;

    // Calculate overlap based on the chunk we *just* finalized (if any)
    let overlap_text = if config.token_overlap > 0 {
        if let Some(last_text) = last_finalized_chunk_text {
             trace!(overlap_tokens = config.token_overlap, "Calculating overlap for the split block");
            calculate_overlap_text(last_text, config.token_overlap, tokenizer)?
        } else {
            trace!("No previous chunk to get overlap from for split block.");
            String::new()
        }
    } else {
        trace!("Overlap is disabled.");
        String::new()
    };

    // Scenario 3: Handle the block that didn't fit. It might need splitting.
    trace!("Calling split_and_process_block for the oversized block.");
    split_and_process_block(
        block_text,
        overlap_text,
        current_chunk_text,
        current_chunk_tokens,
        chunks,
        chunk_index,
        last_finalized_chunk_text,
        source_id,
        heading_path,
        config,
        tokenizer,
    )?;

    block_buffer.clear();
    trace!("Exiting finalize_previous_block");
    Ok(())
}

#[instrument(skip_all, fields(block_len = block_text.len(), initial_overlap_len = initial_overlap.len()))]
fn split_and_process_block(
    block_text: &str,
    initial_overlap: String,
    current_chunk_text: &mut String,
    current_chunk_tokens: &mut usize,
    chunks: &mut Vec<Chunk>,
    chunk_index: &mut usize,
    last_finalized_chunk_text: &mut Option<String>,
    source_id: &str,
    heading_path: &str,
    config: &ChunkerConfig,
    tokenizer: &CoreBPE,
) -> Result<(), ChunkerError> {
    trace!("Entering split_and_process_block");

    let mut processed_offset = 0;

    // Prepend initial overlap if starting a new chunk context here
    if !initial_overlap.is_empty() {
        let overlap_tokens = count_tokens(&initial_overlap, tokenizer)?;
         trace!(overlap_tokens, overlap_text_preview = %ellipsize(&initial_overlap, 50), "Applying initial overlap for split block");
        if overlap_tokens < config.max_tokens {
            current_chunk_text.push_str(&initial_overlap);
            if !initial_overlap.ends_with('\n') && !initial_overlap.ends_with(' ') {
                current_chunk_text.push('\n');
            }
            *current_chunk_tokens = overlap_tokens;
             trace!(current_chunk_tokens = *current_chunk_tokens, "Chunk tokens after initial overlap");
        } else {
             warn!(overlap_tokens, max_tokens=config.max_tokens, "Initial overlap for split block exceeds max tokens, skipping.");
        }
    }

    let avg_chars_per_token = 4.0;

    while processed_offset < block_text.len() {
        let loop_span = span!(Level::TRACE, "split_loop", processed_offset, current_tokens = *current_chunk_tokens);
        let _enter = loop_span.enter();

        let remaining_text = &block_text[processed_offset..];
         trace!(remaining_len = remaining_text.len(), "Processing remaining text in block");

        // Estimate character count needed to fill the chunk
        let target_chars = ((config.max_tokens.saturating_sub(*current_chunk_tokens)) as f64 * avg_chars_per_token).max(1.0) as usize;
        trace!(target_chars, "Calculated target characters for next segment");

        // Find character boundary (approximate split point)
        let mut char_boundary = remaining_text
            .char_indices()
            .skip(1)
            .find(|(idx, _)| *idx >= target_chars)
            .map(|(idx, _)| idx)
            .unwrap_or(remaining_text.len());
         trace!(char_boundary, "Initial character boundary determined");

        // Try to find a better boundary (whitespace) near the target
        if char_boundary < remaining_text.len() {
             if let Some(ws_pos) = remaining_text[..char_boundary].rfind(|c: char| c.is_whitespace()) {
                 // Arbitrary threshold: if whitespace is within ~25% of target back from boundary
                 if char_boundary - ws_pos < target_chars / 4 {
                      trace!(old_boundary = char_boundary, new_boundary = ws_pos + 1, "Adjusting boundary to whitespace");
                     char_boundary = ws_pos + 1;
                 }
             }
        }

        let text_segment = &remaining_text[..char_boundary];
        let segment_tokens = count_tokens(text_segment, tokenizer)?;
         trace!(segment_len = text_segment.len(), segment_tokens, "Calculated segment token count");

        // If segment fits in the current chunk
        if *current_chunk_tokens + segment_tokens <= config.max_tokens {
            trace!("Segment fits, appending to current chunk");
            current_chunk_text.push_str(text_segment);
            *current_chunk_tokens += segment_tokens;
            processed_offset += text_segment.len();

            if processed_offset == block_text.len() {
                trace!("Entire block processed");
                break; // Consumed the whole block
            }
        } else {
            // Segment does NOT fit
            trace!("Segment does not fit (current_tokens={}, segment_tokens={}, max_tokens={})", *current_chunk_tokens, segment_tokens, config.max_tokens);

            // 1. Finalize the current chunk if it has content
            if *current_chunk_tokens > 0 {
                let final_text = current_chunk_text.trim().to_string();
                if !final_text.is_empty() {
                    debug!(chunk_index = *chunk_index, token_count = *current_chunk_tokens, text_len = final_text.len(), heading_path=%heading_path, "Finalizing chunk during split");
                    chunks.push(Chunk {
                        source_id: source_id.to_string(),
                        chunk_index: *chunk_index,
                        chunk_text: final_text.clone(),
                        token_count: *current_chunk_tokens,
                        heading_path: heading_path.to_string(),
                    });
                    *chunk_index += 1;
                    *last_finalized_chunk_text = Some(final_text);
                } else {
                    trace!("Current chunk text was empty or whitespace, not creating chunk during split");
                }
            } else {
                 trace!("Current chunk is empty, cannot finalize");
            }

            // 2. Start new chunk: Reset state and calculate overlap
             trace!("Resetting chunk text/tokens for next segment");
            current_chunk_text.clear();
            *current_chunk_tokens = 0;
            let overlap_text = if config.token_overlap > 0 {
                if let Some(last_text) = last_finalized_chunk_text {
                    trace!("Calculating overlap for next chunk in split");
                    calculate_overlap_text(last_text, config.token_overlap, tokenizer)?
                } else { String::new() }
            } else { String::new() };

            if !overlap_text.is_empty() {
                let overlap_tokens = count_tokens(&overlap_text, tokenizer)?;
                trace!(overlap_tokens, overlap_text_preview = %ellipsize(&overlap_text, 50),"Applying overlap for next chunk in split");
                if overlap_tokens < config.max_tokens {
                    current_chunk_text.push_str(&overlap_text);
                    if !overlap_text.ends_with('\n') && !overlap_text.ends_with(' ') {
                        current_chunk_text.push('\n');
                    }
                    *current_chunk_tokens = overlap_tokens;
                     trace!(current_chunk_tokens = *current_chunk_tokens, "Chunk tokens after applying overlap");
                } else {
                     warn!(overlap_tokens, max_tokens=config.max_tokens, "Overlap calculated during split exceeds max tokens, skipping.");
                }
            }

            // 3. Re-evaluate if the *current segment* fits into the *new* chunk (with overlap)
             trace!("Re-evaluating segment fit after starting new chunk with overlap");
            if *current_chunk_tokens + segment_tokens <= config.max_tokens {
                trace!("Segment now fits into the new chunk");
                current_chunk_text.push_str(text_segment);
                *current_chunk_tokens += segment_tokens;
                processed_offset += text_segment.len();
            } else {
                // Edge Case: Segment *still* doesn't fit, even in a new chunk (potentially with overlap).
                // This happens if segment_tokens > (max_tokens - overlap_tokens), or if segment_tokens > max_tokens
                warn!(segment_tokens, current_chunk_tokens = *current_chunk_tokens, max_tokens = config.max_tokens, "Segment is too large even for a new chunk. Adding anyway (may exceed limit).");

                // Only add if the chunk is empty (or just contains overlap) to prevent combining two oversized segments.
                 // If current chunk has content (overlap), finalize *that* first? No, the overlap should be part of the chunk *with* this segment.
                 // Add the segment, accepting it will violate max_tokens for this chunk.
                current_chunk_text.push_str(text_segment);
                *current_chunk_tokens += segment_tokens;
                processed_offset += text_segment.len();

                // Finalize this single oversized chunk immediately
                let final_text = current_chunk_text.trim().to_string();
                if !final_text.is_empty() {
                     debug!(chunk_index = *chunk_index, token_count = *current_chunk_tokens, text_len = final_text.len(), heading_path=%heading_path, "Finalizing oversized chunk immediately");
                    chunks.push(Chunk {
                        source_id: source_id.to_string(),
                        chunk_index: *chunk_index,
                        chunk_text: final_text.clone(),
                        token_count: *current_chunk_tokens, // Report actual (oversized) count
                        heading_path: heading_path.to_string(),
                    });
                    *chunk_index += 1;
                    *last_finalized_chunk_text = Some(final_text);
                }
                // Reset for the *next* segment (if any)
                 trace!("Resetting chunk text/tokens after finalizing oversized chunk");
                current_chunk_text.clear();
                *current_chunk_tokens = 0;
                // Overlap for the *next* chunk will be calculated in the next loop iteration or finalize_previous_block
            }
        }
    } // End while loop processing block_text

    trace!("Exiting split_and_process_block");
    Ok(())
}


/// Token counting helper - currently infallible despite Result signature
#[inline]
fn count_tokens(text: &str, tokenizer: &CoreBPE) -> Result<usize, ChunkerError> {
    trace!(text_len=text.len(), "Counting tokens");
    Ok(tokenizer.encode_with_special_tokens(text).len())
}

/// Generates the "Path > To > Heading" string from the stack.
fn generate_heading_path_string(stack: &[(u32, String)]) -> String {
    let path = stack
        .iter()
        .map(|(_, title)| title.as_str())
        .filter(|s| !s.is_empty()) // Avoid empty titles if parsing logic allows them temporarily
        .collect::<Vec<&str>>()
        .join(" > ");
     trace!(path=%path, num_levels=stack.len(), "Generated heading path string");
     path
}

/// Calculates the overlap text from the end of the previous chunk using tokenization.
#[instrument(skip_all, fields(last_chunk_len = last_chunk_text.len(), overlap_tokens))]
fn calculate_overlap_text(
    last_chunk_text: &str,
    overlap_tokens: usize,
    tokenizer: &CoreBPE,
) -> Result<String, ChunkerError> {
    trace!("Calculating overlap text");
    if overlap_tokens == 0 || last_chunk_text.is_empty() {
        trace!("Overlap is zero or last chunk empty, returning empty string.");
        return Ok(String::new());
    }

    trace!("Tokenizing last chunk text for overlap calculation");
    let tokens = tokenizer.encode_with_special_tokens(last_chunk_text); // Assuming infallible encode
        // .map_err(|e| ChunkerError::Tokenization(e.into()))?; // If encode could fail

    if tokens.is_empty() {
        trace!("Last chunk text produced no tokens.");
        return Ok(String::new());
    }

    if tokens.len() <= overlap_tokens {
        trace!(num_tokens = tokens.len(), "Overlap size is >= chunk size, returning whole chunk as overlap.");
        // Return the whole text as overlap. Alternatively, could return "" or error?
        // Returning whole text seems reasonable for RAG context preservation.
        return Ok(last_chunk_text.to_string());
    }

    let overlap_token_slice = &tokens[tokens.len() - overlap_tokens..];
     trace!(slice_len = overlap_token_slice.len(), "Decoding token slice for overlap");
    let overlap_text = tokenizer.decode(overlap_token_slice.into()).map_err(|e| {
        error!("Failed to decode overlap token slice: {:?}", e);
        ChunkerError::TokenizationError(format!("Token decoding failed: {}", e))
    })?;

    // Trim potential leading/trailing whitespace artifacts from decoding partial tokens
    let trimmed_overlap = overlap_text.trim();
    trace!(overlap_text_len = trimmed_overlap.len(), "Overlap calculation complete");
    Ok(trimmed_overlap.to_string())
}

/// Utility to ellipsize long strings for logging
fn ellipsize(s: &str, max_len: usize) -> String {
    if s.len() <= max_len {
        s.to_string()
    } else {
        format!("{}...", &s[..max_len.saturating_sub(3)])
    }
}


// --- Unit Tests ---
#[cfg(test)]
mod tests {
    use super::*;
    use tracing_subscriber::{fmt, EnvFilter}; // For setting up tracing in tests

    // Helper to initialize tracing subscriber once for all tests in this module
    fn setup_tracing() {
        // Using try_init() is important if tests run in parallel or if logging is initialized elsewhere
        let _ = fmt()
            .with_env_filter(EnvFilter::from_default_env()) // Control level with RUST_LOG=trace/debug/info
            .with_test_writer() // Write to test output
            .try_init();
    }


    #[test]
    fn simple_paragraph_chunking() {
        setup_tracing(); // Initialize tracing for this test run
        let markdown = "This is the first paragraph.\n\nThis is the second paragraph, which is a bit longer.";
        let config = ChunkerConfig { max_tokens: 10, token_overlap: 2, ..Default::default() };
        let chunks = chunk_markdown("doc1", markdown, &config).unwrap();

        // No println needed, check assertions and tracing output (if RUST_LOG is set)
        assert!(chunks.len() >= 2);

        assert_eq!(chunks[0].chunk_text, "This is the first paragraph.");
        assert_eq!(chunks[0].token_count, 7);

        assert!(chunks[1].chunk_text.contains("This is the second"));
        assert!(chunks[1].token_count <= config.max_tokens + 1); // Allow slight overshoot potential
                                                                 // Check overlap calculation more directly if possible, or subsequent chunks
        // Example: Overlap for chunk 1 from chunk 0 ("This is the first paragraph.")
        // Tokens (~7): [This, is, the, first, paragraph, .]
        // Overlap (2): [paragraph, .] -> Decodes to " paragraph." or similar
        let expected_overlap_start = " paragraph."; // Based on cl100k_base
        assert!(chunks[1].chunk_text.trim_start().starts_with(expected_overlap_start.trim_start()) || chunks[1].chunk_text.trim_start().starts_with("aph.")); // Allow for tokenization quirks


        if chunks.len() > 2 {
             assert!(chunks[2].chunk_text.contains("longer"));
             assert!(chunks[2].token_count <= config.max_tokens + 1);
        }
    }

    #[test]
    fn heading_handling() {
         setup_tracing();
        let markdown = "# Title\n\nFirst paragraph.\n\n## Section 1\n\nSecond paragraph.";
        let config = ChunkerConfig { max_tokens: 20, token_overlap: 3, ..Default::default() };
        let chunks = chunk_markdown("doc2", markdown, &config).unwrap();

        assert!(chunks.len() > 1);
        assert_eq!(chunks[0].heading_path, "Title");
        assert!(chunks[0].chunk_text.contains("Title"));
        assert!(chunks[0].chunk_text.contains("First paragraph"));

        let chunk_with_section_heading = chunks.iter().find(|c| c.chunk_text.contains("Section 1")).expect("Section 1 heading not found");
        assert_eq!(chunk_with_section_heading.heading_path, "Title > Section 1");

        let chunk_with_second_para = chunks.iter().find(|c| c.chunk_text.contains("Second paragraph")).expect("Second paragraph not found");
        assert_eq!(chunk_with_second_para.heading_path, "Title > Section 1");
    }

    #[test]
    fn code_block_handling() {
        setup_tracing();
        let markdown = "Some text.\n\n```rust\nfn main() {\n  println!(\"Hello\");\n}\n```\n\nMore text.";
        let config = ChunkerConfig { max_tokens: 15, token_overlap: 4, ..Default::default() };
        let chunks = chunk_markdown("doc3", markdown, &config).unwrap();

        let code_chunk = chunks.iter().find(|c| c.chunk_text.contains("fn main()")).expect("Code not found");
        assert!(code_chunk.chunk_text.contains("```rust"));
        assert!(code_chunk.chunk_text.contains("```")); // Check for closing fence too
    }

    #[test]
    fn large_block_splitting() {
         setup_tracing();
        let long_line = "This is a very long line that keeps going and going designed to exceed the token limit all by itself. ".repeat(10);
        let markdown = format!("# Section\n\n{}", long_line);
        let config = ChunkerConfig { max_tokens: 30, token_overlap: 5, ..Default::default() };
        let chunks = chunk_markdown("doc4", &markdown, &config).unwrap();

        assert!(chunks.len() > 2);

        // Check token limits, allowing for the "add oversized segment anyway" logic
        for chunk in chunks.iter() {
            if chunk.token_count > config.max_tokens {
                 warn!(chunk.chunk_index, chunk.token_count, config.max_tokens, "Chunk exceeded max_tokens (potentially expected for oversized segments)");
                 // It should generally not exceed max_tokens *by a lot* unless a single segment was huge
                 // Let's allow up to ~double for extreme edge cases with large tokens/small limit
                 assert!(chunk.token_count < config.max_tokens * 2, "Chunk {} significantly exceeded max_tokens", chunk.chunk_index);
            }
        }

        // Check for overlap presence
        if chunks.len() > 2 {
            // Find first two chunks likely containing parts of the long line
             let first_long_chunk_idx = chunks.iter().position(|c| c.chunk_text.contains("very long line")).unwrap_or(0);
             if first_long_chunk_idx + 1 < chunks.len() {
                let text1 = &chunks[first_long_chunk_idx].chunk_text;
                let text2 = &chunks[first_long_chunk_idx + 1].chunk_text;
                let overlap_tokens = config.token_overlap;

                let expected_overlap = calculate_overlap_text(text1, overlap_tokens, &cl100k_base().unwrap()).unwrap();
                trace!(text1_preview = %ellipsize(text1, 50), text2_preview = %ellipsize(text2, 50), expected_overlap_preview = %ellipsize(&expected_overlap, 50), "Checking overlap between large block splits");

                assert!(!expected_overlap.is_empty(), "Overlap should not be empty here");
                assert!(text2.trim_start().starts_with(&expected_overlap), "Chunk 2 should start with overlap from chunk 1");
            }
        }
    }

    #[test]
    fn test_config_errors() {
         setup_tracing();
        let markdown = "Some text";
        let config_overlap_too_big = ChunkerConfig { max_tokens: 10, token_overlap: 10, ..Default::default() };
        let result = chunk_markdown("doc_cfg", markdown, &config_overlap_too_big);
        assert!(matches!(result, Err(ChunkerError::Config(_))));

        let config_zero_tokens = ChunkerConfig { max_tokens: 0, token_overlap: 0, ..Default::default() };
        let result = chunk_markdown("doc_cfg", markdown, &config_zero_tokens);
        assert!(matches!(result, Err(ChunkerError::Config(_))));
    }

}