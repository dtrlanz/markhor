//! Imports a visual document into a folder: runs OCR, stores the original, and writes the
//! recognized text as a document.

use std::path::{Path, PathBuf};

use markhor_core::{
    library::{AccessLibraryError, Document, Folder, NewSource, Part},
    ocr::{OcrError, OcrInput, OcrModel, OcrOptions, OcrPage, media_type_from_url},
    permissions::Permission,
};
use thiserror::Error;

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

/// How the pages returned by OCR are stored as the text of a document.
///
/// Pages are never reflowed. A page break inside a block (paragraph, list, table, or code block)
/// is kept as a break, so the block may render differently from the original page. Running
/// headers and footers, page numbers, hyphenated words split across pages, and multi-column
/// reading order are kept as OCR returned them.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum PageMode {
    /// Each page becomes its own part, identified by the page's zero-based index. Page boundaries
    /// are recoverable from the part ids.
    PartPerPage,
    /// One part. Each page is preceded by a `<milestone unit="page" n="{index}" />` marker, so the
    /// page boundaries are recoverable from the text. The markers remain in the text, including
    /// the text given to chunkers and embedding models. Inside a code block, a marker is literal
    /// text.
    #[default]
    Milestones,
    /// One part with no markers. Pages are joined by blank lines, and page boundaries are lost.
    /// A code block that spans a page break is left unclosed.
    Merged,
}

/// Options for [`import`].
#[derive(Debug, Clone, Default)]
pub struct ImportOptions {
    pub ocr: OcrOptions,
    pub page_mode: PageMode,
    /// Permissions needed to access the imported document. `None` uses the workspace default;
    /// `Some(vec![])` makes the document unrestricted.
    pub required_permissions: Option<Vec<Permission>>,
}

