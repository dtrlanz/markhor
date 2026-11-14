use std::path::Path;

use base64::{Engine, engine::general_purpose::STANDARD};
use mime::Mime;

use super::{Client, Error, api};

/// A document or image to run OCR on.
///
/// Mistral distinguishes documents (PDF, PPTX, DOCX) from images (PNG, JPEG, AVIF). Constructors
/// that take bytes or a path determine this from the file type; for URLs the caller has to say.
#[derive(Debug, Clone)]
pub struct DocumentSource(api::Document);

impl DocumentSource {
    /// A publicly reachable (or signed) URL of a PDF, PPTX or DOCX document.
    pub fn document_url(url: impl Into<String>) -> Self {
        Self(api::Document::DocumentUrl {
            document_url: url.into(),
        })
    }

    /// A publicly reachable (or signed) URL of an image.
    pub fn image_url(url: impl Into<String>) -> Self {
        Self(api::Document::ImageUrl {
            image_url: url.into(),
        })
    }

    /// A file previously uploaded with [`Files::upload`](super::Files::upload).
    pub fn file(file_id: impl Into<String>) -> Self {
        Self(api::Document::File {
            file_id: file_id.into(),
        })
    }

    /// File contents, sent inline as a base64 `data:` URL.
    pub fn from_bytes(data: &[u8], mime: &Mime) -> Result<Self, Error> {
        let essence = mime.essence_str();
        let encoded = STANDARD.encode(data);
        let url = format!("data:{essence};base64,{encoded}");
        match essence {
            "application/pdf" | DOCX_MIME | PPTX_MIME => Ok(Self::document_url(url)),
            "image/png" | "image/jpeg" | "image/avif" => Ok(Self::image_url(url)),
            _ => Err(Error::UnsupportedFileType(essence.to_owned())),
        }
    }

    /// Reads a local file, inferring its type from the extension.
    pub async fn from_path(path: impl AsRef<Path>) -> Result<Self, Error> {
        let path = path.as_ref();
        let mime = mime_from_extension(path)?;
        let data = tokio::fs::read(path).await?;
        Self::from_bytes(&data, &mime)
    }
}

const DOCX_MIME: &str = "application/vnd.openxmlformats-officedocument.wordprocessingml.document";
const PPTX_MIME: &str = "application/vnd.openxmlformats-officedocument.presentationml.presentation";

fn mime_from_extension(path: &Path) -> Result<Mime, Error> {
    let extension = path
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase);
    let mime = match extension.as_deref() {
        Some("pdf") => "application/pdf",
        Some("docx") => DOCX_MIME,
        Some("pptx") => PPTX_MIME,
        Some("png") => "image/png",
        Some("jpg" | "jpeg") => "image/jpeg",
        Some("avif") => "image/avif",
        _ => return Err(Error::UnsupportedFileType(path.display().to_string())),
    };
    Ok(mime.parse().expect("hard-coded MIME types are valid"))
}

#[derive(Debug, Clone, Default)]
pub struct OcrOptions {
    /// Return the extracted images (decoded) in addition to the text.
    pub include_images: bool,

    /// Zero-based page numbers to process. All pages if `None`.
    pub pages: Option<Vec<u32>>,

    /// Overrides [`ClientConfig::ocr_model`](super::ClientConfig::ocr_model).
    pub model: Option<String>,
}

#[derive(Debug, Clone)]
pub struct OcrOutput {
    pub pages: Vec<OcrPage>,
    /// The model that processed the document.
    pub model: String,
    pub pages_processed: u32,
    pub doc_size_bytes: Option<u64>,
}

impl OcrOutput {
    /// The markdown of all pages, separated by blank lines.
    pub fn markdown(&self) -> String {
        let pages: Vec<&str> = self.pages.iter().map(|p| p.markdown.as_str()).collect();
        pages.join("\n\n")
    }
}

#[derive(Debug, Clone)]
pub struct OcrPage {
    pub index: u32,
    /// Image and table references use the ids of [`OcrImage`]s, e.g. `![img-0.jpeg](img-0.jpeg)`.
    pub markdown: String,
    pub images: Vec<OcrImage>,
}

#[derive(Debug, Clone)]
pub struct OcrImage {
    /// The name used to reference the image in the page markdown, e.g. `img-0.jpeg`.
    pub id: String,
    pub mime: Option<Mime>,
    /// The decoded image. `None` unless [`OcrOptions::include_images`] was set.
    pub data: Option<Vec<u8>>,
}

impl Client {
    pub async fn ocr(
        &self,
        source: &DocumentSource,
        options: &OcrOptions,
    ) -> Result<OcrOutput, Error> {
        let request = api::OcrRequest {
            model: options
                .model
                .as_deref()
                .unwrap_or(&self.inner.config.ocr_model),
            document: &source.0,
            pages: options.pages.as_deref(),
            include_image_base64: options.include_images,
            image_limit: (!options.include_images).then_some(0),
        };
        let response: api::OcrResponse = api::send(self.inner.post("ocr").json(&request)).await?;
        OcrOutput::try_from(response)
    }
}

impl TryFrom<api::OcrResponse> for OcrOutput {
    type Error = Error;

