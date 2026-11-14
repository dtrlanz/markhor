//! Live tests for the Mistral client against the real API (OCR and Files).
//!
//! Require a `.env` file (or environment) containing `MISTRAL_API_KEY=<key>` and, for the tests
//! that go through a `Session`, `MISTRAL_API_KEY_PERMISSIONS=<comma-separated permissions>`.
//! Ignored by default to avoid network calls (and spend) on every `cargo test`.
//! Run with: `cargo test -p markhor_extensions --test mistral-ocr -- --ignored --test-threads=1`

use std::{
    path::Path,
    sync::atomic::{AtomicU64, Ordering},
};

use markhor_core::{
    convert::Converter,
    dependencies::{ApiKey, Session},
    extension::{Comp, ExtensionConfig},
};
use markhor_extensions::mistral::{Client, DocumentSource, OcrOptions, OcrOutput, UploadOptions};
use tokio::io::AsyncReadExt;

mod common;

const TEST_PDF_PATH: &str = "tests/common/Lorem ipsum with Mona Lisa.pdf";
const TEST_DOCX_PATH: &str = "tests/common/Lorem ipsum with Mona Lisa.docx";

// Adjust the number of tests to run per minute to avoid exceeding rate limits
const TESTS_PER_MINUTE: u64 = 20;
const TEST_DELAY_IN_MILLISECONDS: u64 = 60 * 1000 / TESTS_PER_MINUTE;
static TEST_COUNT: AtomicU64 = AtomicU64::new(0);

async fn delay_test() {
    let count = TEST_COUNT.fetch_add(1, Ordering::SeqCst);
    tokio::time::sleep(tokio::time::Duration::from_millis(
        TEST_DELAY_IN_MILLISECONDS * count,
    ))
    .await;
}

fn client() -> Client {
    let key = common::get_api_key("MISTRAL_API_KEY");
    Client::new(ApiKey::new(key, "mistral".into(), "test".into()))
}

fn images_included() -> OcrOptions {
    OcrOptions {
        include_images: true,
        ..Default::default()
    }
}

/// Checks that apply to every successful OCR response.
fn assert_consistent(output: &OcrOutput) {
    assert!(output.pages_processed > 0, "expected at least one page");
    assert_eq!(output.pages.len() as u32, output.pages_processed);
}

fn assert_has_decoded_images(output: &OcrOutput) {
    let images: Vec<_> = output.pages.iter().flat_map(|p| &p.images).collect();
    assert!(!images.is_empty(), "expected extracted images");
    for image in images {
        let data = image.data.as_deref().expect("image data was requested");
        assert!(!data.is_empty(), "image '{}' has no data", image.id);
    }
}

#[tokio::test]
#[ignore = "requires MISTRAL_API_KEY and network access"]
async fn pdf_ocr_via_public_url() {
    delay_test().await;

    let source = DocumentSource::document_url("https://arxiv.org/pdf/2201.04234");
    let output = client()
        .ocr(&source, &images_included())
        .await
        .expect("OCR failed");

    assert_consistent(&output);
    assert!(output.pages.len() > 1, "expected a multi-page document");
    assert!(!output.markdown().trim().is_empty());
    assert_has_decoded_images(&output);
}

#[tokio::test]
#[ignore = "requires MISTRAL_API_KEY and network access"]
async fn png_ocr_via_public_url() {
    delay_test().await;

    let source = DocumentSource::image_url(
        "https://raw.githubusercontent.com/mistralai/cookbook/refs/heads/main/mistral/ocr/receipt.png",
    );
    let output = client()
        .ocr(&source, &OcrOptions::default())
        .await
        .expect("OCR failed");

    assert_consistent(&output);
    assert!(!output.markdown().trim().is_empty());
}

#[tokio::test]
#[ignore = "requires MISTRAL_API_KEY and network access"]
async fn pdf_ocr_via_file_upload_workflow() {
    delay_test().await;

    let client = client();
    let files = client.files();
    let file_path = Path::new(TEST_PDF_PATH);

    let uploaded = files
        .upload(file_path, &UploadOptions::default())
        .await
        .expect("upload failed");
    assert!(!uploaded.id.is_empty());
    assert_eq!(
        uploaded.filename,
        file_path.file_name().unwrap().to_str().unwrap()
    );
    assert!(uploaded.size_bytes.is_none_or(|b| b > 0));

    // Run the remaining steps before asserting so that the file is always deleted.
    let outcome = async {
        let url = files.signed_url(&uploaded.id, 1).await?;
        assert!(url.starts_with("https://"), "unexpected signed URL: {url}");
        client
            .ocr(&DocumentSource::document_url(url), &images_included())
            .await
    }
    .await;
    let deleted = files.delete(&uploaded.id).await;

    let output = outcome.expect("signed URL or OCR failed");
    assert!(deleted.expect("delete failed"), "file was not deleted");

    assert_consistent(&output);
    assert!(output.pages[0].markdown.contains("Lorem ipsum"));
    assert_has_decoded_images(&output);
}

