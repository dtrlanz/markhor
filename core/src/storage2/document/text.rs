use sha2::{Digest, Sha256};
use std::{collections::HashMap, ops::{Deref, DerefMut}, sync::OnceLock};

use tracing::{debug, info, instrument, trace, warn};

use crate::{chunking::{Chunker, ChunkerError}, extension::F11y, markdown::{ToMarkdown, WITH_MILESTONES, WITHOUT_XML}, storage2::{HashValue, document::chunks::ChunkCache}};

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Text {
    hash: OnceLock<HashValue>,
    parts: Vec<Part>,
}

impl Text {
    pub fn new() -> Self {
        Self {
            hash: OnceLock::new(),
            parts: Vec::new(),
        }
    }

    pub fn hash(&self) -> HashValue {
        self.hash.get_or_init(|| {
            let mut hasher = Sha256::new();
            for part in &self.parts {
                hasher.update(part.id.as_bytes());
                hasher.update(part.content.as_bytes());
            }
            HashValue { value: hasher.finalize().into() }
        }).clone()
    }

    pub fn parts(&self) -> &[Part] {
        &self.parts
    }

    pub fn parts_mut(&mut self) -> &mut [Part] {
        // Text may change, so invalidate the hash
        self.hash = OnceLock::new();
        &mut self.parts
    }

    pub fn import(&mut self, source_str: Option<&str>, keyword: Option<&str>) {
        // Invalidate hash
        self.hash = OnceLock::new();

        match (source_str, keyword) {
            // Text only has one part
            (Some(text), None) => {
                self.parts = vec![Part {
                    id: String::new(),
                    content: text.to_string(),
                }];
            },
            // Text has multiple parts divided by XML milestones
            (Some(text), Some(_parts)) => {
                self.parts.clear();
                for r in text.to_markdown(WITH_MILESTONES).regions() {
                    if &*r.unit == "part" {
                        let id = r.attribute("id").flatten().map(|s| s.to_string()).unwrap_or_default();
                        if self.parts.iter().any(|part| part.id == id) {
                            warn!("Duplicate text part id '{}', skipping", id);
                            continue;
                        }
                        self.parts.push(Part {
                            id,
                            content: r.content().to_string(),
                        });
                    }
                }
            },
            // No text
            (None, _) => {
                self.parts.clear();
            },
        }
    }

    pub fn export(&self, _keyword: &mut Option<String>) -> Option<String> {
        if self.parts.is_empty() {
            None
        } else if self.parts.len() == 1 && self.parts[0].id.is_empty() {
            Some(self.parts[0].content.clone())
        } else {
            let mut text = String::new();
            for Part { id, content } in self.parts.iter() {
                text.push_str(content.to_markdown(WITHOUT_XML)
                    .prepend_milestone("part".into(), id, vec![]).as_ref());
                text.push_str("\n");
            }
            Some(text)
        }
    }
}

impl FromIterator<Part> for Text {
    fn from_iter<T: IntoIterator<Item = Part>>(iter: T) -> Self {
        let mut text = Text::new();
        text.parts = iter.into_iter().collect();
        text
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Hash)]
pub struct Part {
    id: String,
    content: String,
}

impl Part {
    pub fn new(id: impl Into<String>, content: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            content: content.into(),
        }
    }

    pub fn id(&self) -> &str {
        &self.id
    }

    #[instrument(skip(self, chunker, cached_chunks))]
    pub(crate) fn validate_cached_chunks(&self, chunker: &F11y<dyn Chunker>, cached_chunks: &mut Vec<ChunkCache>) -> Result<(), ChunkerError> {
        let chunks = chunker.chunk(&self.content)?;
        debug!("Generated {} chunks for text part '{}'", chunks.len(), self.id);
        cached_chunks.truncate(chunks.len());
        for (idx, chunk) in chunks.into_iter().enumerate() {
            let chunk_text = &self.content[chunk.text_range.clone()];
            let hash = HashValue::from(chunk_text);
            if idx < cached_chunks.len() {
                // Chunk has been cached before; check equality
                if cached_chunks[idx].chunk != chunk || cached_chunks[idx].hash != hash {
                    // Cache is outdated, update & drop invalid embeddings
                    cached_chunks[idx] = ChunkCache {
                        chunk,
                        hash,
                        embeddings: HashMap::new(),
                    };
                }
            } else {
                // New chunk, add to cache
                cached_chunks.push(ChunkCache {
                    chunk: chunk,
                    hash,
                    embeddings: HashMap::new(),
                });
            }
        }
        Ok(())
    }
}

impl Deref for Part {
    type Target = String;

    fn deref(&self) -> &String {
        &self.content
    }
}

impl DerefMut for Part {
    fn deref_mut(&mut self) -> &mut String {
        &mut self.content
    }
}