    fn try_from(response: api::OcrResponse) -> Result<Self, Error> {
        let pages = response
            .pages
            .into_iter()
            .map(|page| {
                let images = page
                    .images
                    .into_iter()
                    .map(OcrImage::try_from)
                    .collect::<Result<_, _>>()?;
                Ok(OcrPage {
                    index: page.index,
                    markdown: page.markdown,
                    images,
                })
            })
            .collect::<Result<_, Error>>()?;
        Ok(Self {
            pages,
            model: response.model,
            pages_processed: response.usage_info.pages_processed,
            doc_size_bytes: response.usage_info.doc_size_bytes,
        })
    }
}

impl TryFrom<api::Image> for OcrImage {
    type Error = Error;

    fn try_from(image: api::Image) -> Result<Self, Error> {
        let Some(encoded) = image.image_base64 else {
            return Ok(Self {
                id: image.id,
                mime: None,
                data: None,
            });
        };

        // The payload is normally a data URL (`data:image/jpeg;base64,...`) but may be bare base64.
        let (mime, payload) = match encoded
            .strip_prefix("data:")
            .and_then(|d| d.split_once(','))
        {
            Some((header, payload)) => {
                let mime = header.split(';').next().and_then(|m| m.parse().ok());
                (mime, payload)
            }
            None => (None, encoded.as_str()),
        };
        let data = STANDARD
            .decode(payload)
            .map_err(|source| Error::InvalidImageData {
                id: image.id.clone(),
                source,
            })?;
        Ok(Self {
            id: image.id,
            mime,
            data: Some(data),
        })
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;
    use wiremock::{
        Mock, MockServer, ResponseTemplate,
        matchers::{body_partial_json, header, method, path},
    };

    use super::*;
    use crate::mistral::test_client;

    #[test]
    fn from_bytes_chooses_document_or_image_kind() {
        let pdf = DocumentSource::from_bytes(b"abc", &"application/pdf".parse().unwrap()).unwrap();
        match pdf.0 {
            api::Document::DocumentUrl { document_url } => {
                assert_eq!(document_url, "data:application/pdf;base64,YWJj")
            }
            other => panic!("expected document_url, got {other:?}"),
        }

        // Parameters on the MIME type must not matter.
        let png = DocumentSource::from_bytes(b"abc", &"image/png; q=1".parse().unwrap()).unwrap();
        assert!(matches!(png.0, api::Document::ImageUrl { .. }));

        let err = DocumentSource::from_bytes(b"abc", &mime::TEXT_PLAIN).unwrap_err();
        assert!(matches!(err, Error::UnsupportedFileType(_)));
    }

    #[tokio::test]
    async fn from_path_rejects_unknown_extension() {
        let err = DocumentSource::from_path("notes.txt").await.unwrap_err();
        assert!(matches!(err, Error::UnsupportedFileType(_)));
    }

    #[tokio::test]
    async fn ocr_sends_request_and_decodes_images() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/ocr"))
            .and(header("authorization", "Bearer test-key"))
            .and(body_partial_json(json!({
                "model": "mistral-ocr-latest",
                "document": {"type": "document_url", "document_url": "https://example.com/a.pdf"},
                "pages": [0, 2],
                "include_image_base64": true,
            })))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "pages": [
                    {"index": 0, "markdown": "first ![img-0.jpeg](img-0.jpeg)", "dimensions": null,
                     "images": [{"id": "img-0.jpeg", "top_left_x": null, "top_left_y": null,
                                 "bottom_right_x": null, "bottom_right_y": null,
                                 "image_base64": "data:image/jpeg;base64,YWJj"}]},
                    {"index": 2, "markdown": "third", "images": [], "dimensions": {}},
                ],
                "model": "mistral-ocr-2505",
                "usage_info": {"pages_processed": 2, "doc_size_bytes": 1234},
            })))
            .expect(1)
            .mount(&server)
            .await;

        let options = OcrOptions {
            include_images: true,
            pages: Some(vec![0, 2]),
            model: None,
        };
        let output = test_client(&server)
            .ocr(
                &DocumentSource::document_url("https://example.com/a.pdf"),
                &options,
            )
            .await
            .unwrap();

        assert_eq!(
            output.markdown(),
            "first ![img-0.jpeg](img-0.jpeg)\n\nthird"
        );
        assert_eq!(output.pages[1].index, 2);
        assert_eq!(output.pages_processed, 2);
        let image = &output.pages[0].images[0];
        assert_eq!(image.data.as_deref(), Some(&b"abc"[..]));
        assert_eq!(image.mime, Some(mime::IMAGE_JPEG));
    }

    #[tokio::test]
    async fn ocr_reports_api_errors() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(429).set_body_json(json!({
                "object": "error", "message": "Rate limit exceeded",
                "type": "rate_limited", "param": null, "code": "1300"
            })))
            .mount(&server)
            .await;

        let err = test_client(&server)
            .ocr(
                &DocumentSource::image_url("https://example.com/a.png"),
                &OcrOptions::default(),
            )
            .await
            .unwrap_err();
        assert!(err.is_retryable());
        assert!(err.to_string().contains("Rate limit exceeded"), "{err}");
    }
}