/// Recognizes `input` and stores it in `folder` under the input's file name.
///
/// The name is checked before any work is done, and recognition runs before anything is written,
/// so a failed import leaves no files behind.
pub async fn import(
    folder: &Folder,
    input: OcrInput,
    model: &dyn OcrModel,
    options: &ImportOptions,
) -> Result<Document, ImportError> {
    let name = document_name(&input)?;
    if folder.has_document(&name).await? {
        return Err(ImportError::NameClash(folder.path().join(&name)));
    }

    let (output, original) = match input {
        OcrInput::Path(path) => {
            let output = model
                .recognize(OcrInput::Path(path.clone()), &options.ocr)
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
                    &options.ocr,
                )
                .await?;
            (output, Original::Data(data))
        }
        OcrInput::Bytes { .. } => return Err(ImportError::UnnamedInput),
    };

    let created = match &original {
        Original::File(path) => {
            folder
                .create_document(
                    &name,
                    NewSource::CopyFrom(path.clone()),
                    options.required_permissions.as_deref(),
                )
                .await
        }
        Original::Data(data) => {
            folder
                .create_document(
                    &name,
                    NewSource::Bytes(data.as_slice()),
                    options.required_permissions.as_deref(),
                )
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
    match store_text(&mut doc, output.pages, options.page_mode, &mut attachments).await {
        Ok(()) => Ok(doc),
        Err(e) => {
            doc.discard(&attachments).await;
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

/// Writes the pages of `doc` as text laid out according to `mode`, stores their images as
/// attachments, and saves the document.
///
/// Every attachment written is pushed to `attachments`, so the caller can remove them on failure.
async fn store_text(
    doc: &mut Document,
    pages: Vec<OcrPage>,
    mode: PageMode,
    attachments: &mut Vec<PathBuf>,
) -> Result<(), ImportError> {
    let mut parts = Vec::new();
    let mut blocks = Vec::with_capacity(pages.len());
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
        match mode {
            PageMode::PartPerPage => parts.push(Part::new(page.index.to_string(), markdown)),
            // The blank lines keep a page break from joining the blocks on either side of it.
            PageMode::Milestones => blocks.push(format!(
                "<milestone unit=\"page\" n=\"{}\" />\n\n{}",
                page.index,
                markdown.trim()
            )),
            PageMode::Merged => {
                let trimmed = markdown.trim();
                if !trimmed.is_empty() {
                    blocks.push(trimmed.to_owned());
                }
            }
        }
    }
    if !blocks.is_empty() {
        parts.push(Part::new("", blocks.join("\n\n")));
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
///
/// The target is written as a `<…>` link destination, which may contain spaces and parentheses.
/// Backslashes and angle brackets in the target are escaped.
// FIXME: This seems hacky and brittle.
fn rewrite_image_link(markdown: &str, id: &str, target: &str) -> String {
    let mut destination = String::from("<");
    for c in target.chars() {
        if matches!(c, '\\' | '<' | '>') {
            destination.push('\\');
        }
        destination.push(c);
    }
    destination.push('>');
    markdown.replace(&format!("]({id})"), &format!("]({destination})"))
}

#[cfg(test)]
mod tests {
    use async_trait::async_trait;
    use markhor_core::{
        library::{Part, Workspace},
        markdown::{ToMarkdown, WITH_MILESTONES},
        ocr::{OcrImage, OcrOutput},
    };
    use mime::Mime;
    use tempfile::TempDir;
    use wiremock::{Mock, MockServer, ResponseTemplate, matchers::method};

    use super::*;

    // Mirror the layout constants used by `markhor_core::library`.
    const ATTACHMENTS_DIR: &str = "attachments";
    const METADATA_EXTENSION: &str = "mark";

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

    /// An empty workspace. The temporary directory is returned so it lives as long as the test.
    async fn workspace() -> (TempDir, Workspace) {
        let dir = tempfile::tempdir().unwrap();
        let ws = Workspace::open(dir.path()).await.unwrap();
        (dir, ws)
    }

    /// Writes a source file named `name` in a new temporary directory.
    fn source(name: &str, data: &[u8]) -> (TempDir, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(name);
        std::fs::write(&path, data).unwrap();
        (dir, path)
    }

    fn image(id: &str, data: &[u8]) -> OcrImage {
        OcrImage {
            id: id.into(),
            mime: Some(mime::IMAGE_JPEG),
            data: Some(data.to_vec()),
        }
    }

    fn with_images() -> ImportOptions {
        ImportOptions {
            ocr: OcrOptions {
                include_images: true,
                ..Default::default()
            },
            ..Default::default()
        }
    }

    #[tokio::test]
    async fn import_uses_workspace_default_unless_permissions_are_given() {
        use markhor_core::permissions::{ON_DEVICE, PUBLIC, Restricted};

        let (_ws_dir, ws) = workspace().await;
        let cases = [
            ("default.png", None),
            ("public.png", Some(vec![PUBLIC])),
            ("open.png", Some(vec![])),
        ];
        for (name, required_permissions) in cases {
            let (_src_dir, src) = source(name, b"png-bytes");
            let options = ImportOptions {
                required_permissions,
                ..Default::default()
            };
            import(&ws.root(), OcrInput::Path(src), &two_page_model(), &options)
                .await
                .unwrap();
        }

        let reopen = |name: &'static str| ws.document(name);
        assert_eq!(
            reopen("default.png").await.unwrap().permissions_required(),
            &[ON_DEVICE]
        );
        assert_eq!(
            reopen("public.png").await.unwrap().permissions_required(),
            &[PUBLIC]
        );
        assert!(
            reopen("open.png")
                .await
                .unwrap()
                .permissions_required()
                .is_empty()
        );
    }

    #[tokio::test]
    async fn local_file_is_copied_and_each_page_becomes_a_part() {
        let (_ws_dir, ws) = workspace().await;
        let (_src_dir, src) = source("scan.png", b"png-bytes");

        import(
            &ws.root(),
            OcrInput::Path(src),
            &two_page_model(),
            &ImportOptions {
                page_mode: PageMode::PartPerPage,
                ..Default::default()
            },
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
    async fn default_import_marks_each_page_in_one_part() {
        let (_ws_dir, ws) = workspace().await;
        let (_src_dir, src) = source("scan.png", b"png-bytes");

        import(
            &ws.root(),
            OcrInput::Path(src),
            &two_page_model(),
            &ImportOptions::default(),
        )
        .await
        .unwrap();

        let reopened = ws.document("scan.png").await.unwrap();
        let text = reopened.text().await.unwrap();
        let parts = text.parts();
        assert_eq!(parts.len(), 1);
        assert_eq!(
            parts[0].content(),
            "<milestone unit=\"page\" n=\"0\" />\n\npage one\n\n<milestone unit=\"page\" n=\"1\" />\n\npage two"
        );
    }

    #[tokio::test]
    async fn page_markers_do_not_join_blocks_across_pages() {
        let (_ws_dir, ws) = workspace().await;
        let (_src_dir, src) = source("scan.png", b"png-bytes");
        let model = FixedOcr {
            pages: vec![page(0, "First paragraph."), page(1, "Second paragraph.")],
            fail: false,
        };

        import(
            &ws.root(),
            OcrInput::Path(src),
            &model,
            &ImportOptions::default(),
        )
        .await
        .unwrap();

        let reopened = ws.document("scan.png").await.unwrap();
        let text = reopened.text().await.unwrap();
        let html = text.parts()[0]
            .content()
            .to_markdown(WITH_MILESTONES)
            .to_html();
        assert!(html.contains("<p>First paragraph.</p>"), "{html}");
        assert!(html.contains("<p>Second paragraph.</p>"), "{html}");
    }

    #[tokio::test]
    async fn merged_pages_are_joined_by_blank_lines_and_empty_pages_are_skipped() {
        let (_ws_dir, ws) = workspace().await;
        let (_src_dir, src) = source("scan.png", b"png-bytes");
        let model = FixedOcr {
            pages: vec![page(0, "page one\n"), page(1, "  \n"), page(2, "page two")],
            fail: false,
        };
        let options = ImportOptions {
            page_mode: PageMode::Merged,
            ..Default::default()
        };

        import(&ws.root(), OcrInput::Path(src), &model, &options)
            .await
            .unwrap();

        let reopened = ws.document("scan.png").await.unwrap();
        let text = reopened.text().await.unwrap();
        let parts = text.parts();
        assert_eq!(parts.len(), 1);
        assert_eq!(parts[0].content(), "page one\n\npage two");
    }

    #[tokio::test]
    async fn image_links_resolve_to_stored_attachments() {
        let (_ws_dir, ws) = workspace().await;
        let (_src_dir, src) = source("scan.png", b"png-bytes");
        let model = FixedOcr {
            pages: vec![OcrPage {
                index: 0,
                markdown: "text ![img-0.jpeg](img-0.jpeg)".into(),
                images: vec![image("img-0.jpeg", b"jpeg-bytes")],
            }],
            fail: false,
        };

        import(&ws.root(), OcrInput::Path(src), &model, &with_images())
            .await
            .unwrap();

        // Links are relative to the sidecar, which sits next to the document. Whatever target the
        // link is rewritten to, it must resolve to the stored image.
        let text = ws.document("scan.png").await.unwrap();
        let markdown = text.text().await.unwrap().parts()[0].content().to_owned();
        let destinations = image_destinations(&markdown);
        assert_eq!(destinations.len(), 1);
        assert_eq!(
            std::fs::read(ws.path().join(&destinations[0])).unwrap(),
            b"jpeg-bytes"
        );
    }

    #[tokio::test]
    async fn name_clash_is_an_error_and_keeps_existing_document() {
        let (_ws_dir, ws) = workspace().await;
        let (_first_dir, first) = source("scan.png", b"first");
        let (_second_dir, second) = source("scan.png", b"second");

        import(
            &ws.root(),
            OcrInput::Path(first),
            &two_page_model(),
            &ImportOptions::default(),
        )
        .await
        .unwrap();

        let err = import(
            &ws.root(),
            OcrInput::Path(second),
            &two_page_model(),
            &ImportOptions::default(),
        )
        .await
        .unwrap_err();
        assert!(matches!(err, ImportError::NameClash(_)), "{err}");
        assert_eq!(std::fs::read(ws.path().join("scan.png")).unwrap(), b"first");
    }

    #[tokio::test]
    async fn failed_ocr_leaves_no_files_behind() {
        let (_ws_dir, ws) = workspace().await;
        let (_src_dir, src) = source("scan.png", b"png-bytes");
        let model = FixedOcr {
            pages: vec![],
            fail: true,
        };

        let err = import(
            &ws.root(),
            OcrInput::Path(src),
            &model,
            &ImportOptions::default(),
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
        let (_ws_dir, ws) = workspace().await;
        // A file named `attachments` makes the attachment directory impossible to create.
        std::fs::write(ws.path().join(ATTACHMENTS_DIR), "not a directory").unwrap();
        let (_src_dir, src) = source("scan.png", b"png-bytes");
        let model = FixedOcr {
            pages: vec![OcrPage {
                index: 0,
                markdown: "![img-0.jpeg](img-0.jpeg)".into(),
                images: vec![image("img-0.jpeg", b"jpeg-bytes")],
            }],
            fail: false,
        };

        let err = import(&ws.root(), OcrInput::Path(src), &model, &with_images())
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

    /// The destinations of the images in `markdown`, as CommonMark parses them.
    fn image_destinations(markdown: &str) -> Vec<String> {
        pulldown_cmark::Parser::new_ext(markdown, WITH_MILESTONES.md_options)
            .filter_map(|event| match event {
                pulldown_cmark::Event::Start(pulldown_cmark::Tag::Image { dest_url, .. }) => {
                    Some(dest_url.into_string())
                }
                _ => None,
            })
            .collect()
    }

    #[tokio::test]
    async fn image_ids_repeated_across_pages_get_distinct_attachments() {
        let (_ws_dir, ws) = workspace().await;
        let (_src_dir, src) = source("scan.png", b"png-bytes");
        let model = FixedOcr {
            pages: vec![
                OcrPage {
                    index: 0,
                    markdown: "![img-0.jpeg](img-0.jpeg)".into(),
                    images: vec![image("img-0.jpeg", b"first")],
                },
                OcrPage {
                    index: 1,
                    markdown: "![img-0.jpeg](img-0.jpeg)".into(),
                    images: vec![image("img-0.jpeg", b"second")],
                },
            ],
            fail: false,
        };

        import(&ws.root(), OcrInput::Path(src), &model, &with_images())
            .await
            .unwrap();

        let reopened = ws.document("scan.png").await.unwrap();
        let text = reopened.text().await.unwrap();
        let targets = image_destinations(text.parts()[0].content());
        assert_eq!(targets.len(), 2);
        assert_eq!(
            std::fs::read(ws.path().join(&targets[0])).unwrap(),
            b"first"
        );
        assert_eq!(
            std::fs::read(ws.path().join(&targets[1])).unwrap(),
            b"second"
        );
    }

    #[tokio::test]
    async fn names_with_spaces_resolve_to_their_attachments() {
        let (_ws_dir, ws) = workspace().await;
        let (_src_dir, src) = source("my scan.png", b"png-bytes");
        let model = FixedOcr {
            pages: vec![OcrPage {
                index: 0,
                markdown: "![img 0.jpeg](img 0.jpeg)".into(),
                images: vec![image("img 0.jpeg", b"jpeg-bytes")],
            }],
            fail: false,
        };

        import(&ws.root(), OcrInput::Path(src), &model, &with_images())
            .await
            .unwrap();

        let reopened = ws.document("my scan.png").await.unwrap();
        let text = reopened.text().await.unwrap();
        let target = image_destinations(text.parts()[0].content());
        assert_eq!(target.len(), 1);
        assert_eq!(
            std::fs::read(ws.path().join(&target[0])).unwrap(),
            b"jpeg-bytes"
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

        let (_ws_dir, ws) = workspace().await;
        let input = OcrInput::Url {
            url: format!("{}/files/scan.png", server.uri()),
            mime: None,
        };

        import(
            &ws.root(),
            input,
            &two_page_model(),
            &ImportOptions::default(),
        )
        .await
        .unwrap();

        // The mock enforces a single download when the server is dropped.
        assert_eq!(
            std::fs::read(ws.path().join("scan.png")).unwrap(),
            b"png-bytes"
        );
    }
}
