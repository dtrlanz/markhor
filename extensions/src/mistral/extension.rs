//! Adapter exposing the Mistral [`Client`] to Markhor's extension system.
//!
//! This is the only module in `mistral` that uses Markhor's component traits. The adapter forwards
//! documents to the client, and passes URLs to Mistral so that it can fetch them directly. Richer
//! use cases should call the client directly.

use async_trait::async_trait;
use markhor_core::{
    extension::Extension,
    ocr::{self as ocr_core, OcrModel},
};
use mime::Mime;

use super::{Client, DocumentSource, Error};

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

    fn ocr_models(&self) -> Vec<Box<dyn OcrModel>> {
        vec![Box::new(MistralOcr {
            client: self.clone(),
        })]
    }
}

/// Runs Mistral OCR behind Markhor's [`OcrModel`] trait.
struct MistralOcr {
    client: Client,
}

impl MistralOcr {
    async fn run(
        &self,
        source: &DocumentSource,
        options: &ocr_core::OcrOptions,
    ) -> Result<ocr_core::OcrOutput, ocr_core::OcrError> {
        let options = super::OcrOptions {
            pages: options.pages.clone(),
            include_images: options.include_images,
            model: None,
        };
        let output = self.client.ocr(source, &options).await.map_err(map_error)?;
        Ok(to_core_output(output))
    }
}

#[async_trait]
impl OcrModel for MistralOcr {
    fn model_id(&self) -> &str {
        &self.client.inner.config.ocr_model
    }

    fn supports(&self, mime: &Mime) -> bool {
        super::ocr::supports_mime(mime)
    }

    async fn recognize_bytes(
        &self,
        data: &[u8],
        mime: &Mime,
        options: &ocr_core::OcrOptions,
    ) -> Result<ocr_core::OcrOutput, ocr_core::OcrError> {
        if !self.supports(mime) {
            return Err(ocr_core::OcrError::UnsupportedMimeType(mime.clone()));
        }
        let source = DocumentSource::from_bytes(data, mime).map_err(map_error)?;
        self.run(&source, options).await
    }

    async fn recognize_url(
        &self,
        url: &str,
        mime: Option<&Mime>,
        options: &ocr_core::OcrOptions,
    ) -> Result<ocr_core::OcrOutput, ocr_core::OcrError> {
        let mime = match mime {
            Some(mime) => mime.clone(),
            None => ocr_core::media_type_from_url(url).ok_or_else(|| {
                ocr_core::OcrError::UnsupportedInput(format!("cannot infer media type of {url}"))
            })?,
        };
        if !self.supports(&mime) {
            return Err(ocr_core::OcrError::UnsupportedMimeType(mime));
        }
        // Mistral fetches the URL itself, so the document is not downloaded here.
        let source = if mime.type_() == mime::IMAGE {
            DocumentSource::image_url(url)
        } else {
            DocumentSource::document_url(url)
        };
        self.run(&source, options).await
    }
}

fn to_core_output(output: super::OcrOutput) -> ocr_core::OcrOutput {
    ocr_core::OcrOutput {
        pages: output
            .pages
            .into_iter()
            .map(|page| ocr_core::OcrPage {
                index: page.index,
                markdown: page.markdown,
                images: page
                    .images
                    .into_iter()
                    .map(|image| ocr_core::OcrImage {
                        id: image.id,
                        mime: image.mime,
                        data: image.data,
                    })
                    .collect(),
            })
            .collect(),
        model_id: output.model,
        usage: Some(ocr_core::OcrUsage {
            pages_processed: Some(output.pages_processed),
            input_bytes: output.doc_size_bytes,
            input_tokens: None,
            output_tokens: None,
        }),
    }
}

fn map_error(error: Error) -> ocr_core::OcrError {
    match error {
        Error::Io(e) => ocr_core::OcrError::Io(e),
        Error::UnsupportedFileType(what) => ocr_core::OcrError::UnsupportedInput(what),
        other => ocr_core::OcrError::Provider {
            retryable: other.is_retryable(),
            source: Box::new(other),
        },
    }
}

