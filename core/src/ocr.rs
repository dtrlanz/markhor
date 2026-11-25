//! Optical character recognition: visual documents in, page-structured Markdown out.

use std::path::{Path, PathBuf};

use async_trait::async_trait;
use mime::Mime;
use thiserror::Error;

/// A model that recognizes text in visual documents such as scanned PDFs and images.
///
/// Consumers call [`recognize`](OcrModel::recognize). Implementors override the hook matching the
/// input they handle natively. The default `recognize_path` and `recognize_url` load the document
/// and delegate to `recognize_bytes`, so a model that only implements `recognize_bytes` accepts
/// every input kind.
#[async_trait]
pub trait OcrModel: Send + Sync {
    /// Identifies the underlying model, for provenance and cache keys.
    fn model_id(&self) -> &str;

    /// Whether this model accepts documents of the given media type.
    fn supports(&self, mime: &Mime) -> bool;

    /// Recognizes text in `input`.
    // TODO: Decide whether pages that succeeded should be returned alongside an error when only
    // some pages fail.
    async fn recognize(
        &self,
        input: OcrInput,
        options: &OcrOptions,
    ) -> Result<OcrOutput, OcrError> {
        match input {
            OcrInput::Bytes { data, mime } => self.recognize_bytes(&data, &mime, options).await,
            OcrInput::Path(path) => self.recognize_path(&path, options).await,
            OcrInput::Url { url, mime } => self.recognize_url(&url, mime.as_ref(), options).await,
        }
    }

    /// Recognizes an in-memory document.
    async fn recognize_bytes(
        &self,
        _data: &[u8],
        mime: &Mime,
        _options: &OcrOptions,
    ) -> Result<OcrOutput, OcrError> {
        Err(OcrError::UnsupportedInput(format!(
            "{} does not accept {} as bytes",
            self.model_id(),
            mime
        )))
    }

    /// Recognizes a local file. The media type is inferred from the file extension.
    async fn recognize_path(
        &self,
        path: &Path,
        options: &OcrOptions,
    ) -> Result<OcrOutput, OcrError> {
        let mime = mime_guess2::from_path(path).first().ok_or_else(|| {
            OcrError::UnsupportedInput(format!("cannot infer media type of {}", path.display()))
        })?;
        if !self.supports(&mime) {
            return Err(OcrError::UnsupportedMimeType(mime));
        }
        let data = tokio::fs::read(path).await?;
        self.recognize_bytes(&data, &mime, options).await
    }

    /// Recognizes a remote document. The media type is taken from `mime` if given, otherwise
    /// inferred from the URL path.
    async fn recognize_url(
        &self,
        url: &str,
        mime: Option<&Mime>,
        options: &OcrOptions,
    ) -> Result<OcrOutput, OcrError> {
        let mime = match mime {
            Some(mime) => mime.clone(),
            None => media_type_from_url(url).ok_or_else(|| {
                OcrError::UnsupportedInput(format!("cannot infer media type of {url}"))
            })?,
        };
        if !self.supports(&mime) {
            return Err(OcrError::UnsupportedMimeType(mime));
        }
        let data = reqwest::get(url).await?.error_for_status()?.bytes().await?;
        self.recognize_bytes(&data, &mime, options).await
    }
}

/// Infers the media type of a URL from its path, ignoring any query string or fragment.
pub fn media_type_from_url(url: &str) -> Option<Mime> {
    let parsed = reqwest::Url::parse(url).ok()?;
    mime_guess2::from_path(parsed.path()).first()
}

/// A document to recognize, in the form the caller has it.
#[derive(Debug, Clone)]
pub enum OcrInput {
    Bytes { data: Vec<u8>, mime: Mime },
    Path(PathBuf),
    Url { url: String, mime: Option<Mime> },
}

#[derive(Debug, Clone, Default)]
pub struct OcrOptions {
    /// Zero-based page numbers to process. All pages if `None`.
    pub pages: Option<Vec<u32>>,
    /// Return extracted image data in addition to the markdown.
    pub include_images: bool,
}

