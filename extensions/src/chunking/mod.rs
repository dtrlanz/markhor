use markhor_core::extension::Extension;
use plaintext::PlainTextChunker;
use tracing::error;

pub mod plaintext;


pub struct Chunkers;

impl Extension for Chunkers {
    fn name(&self) -> &str {
        "chunkers"
    }

    fn description(&self) -> &str {
        "Chunking extension"
    }

    fn uri(&self) -> &str {
        "TODO chunker uri"
    }

    fn chunker(&self) -> Option<Box<dyn markhor_core::chunking::Chunker>> {
        // match markdown::chunker::MarkdownChunker::new(Default::default()) {
        //     Ok(chunker) => Some(std::sync::Arc::new(chunker)),
        //     Err(e) => {
        //         error!("Error creating chunker: {:?}", e);
        //         None
        //     }
        // }

        let chunker = PlainTextChunker::new(2000, 200).unwrap();
        Some(Box::new(chunker))
    }
}