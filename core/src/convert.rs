use crate::storage::{Content, ContentBuilder};
use mime::Mime;

#[dynosaur::dynosaur(pub DynConverter)]
pub trait Converter {
    async fn convert(&self, input: Content, output_type: Mime) -> Result<ContentBuilder, ConversionError>;
}



pub enum ConversionError {
    IoError(std::io::Error),
    UnsupportedMimeType(Mime),
    ConversionFailed(String),
}