#[derive(Debug, Clone)]
pub struct OcrOutput {
    /// Pages in document order.
    pub pages: Vec<OcrPage>,
    /// The model that produced the output, which may differ from the requested alias.
    pub model_id: String,
    pub usage: Option<OcrUsage>,
}

#[derive(Debug, Clone)]
pub struct OcrPage {
    /// Zero-based page index in the source document.
    pub index: u32,
    /// Image references use the ids of the page's [`OcrImage`]s, e.g. `![img-0.jpeg](img-0.jpeg)`.
    pub markdown: String,
    pub images: Vec<OcrImage>,
}

#[derive(Debug, Clone)]
pub struct OcrImage {
    pub id: String,
    pub mime: Option<Mime>,
    /// `None` unless [`OcrOptions::include_images`] was set.
    pub data: Option<Vec<u8>>,
}

/// Usage reported by the model. Each field is `None` when the model does not report it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct OcrUsage {
    pub pages_processed: Option<u32>,
    pub input_bytes: Option<u64>,
    pub input_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
}

#[derive(Debug, Error)]
pub enum OcrError {
    #[error("Unsupported media type: {0}")]
    UnsupportedMimeType(Mime),

    #[error("Unsupported input: {0}")]
    UnsupportedInput(String),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("HTTP error: {0}")]
    Http(#[from] reqwest::Error),

    /// An error from the model provider. Implementors decide whether it is retryable.
    #[error("OCR failed: {source}")]
    Provider {
        retryable: bool,
        source: Box<dyn std::error::Error + Send + Sync>,
    },
}

impl OcrError {
    pub fn is_retryable(&self) -> bool {
        match self {
            Self::Provider { retryable, .. } => *retryable,
            Self::Http(e) => {
                e.is_timeout() || e.is_connect() || e.status().is_some_and(|s| s.is_server_error())
            }
            _ => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use wiremock::{
        Mock, MockServer, ResponseTemplate,
        matchers::{method, path},
    };

    use super::*;

    /// Echoes document bytes back as markdown and records the media types it receives.
    struct EchoModel {
        supported: Mime,
        seen: Mutex<Vec<Mime>>,
    }

    impl EchoModel {
        fn new(supported: Mime) -> Self {
            Self {
                supported,
                seen: Mutex::new(Vec::new()),
            }
        }

        fn seen(&self) -> Vec<Mime> {
            self.seen.lock().unwrap().clone()
        }
    }

    #[async_trait]
    impl OcrModel for EchoModel {
        fn model_id(&self) -> &str {
            "test-echo"
        }

        fn supports(&self, mime: &Mime) -> bool {
            mime.essence_str() == self.supported.essence_str()
        }

        async fn recognize_bytes(
            &self,
            data: &[u8],
            mime: &Mime,
            _options: &OcrOptions,
        ) -> Result<OcrOutput, OcrError> {
            self.seen.lock().unwrap().push(mime.clone());
            Ok(OcrOutput {
                pages: vec![OcrPage {
                    index: 0,
                    markdown: String::from_utf8_lossy(data).into_owned(),
                    images: vec![],
                }],
                model_id: self.model_id().to_owned(),
                usage: None,
            })
        }
    }

    /// Overrides no hooks. Every input kind should report `UnsupportedInput`, not recurse.
    struct NoHooksModel;

    #[async_trait]
    impl OcrModel for NoHooksModel {
        fn model_id(&self) -> &str {
            "test-no-hooks"
        }

        fn supports(&self, _mime: &Mime) -> bool {
            true
        }
    }

    fn markdown(output: &OcrOutput) -> &str {
        &output.pages[0].markdown
    }

    #[tokio::test]
    async fn path_input_is_read_and_handed_to_bytes_hook() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("scan.png");
        std::fs::write(&file, b"abc").unwrap();

        let echo = EchoModel::new(mime::IMAGE_PNG);
        let model: &dyn OcrModel = &echo;
        let output = model
            .recognize(OcrInput::Path(file), &OcrOptions::default())
            .await
            .unwrap();

        assert_eq!(markdown(&output), "abc");
        assert_eq!(echo.seen(), [mime::IMAGE_PNG]);
    }

    #[tokio::test]
    async fn unsupported_path_is_rejected_without_reading_file() {
        let echo = EchoModel::new(mime::APPLICATION_PDF);
        // The files do not exist, so reading them would fail with an IO error.
        let err = echo
            .recognize(OcrInput::Path("notes.txt".into()), &OcrOptions::default())
            .await
            .unwrap_err();
        assert!(matches!(err, OcrError::UnsupportedMimeType(_)), "{err}");

        let err = echo
            .recognize(OcrInput::Path("scan".into()), &OcrOptions::default())
            .await
            .unwrap_err();
        assert!(matches!(err, OcrError::UnsupportedInput(_)), "{err}");
        assert!(echo.seen().is_empty());
    }

    #[tokio::test]
    async fn model_overriding_no_hooks_reports_unsupported_input() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("scan.png");
        std::fs::write(&file, b"abc").unwrap();

        let model = NoHooksModel;
        let err = model
            .recognize(OcrInput::Path(file), &OcrOptions::default())
            .await
            .unwrap_err();
        assert!(matches!(err, OcrError::UnsupportedInput(_)), "{err}");

        let input = OcrInput::Bytes {
            data: b"abc".to_vec(),
            mime: mime::IMAGE_PNG,
        };
        let err = model
            .recognize(input, &OcrOptions::default())
            .await
            .unwrap_err();
        assert!(matches!(err, OcrError::UnsupportedInput(_)), "{err}");
    }

