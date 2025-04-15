use std::ops::Range;

use serde::{Deserialize, Serialize};

use crate::{embedding::Embedding, extension::FunctionalityId};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ExtensionData {
    Embeddings(Vec<(Embedding, Range<usize>)>),
    Chunks(()),
}