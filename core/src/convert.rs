use crate::{extension::Functionality, storage::{Content, ContentBuilder}};
use async_trait::async_trait;
use mime::Mime;

#[async_trait]
pub trait Converter: Functionality {
    async fn convert(&self, input: Content, output_type: Mime) -> Result<ContentBuilder, ConversionError>;
}



pub enum ConversionError {
    IoError(std::io::Error),
    UnsupportedMimeType(Mime),
    ConversionFailed(String),
    Other(Box<dyn std::error::Error + Send + Sync>),
}

