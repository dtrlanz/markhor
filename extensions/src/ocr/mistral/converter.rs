use std::{path::PathBuf, sync::Arc};

use async_trait::async_trait;
use markhor_core::{convert::{ConversionError, Converter}};
use mime::Mime;
use tokio::io::AsyncRead;

use super::client::{MistralClientInner, URI};


pub struct MistralOcr(pub(crate) Arc<MistralClientInner>);

#[async_trait]
impl Converter for MistralOcr {
    async fn convert(&self, input: PathBuf, output_type: Mime) -> Result<Vec<Box<dyn AsyncRead + Unpin>>, ConversionError> {
        // Check if markdown is expected
        if output_type != "text/markdown" {
            return Err(ConversionError::UnsupportedMimeType(output_type));
        }

        // just yolo the error conversions for now
        // TODO: fix this once `ConversionError` is implemented properly
        let dir = tempfile::tempdir().map_err(|e| ConversionError::Other(Box::new(e)))?;
        let output_path = dir.path().join("output.md");

        self.0.ocr_file_to_markdown(input, &*output_path).await.map_err(|e| ConversionError::Other(Box::new(e)))?;

        let file = tokio::fs::File::open(&output_path).await.map_err(|e| ConversionError::IoError(e))?;
        let reader = tokio::io::BufReader::new(file);

        Ok(vec![Box::new(reader)])
    }
}
