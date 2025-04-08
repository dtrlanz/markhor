use crate::{extension::Functionality, storage::Content};
use async_trait::async_trait;
use mime::Mime;
use thiserror::Error;
use tokio::io::AsyncRead;

#[async_trait]
pub trait Converter: Functionality {
    async fn convert(&self, input: Content, output_type: Mime) -> Result<Vec<Box<dyn AsyncRead + Unpin>>, ConversionError>;
}

#[derive(Debug, Error)]
pub enum ConversionError {
    #[error("IO Error: {0}")]
    IoError(std::io::Error),

    #[error("Target media type not supported: {0}")]
    UnsupportedMimeType(Mime),

    //ConversionFailed(String),

    #[error("Conversion failed: {0}")]
    Other(Box<dyn std::error::Error + Send + Sync>),
}

