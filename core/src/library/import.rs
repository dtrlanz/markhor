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
    #[error(transparent)]
    Ocr(#[from] OcrError),

    #[error("Library error: {0}")]
    Library(#[from] AccessLibraryError),

    #[error("Failed to download document: {0}")]
    Download(#[from] reqwest::Error),

    #[error("A document named {0} already exists")]
    NameClash(PathBuf),

    #[error("No document name can be derived from the input")]
    UnnamedInput,
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
pub async fn import(
    folder: &Folder,
    input: OcrInput,
    model: &dyn OcrModel,
    options: &OcrOptions,
) -> Result<Document, ImportError> {
    let name = document_name(&input)?;
    if name_is_taken(folder, &name).await? {
        return Err(ImportError::NameClash(folder.path().join(&name)));
    }

    let (output, original) = match input {
        OcrInput::Path(path) => {
            let output = model
                .recognize(OcrInput::Path(path.clone()), options)
                .await?;
            (output, Original::File(path))
        }
        OcrInput::Url { url, mime } => {
            let mime = mime.or_else(|| media_type_from_url(&url)).ok_or_else(|| {
                OcrError::UnsupportedInput(format!("cannot infer media type of {url}"))
            })?;
            // The download is both the original and the OCR input, so it is fetched only once.
            let data = reqwest::get(url.as_str())
                .await?
                .error_for_status()?
                .bytes()
                .await?
                .to_vec();
            let output = model
                .recognize(
                    OcrInput::Bytes {
                        data: data.clone(),
                        mime,
                    },
                    options,
                )
                .await?;
            (output, Original::Data(data))
        }
        OcrInput::Bytes { .. } => return Err(ImportError::UnnamedInput),
    };

    let created = match &original {
        Original::File(path) => {
            folder
                .create_document(&name, NewSource::CopyFrom(path.clone()))
                .await
        }
        Original::Data(data) => {
            folder
                .create_document(&name, NewSource::Bytes(data.as_slice()))
                .await
        }
    };
    let mut doc = created.map_err(|e| match e {
        AccessLibraryError::FileAlreadyExists(_) => {
            ImportError::NameClash(folder.path().join(&name))
        }
        e => ImportError::Library(e),
    })?;

    let mut attachments = Vec::new();
    match store_text(&mut doc, output.pages, &mut attachments).await {
        Ok(()) => Ok(doc),
        Err(e) => {
            discard(&doc, &attachments).await;
            Err(e)
        }
    }
}

/// The name a document imported from `input` is given: the file name of a path, or the last
/// segment of a URL.
fn document_name(input: &OcrInput) -> Result<String, ImportError> {
    let name = match input {
        OcrInput::Path(path) => path.file_name().and_then(|n| n.to_str()).map(str::to_owned),
        OcrInput::Url { url, .. } => reqwest::Url::parse(url).ok().and_then(|parsed| {
            parsed
                .path_segments()?
                .last()
                .filter(|segment| !segment.is_empty())
                .map(str::to_owned)
        }),
        OcrInput::Bytes { .. } => None,
    };
    name.ok_or(ImportError::UnnamedInput)
}

/// A document or its metadata sidecar already exists under `name`.
async fn name_is_taken(folder: &Folder, name: &str) -> Result<bool, AccessLibraryError> {
    let path = folder.path().join(name);
    Ok(fs::try_exists(&path).await?
        || fs::try_exists(path.with_added_extension(METADATA_EXTENSION)).await?)
}

/// Writes each page as a part of `doc`, stores its images as attachments, and saves the document.
///
/// Every attachment written is pushed to `attachments`, so the caller can remove them on failure.
async fn store_text(
    doc: &mut Document,
    pages: Vec<OcrPage>,
    attachments: &mut Vec<PathBuf>,
) -> Result<(), ImportError> {
    let mut parts = Vec::with_capacity(pages.len());
    for page in pages {
        let mut markdown = page.markdown;
        for image in page.images {
            let Some(data) = image.data else { continue };
            // Image ids are only unique within a page, so the page index is part of the name.
            let name = format!("{}-{}", page.index, image.id);
            let rel = doc.write_attachment(&name, &data).await?;
            markdown = rewrite_image_link(&markdown, &image.id, &link_to(&rel));
            attachments.push(rel);
        }
        parts.push(Part::new(page.index.to_string(), markdown));
    }

    let text = doc.text_mut().await?;
    for part in parts {
        text.push_part(part);
    }
    doc.save().await?;
    Ok(())
}

/// Formats a path relative to the document's directory as a link target.
fn link_to(rel: &Path) -> String {
    rel.components()
        .map(|c| c.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
}

/// Points the image link for `id` at `target`.
fn rewrite_image_link(markdown: &str, id: &str, target: &str) -> String {
    markdown.replace(&format!("]({id})"), &format!("]({target})"))
}

/// Removes what an import wrote. Directories are removed only when empty, so unrelated files
/// are never deleted.
async fn discard(doc: &Document, attachments: &[PathBuf]) {
    let dir = doc.absolute_path.parent().unwrap();
    for rel in attachments {
        let path = dir.join(rel);
        let _ = fs::remove_file(&path).await;
        if let Some(parent) = path.parent() {
            let _ = fs::remove_dir(parent).await;
        }
    }
    let _ = fs::remove_dir(dir.join(ATTACHMENTS_DIR)).await;
    let _ = fs::remove_file(&doc.absolute_path).await;
    let _ = fs::remove_file(doc.absolute_path.with_added_extension(METADATA_EXTENSION)).await;
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
    async fn failed_attachment_write_removes_the_imported_document() {
        // A file named `attachments` makes the attachment directory impossible to create.
        let ws_dir = TempTree::new(fs_tree! {
            ATTACHMENTS_DIR => "not a directory",
        })
        .await
        .unwrap();
        let ws = Workspace::open(&ws_dir).await.unwrap();
        let src = TempTree::new(fs_tree! {
            "scan.png" => b"png-bytes",
        })
        .await
        .unwrap();
        let model = FixedOcr {
            pages: vec![OcrPage {
                index: 0,
                markdown: "![img-0.jpeg](img-0.jpeg)".into(),
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

        let err = import(
            &ws.root(),
            OcrInput::Path(src.join("scan.png")),
            &model,
            &options,
        )
        .await
        .unwrap_err();
        assert!(matches!(err, ImportError::Library(_)), "{err}");

        assert!(!ws.path().join("scan.png").exists());
        assert!(
            !ws.path()
                .join("scan.png")
                .with_added_extension(METADATA_EXTENSION)
                .exists()
        );
        assert_eq!(
            std::fs::read_to_string(ws.path().join(ATTACHMENTS_DIR)).unwrap(),
            "not a directory"
        );
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