async fn ocr_local_file(path: &str, options: &OcrOptions) -> OcrOutput {
    let source = DocumentSource::from_path(path)
        .await
        .expect("unsupported file");
    client().ocr(&source, options).await.expect("OCR failed")
}

async fn assert_local_file_contains(path: &str, expected_text: &str) {
    let output = ocr_local_file(path, &OcrOptions::default()).await;
    assert_consistent(&output);
    assert!(
        output.markdown().contains(expected_text),
        "expected '{expected_text}' in:\n{}",
        output.markdown()
    );
}

#[tokio::test]
#[ignore = "requires MISTRAL_API_KEY and network access"]
async fn lorem_ipsum_png_ocr() {
    delay_test().await;
    assert_local_file_contains("tests/common/Lorem ipsum.png", "Lorem ipsum").await;
}

#[tokio::test]
#[ignore = "requires MISTRAL_API_KEY and network access"]
async fn lorem_ipsum_jpg_ocr() {
    delay_test().await;
    assert_local_file_contains("tests/common/Lorem ipsum.jpg", "Lorem ipsum").await;
}

#[tokio::test]
#[ignore = "requires MISTRAL_API_KEY and network access"]
async fn lorem_ipsum_with_mona_lisa_png_ocr() {
    delay_test().await;
    assert_local_file_contains("tests/common/Lorem ipsum with Mona Lisa.png", "Lorem ipsum").await;
}

#[tokio::test]
#[ignore = "requires MISTRAL_API_KEY and network access"]
async fn mona_lisa_jpg_ocr() {
    delay_test().await;

    // The image contains no text.
    let output = ocr_local_file("tests/common/Mona Lisa.jpg", &OcrOptions::default()).await;
    assert_consistent(&output);
    assert!(
        output.markdown().trim().is_empty(),
        "unexpected text: {}",
        output.markdown()
    );
}

#[tokio::test]
#[ignore = "requires MISTRAL_API_KEY and network access"]
async fn docx_ocr() {
    delay_test().await;
    assert_local_file_contains(TEST_DOCX_PATH, "Lorem ipsum").await;
}

async fn session_with_mistral() -> Session {
    dotenv::dotenv().ok();

    let mut session = Session::new();
    session
        .add_api_keys_from_env()
        .expect("API key environment variables must be valid");
    session
        .add_extension::<(), Client>((), ExtensionConfig::default())
        .await
        .expect("failed to add Mistral extension");
    session
}

async fn converter_output(test_file: &str) -> Vec<String> {
    let session = session_with_mistral().await;
    let converters: Vec<Comp<dyn Converter>> = session.resolve().unwrap();
    assert_eq!(converters.len(), 1, "expected exactly one converter");

    let readers = converters[0]
        .convert(test_file.into(), "text/markdown".parse().unwrap())
        .await
        .expect("conversion failed");
    let mut pages = Vec::new();
    for mut reader in readers {
        let mut markdown = String::new();
        reader.read_to_string(&mut markdown).await.unwrap();
        pages.push(markdown);
    }
    pages
}

#[tokio::test]
#[ignore = "requires MISTRAL_API_KEY, MISTRAL_API_KEY_PERMISSIONS and network access"]
async fn pdf_via_file_to_markdown() {
    delay_test().await;

    let pages = converter_output(TEST_PDF_PATH).await;
    assert!(
        pages[0].contains("Lorem ipsum"),
        "unexpected output: {pages:?}"
    );
}

#[tokio::test]
#[ignore = "requires MISTRAL_API_KEY, MISTRAL_API_KEY_PERMISSIONS and network access"]
async fn png_via_file_to_markdown() {
    delay_test().await;

    let pages = converter_output("tests/common/Lorem ipsum with Mona Lisa.png").await;
    assert!(
        pages[0].contains("Lorem ipsum"),
        "unexpected output: {pages:?}"
    );
}
