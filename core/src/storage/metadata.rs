use serde::{Deserialize, Serialize};
use uuid::Uuid;

// Not public, managed internally by the Document struct
#[derive(Serialize, Deserialize, Debug, Clone)]
pub(crate) struct DocumentMetadata {
    pub(crate) id: Uuid,
    // Add other metadata fields here as needed
}

impl DocumentMetadata {
    pub fn new() -> Self {
        DocumentMetadata { id: Uuid::new_v4() }
    }
}


pub(crate) struct ChunkMetadata {
    
}