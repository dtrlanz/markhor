//! Adapter exposing the Mistral [`Client`] to Markhor's extension system.
//!
//! This is the only module in `mistral` that uses Markhor's component traits. The `Converter`
//! trait is path-in, readers-out, and can't express remote documents, images or options, so the
//! converter here is a thin shim over [`Client::ocr`]; richer use cases should call the client
//! directly.

use std::{io::Cursor, path::PathBuf};

use async_trait::async_trait;
use markhor_core::{
    convert::{ConversionError, Converter},
    extension::Extension,
};
use mime::Mime;
use tokio::io::AsyncRead;

use super::{Client, DocumentSource, Error, OcrOptions};

const EXTENSION_URI: &str = "https://github.com/dtrlanz/markhor/tree/main/extensions/src/mistral";

#[async_trait]
impl Extension for Client {
    fn uri(&self) -> &str {
        EXTENSION_URI
    }

    fn name(&self) -> &str {
        "Mistral Client"
    }

    fn description(&self) -> &str {
        "Client for Mistral AI (OCR only so far)"
    }

    fn converters(&self) -> Vec<Box<dyn Converter>> {
        vec![Box::new(OcrConverter {
            client: self.clone(),
        })]
    }
}

/// Converts PDFs, Office documents and images to Markdown using Mistral OCR.
///
/// Produces one reader per page. Images are not extracted; the markdown keeps its image
/// placeholders (e.g. `![img-0.jpeg](img-0.jpeg)`).
struct OcrConverter {
    client: Client,
}

#[async_trait]
impl Converter for OcrConverter {
    async fn convert(
        &self,
        input: PathBuf,
        output_type: Mime,
    ) -> Result<Vec<Box<dyn AsyncRead + Unpin>>, ConversionError> {
        if output_type.essence_str() != "text/markdown" {
            return Err(ConversionError::UnsupportedMimeType(output_type));
        }

        let source = DocumentSource::from_path(&input)
            .await
            .map_err(conversion_error)?;
        let output = self
            .client
            .ocr(&source, &OcrOptions::default())
            .await
            .map_err(conversion_error)?;

        Ok(output
            .pages
            .into_iter()
            .map(|page| {
                Box::new(Cursor::new(page.markdown.into_bytes())) as Box<dyn AsyncRead + Unpin>
            })
            .collect())
    }
}

fn conversion_error(error: Error) -> ConversionError {
    match error {
        Error::Io(e) => ConversionError::IoError(e),
        // `ConversionError` has no richer variants yet.
        other => ConversionError::Other(Box::new(other)),
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;
    use tokio::io::AsyncReadExt;
    use wiremock::{Mock, MockServer, ResponseTemplate, matchers::method};

    use super::*;
    use crate::mistral::test_client;

    #[tokio::test]
    async fn converter_returns_one_reader_per_page() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "pages": [
                    {"index": 0, "markdown": "page one", "images": []},
                    {"index": 1, "markdown": "page two", "images": []},
                ],
                "model": "mistral-ocr-latest",
                "usage_info": {"pages_processed": 2},
            })))
            .mount(&server)
            .await;

        let dir = tempfile::tempdir().unwrap();
        let input = dir.path().join("scan.png");
        std::fs::write(&input, b"not really a png").unwrap();

        let converters = test_client(&server).converters();
        let readers = converters[0]
            .convert(
                input.clone(),
                "text/markdown; charset=utf-8".parse().unwrap(),
            )
            .await
            .unwrap();
        let mut texts = Vec::new();
        for mut reader in readers {
            let mut text = String::new();
            reader.read_to_string(&mut text).await.unwrap();
            texts.push(text);
        }
        assert_eq!(texts, ["page one", "page two"]);

        let unsupported = converters[0].convert(input, mime::TEXT_HTML).await;
        assert!(matches!(
            unsupported,
            Err(ConversionError::UnsupportedMimeType(_))
        ));
    }
}