#[cfg(test)]
mod tests {
    use markhor_core::ocr::{OcrError, OcrInput, OcrOptions};
    use serde_json::json;
    use wiremock::{
        Mock, MockServer, ResponseTemplate,
        matchers::{body_partial_json, method},
    };

    use super::*;
    use crate::mistral::test_client;

    fn ok_response() -> ResponseTemplate {
        ResponseTemplate::new(200).set_body_json(json!({
            "pages": [],
            "model": "mistral-ocr-2505",
            "usage_info": {"pages_processed": 0},
        }))
    }

    #[tokio::test]
    async fn ocr_model_returns_pages_in_order() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "pages": [
                    {"index": 0, "markdown": "page one", "images": []},
                    {"index": 1, "markdown": "page two", "images": []},
                ],
                "model": "mistral-ocr-2505",
                "usage_info": {"pages_processed": 2, "doc_size_bytes": 99},
            })))
            .mount(&server)
            .await;

        let dir = tempfile::tempdir().unwrap();
        let input = dir.path().join("scan.pdf");
        std::fs::write(&input, b"not really a pdf").unwrap();

        let models = test_client(&server).ocr_models();
        let output = models[0]
            .recognize(OcrInput::Path(input), &OcrOptions::default())
            .await
            .unwrap();

        let pages: Vec<(u32, &str)> = output
            .pages
            .iter()
            .map(|p| (p.index, p.markdown.as_str()))
            .collect();
        assert_eq!(pages, [(0, "page one"), (1, "page two")]);
        assert_eq!(output.model_id, "mistral-ocr-2505");
        assert_eq!(output.usage.unwrap().pages_processed, Some(2));
    }

    #[tokio::test]
    async fn unsupported_media_type_is_rejected_before_request() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ok_response())
            .expect(0)
            .mount(&server)
            .await;

        let models = test_client(&server).ocr_models();
        let input = OcrInput::Bytes {
            data: b"<html></html>".to_vec(),
            mime: mime::TEXT_HTML,
        };
        let err = models[0]
            .recognize(input, &OcrOptions::default())
            .await
            .unwrap_err();
        assert!(matches!(err, OcrError::UnsupportedMimeType(_)), "{err}");
    }

    #[tokio::test]
    async fn remote_urls_are_sent_to_mistral_without_download() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(body_partial_json(json!({
                "document": {"type": "document_url", "document_url": "https://example.com/report.pdf"}
            })))
            .respond_with(ok_response())
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(body_partial_json(json!({
                "document": {"type": "image_url", "image_url": "https://example.com/scan.png"}
            })))
            .respond_with(ok_response())
            .expect(1)
            .mount(&server)
            .await;

        // If the adapter downloaded the file itself, the request would carry base64 data instead
        // of the URL and the mocks above would not match.
        let models = test_client(&server).ocr_models();
        for url in [
            "https://example.com/report.pdf",
            "https://example.com/scan.png",
        ] {
            let input = OcrInput::Url {
                url: url.into(),
                mime: None,
            };
            models[0]
                .recognize(input, &OcrOptions::default())
                .await
                .unwrap();
        }
    }

    #[tokio::test]
    async fn rate_limit_errors_are_retryable_and_keep_message() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(429).set_body_json(json!({
                "object": "error", "message": "Rate limit exceeded",
                "type": "rate_limited", "param": null, "code": "1300"
            })))
            .mount(&server)
            .await;

        let models = test_client(&server).ocr_models();
        let input = OcrInput::Url {
            url: "https://example.com/a.pdf".into(),
            mime: None,
        };
        let err = models[0]
            .recognize(input, &OcrOptions::default())
            .await
            .unwrap_err();
        assert!(err.is_retryable(), "{err}");
        assert!(err.to_string().contains("Rate limit exceeded"), "{err}");
    }
}