    #[tokio::test]
    async fn url_media_type_comes_from_explicit_mime_or_url_path() {
        let server = MockServer::start().await;
        for url_path in ["/doc.pdf", "/download"] {
            Mock::given(method("GET"))
                .and(path(url_path))
                .respond_with(ResponseTemplate::new(200).set_body_bytes(b"pdf-bytes"))
                .mount(&server)
                .await;
        }

        let echo = EchoModel::new(mime::APPLICATION_PDF);
        // The query string must not affect type inference.
        let inferred = OcrInput::Url {
            url: format!("{}/doc.pdf?token=abc", server.uri()),
            mime: None,
        };
        // A URL without an extension needs an explicit type.
        let explicit = OcrInput::Url {
            url: format!("{}/download", server.uri()),
            mime: Some(mime::APPLICATION_PDF),
        };
        for input in [inferred, explicit] {
            let output = echo.recognize(input, &OcrOptions::default()).await.unwrap();
            assert_eq!(markdown(&output), "pdf-bytes");
        }
        assert_eq!(echo.seen(), [mime::APPLICATION_PDF, mime::APPLICATION_PDF]);
    }

    #[tokio::test]
    async fn unsupported_url_is_rejected_without_downloading() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200))
            .expect(0)
            .mount(&server)
            .await;

        let echo = EchoModel::new(mime::APPLICATION_PDF);
        let input = OcrInput::Url {
            url: format!("{}/image.png", server.uri()),
            mime: None,
        };
        let err = echo
            .recognize(input, &OcrOptions::default())
            .await
            .unwrap_err();
        assert!(matches!(err, OcrError::UnsupportedMimeType(_)), "{err}");
    }

    #[tokio::test]
    async fn url_error_status_is_not_passed_to_model() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(503).set_body_string("<html>busy</html>"))
            .mount(&server)
            .await;

        let echo = EchoModel::new(mime::APPLICATION_PDF);
        let input = OcrInput::Url {
            url: format!("{}/doc.pdf", server.uri()),
            mime: None,
        };
        let err = echo
            .recognize(input, &OcrOptions::default())
            .await
            .unwrap_err();
        assert!(matches!(err, OcrError::Http(_)), "{err}");
        assert!(err.is_retryable());
        assert!(echo.seen().is_empty());
    }
}
