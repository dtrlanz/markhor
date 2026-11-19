//! Imports a visual document into a folder: runs OCR, stores the original, and writes the
//! recognized text as a document.

use std::path::{Path, PathBuf};

use thiserror::Error;
use tokio::fs;

use crate::{
    library::{
        ATTACHMENTS_DIR, AccessLibraryError, Document, Folder, METADATA_EXTENSION, NewSource, Part,
    },
    ocr::{OcrError, OcrInput, OcrModel, OcrOptions, OcrPage, media_type_from_url},
};

#[derive(Debug, Error)]
pub enum ImportError {
    #[error("OCR failed: {0}")]
    Ocr(#[from] OcrError),

    #[error("Library error: {0}")]
    Library(#[from] AccessLibraryError),

    #[error("Failed to download document: {0}")]
    Download(#[from] reqwest::Error),

    #[error("A document named {0} already exists")]
    NameClash(PathBuf),
}

/// The original file, stored alongside the recognized text.
enum Original {
    File(PathBuf),
    Data(Vec<u8>),
}

/// Recognizes `input` and stores it in `folder` under the input's file name.
///
/// The name is checked before any work is done, and recognition runs before anything is written,
/// so a failed import leaves no files behind.
pub(crate) async fn import(
    folder: &Folder,
    input: OcrInput,
    model: &dyn OcrModel,
    options: &OcrOptions,
) -> Result<Document, ImportError> {
    todo!()
}

#[cfg(test)]
mod tests {
    use crate::library::{ATTACHMENTS_DIR, METADATA_EXTENSION};
    use async_trait::async_trait;
    use mime::Mime;
    use wiremock::{Mock, MockServer, ResponseTemplate, matchers::method};

    use super::*;
    use crate::library::fs_test_utils::{TempTree, fs_tree};
    use crate::{
        library::{Part, Workspace},
        ocr::{OcrImage, OcrOutput, OcrPage},
    };

    /// Returns fixed pages for any document, or fails if `fail` is set.
    struct FixedOcr {
        pages: Vec<OcrPage>,
        fail: bool,
    }

    #[async_trait]
    impl OcrModel for FixedOcr {
        fn model_id(&self) -> &str {
            "test-fixed"
        }

        fn supports(&self, _mime: &Mime) -> bool {
            true
        }

        async fn recognize_bytes(
            &self,
            _data: &[u8],
            _mime: &Mime,
            _options: &OcrOptions,
        ) -> Result<OcrOutput, OcrError> {
            if self.fail {
                return Err(OcrError::Provider {
                    retryable: false,
                    source: "model unavailable".into(),
                });
            }
            Ok(OcrOutput {
                pages: self.pages.clone(),
                model_id: self.model_id().to_owned(),
                usage: None,
            })
        }
    }

    fn page(index: u32, markdown: &str) -> OcrPage {
        OcrPage {
            index,
            markdown: markdown.into(),
            images: vec![],
        }
    }

    fn two_page_model() -> FixedOcr {
        FixedOcr {
            pages: vec![page(0, "page one"), page(1, "page two")],
            fail: false,
        }
    }

    #[tokio::test]
    async fn local_file_is_copied_and_each_page_becomes_a_part() {
        let ws_dir = TempTree::new(fs_tree! {}).await.unwrap();
        let ws = Workspace::open(&ws_dir).await.unwrap();
        let src = TempTree::new(fs_tree! {
            "scan.png" => b"png-bytes",
        })
        .await
        .unwrap();

        import(
            &ws.root(),
            OcrInput::Path(src.join("scan.png")),
            &two_page_model(),
            &OcrOptions::default(),
        )
        .await
        .unwrap();

        // The original is copied into the workspace and its metadata sidecar is `<name>.mark`.
        assert_eq!(
            std::fs::read(ws.path().join("scan.png")).unwrap(),
            b"png-bytes"
        );
        assert!(
            ws.path()
                .join("scan.png")
                .with_added_extension(METADATA_EXTENSION)
                .is_file()
        );

        // Reopening from disk must yield the same parts, not one part holding raw milestones.
        let reopened = ws.document("scan.png").await.unwrap();
        assert_eq!(reopened.metadata().mime, mime::IMAGE_PNG);
        let contents: Vec<&str> = reopened
            .text()
            .await
            .unwrap()
            .parts()
            .iter()
            .map(Part::content)
            .collect();
        assert_eq!(contents, ["page one", "page two"]);
    }

    #[tokio::test]
    async fn image_links_resolve_to_stored_attachments() {
        let ws_dir = TempTree::new(fs_tree! {}).await.unwrap();
        let ws = Workspace::open(&ws_dir).await.unwrap();
        let src = TempTree::new(fs_tree! {
            "scan.png" => b"png-bytes",
        })
        .await
        .unwrap();
        let model = FixedOcr {
            pages: vec![OcrPage {
                index: 0,
                markdown: "text ![img-0.jpeg](img-0.jpeg)".into(),
                images: vec![OcrImage {
                    id: "img-0.jpeg".into(),
                    mime: Some(mime::IMAGE_JPEG),
                    data: Some(b"jpeg-bytes".to_vec()),
                }],
            }],
            fail: false,
        };
        let options = OcrOptions {
            include_images: true,
            ..Default::default()
        };

        import(
            &ws.root(),
            OcrInput::Path(src.join("scan.png")),
            &model,
            &options,
        )
        .await
        .unwrap();

        // Links are relative to the sidecar, which sits next to the document. Whatever target the
        // link is rewritten to, it must resolve to the stored image.
        let text = ws.document("scan.png").await.unwrap();
        let markdown = text.text().await.unwrap().parts()[0].content().to_owned();
        let target = markdown
            .split("](")
            .nth(1)
            .and_then(|rest| rest.split(')').next())
            .expect("image link present");
        assert_eq!(
            std::fs::read(ws.path().join(target)).unwrap(),
            b"jpeg-bytes"
        );
    }

    #[tokio::test]
    async fn name_clash_is_an_error_and_keeps_existing_document() {
        let ws_dir = TempTree::new(fs_tree! {}).await.unwrap();
        let ws = Workspace::open(&ws_dir).await.unwrap();
        let first_src = TempTree::new(fs_tree! {
            "scan.png" => b"first",
        })
        .await
        .unwrap();
        let second_src = TempTree::new(fs_tree! {
            "scan.png" => b"second",
        })
        .await
        .unwrap();
        let first = first_src.join("scan.png");
        let second = second_src.join("scan.png");

        import(
            &ws.root(),
            OcrInput::Path(first),
            &two_page_model(),
            &OcrOptions::default(),
        )
        .await
        .unwrap();

        let err = import(
            &ws.root(),
            OcrInput::Path(second),
            &two_page_model(),
            &OcrOptions::default(),
        )
        .await
        .unwrap_err();
        assert!(matches!(err, ImportError::NameClash(_)), "{err}");
        assert_eq!(std::fs::read(ws.path().join("scan.png")).unwrap(), b"first");
    }

    #[tokio::test]
    async fn failed_ocr_leaves_no_files_behind() {
        let ws_dir = TempTree::new(fs_tree! {}).await.unwrap();
        let ws = Workspace::open(&ws_dir).await.unwrap();
        let src = TempTree::new(fs_tree! {
            "scan.png" => b"png-bytes",
        })
        .await
        .unwrap();
        let model = FixedOcr {
            pages: vec![],
            fail: true,
        };

        let err = import(
            &ws.root(),
            OcrInput::Path(src.join("scan.png")),
            &model,
            &OcrOptions::default(),
        )
        .await
        .unwrap_err();
        assert!(matches!(err, ImportError::Ocr(_)), "{err}");

        assert!(!ws.path().join("scan.png").exists());
        assert!(
            !ws.path()
                .join("scan.png")
                .with_added_extension(METADATA_EXTENSION)
                .exists()
        );
        assert!(!ws.path().join(ATTACHMENTS_DIR).exists());
    }

    #[tokio::test]
    async fn url_is_downloaded_once_and_the_download_is_stored() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(b"png-bytes"))
            .expect(1)
            .mount(&server)
            .await;

        let ws_dir = TempTree::new(fs_tree! {}).await.unwrap();
        let ws = Workspace::open(&ws_dir).await.unwrap();
        let input = OcrInput::Url {
            url: format!("{}/files/scan.png", server.uri()),
            mime: None,
        };

        import(&ws.root(), input, &two_page_model(), &OcrOptions::default())
            .await
            .unwrap();

        // The mock enforces a single download when the server is dropped.
        assert_eq!(
            std::fs::read(ws.path().join("scan.png")).unwrap(),
            b"png-bytes"
        );
    }
}
