use std::path::Path;

use reqwest::{
    Body,
    multipart::{Form, Part},
};
use tokio_util::io::ReaderStream;

use super::{ClientInner, Error, api};

/// Who can see an uploaded file.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum FileVisibility {
    /// Only the user who uploaded the file.
    #[default]
    User,
    /// Everyone in the workspace.
    Workspace,
}

impl FileVisibility {
    fn as_str(self) -> &'static str {
        match self {
            FileVisibility::User => "user",
            FileVisibility::Workspace => "workspace",
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct UploadOptions {
    pub visibility: FileVisibility,
}

/// Metadata of a file stored with Mistral.
#[derive(Debug, Clone)]
pub struct UploadedFile {
    pub id: String,
    pub filename: String,
    pub size_bytes: Option<u64>,
}

/// Files stored with Mistral, e.g. to process large documents by reference. Obtained from
/// [`Client::files`](super::Client::files).
///
/// Uploaded files persist until deleted, so callers should call [`Files::delete`] when done.
pub struct Files<'a> {
    inner: &'a ClientInner,
}

impl<'a> Files<'a> {
    pub(super) fn new(inner: &'a ClientInner) -> Self {
        Self { inner }
    }

    /// Uploads a local file for use with OCR, streaming it from disk.
    pub async fn upload(
        &self,
        path: impl AsRef<Path>,
        options: &UploadOptions,
    ) -> Result<UploadedFile, Error> {
        let path = path.as_ref();
        let filename = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .ok_or_else(|| Error::UnsupportedFileType(path.display().to_string()))?;
        let file = tokio::fs::File::open(path).await?;
        let len = file.metadata().await?.len();
        let body = Body::wrap_stream(ReaderStream::new(file));
        self.send_upload(Part::stream_with_length(body, len), filename, options)
            .await
    }

    /// Uploads in-memory data for use with OCR.
    pub async fn upload_bytes(
        &self,
        filename: impl Into<String>,
        data: Vec<u8>,
        options: &UploadOptions,
    ) -> Result<UploadedFile, Error> {
        self.send_upload(Part::bytes(data), filename.into(), options)
            .await
    }

    async fn send_upload(
        &self,
        part: Part,
        filename: String,
        options: &UploadOptions,
    ) -> Result<UploadedFile, Error> {
        let form = Form::new()
            .text("purpose", api::PURPOSE_OCR)
            .text("visibility", options.visibility.as_str())
            .part("file", part.file_name(filename));
        let file: api::FileObject = api::send(self.inner.post("files").multipart(form)).await?;
        Ok(UploadedFile {
            id: file.id,
            filename: file.filename,
            size_bytes: file.bytes,
        })
    }

    /// A temporary URL from which the file can be downloaded, valid for `expiry_hours`
    /// (1 to 168). Pass it to [`DocumentSource::document_url`](super::DocumentSource) or
    /// `image_url`, or use [`DocumentSource::file`](super::DocumentSource::file) directly.
    pub async fn signed_url(&self, file_id: &str, expiry_hours: u32) -> Result<String, Error> {
        let request = self
            .inner
            .get(&format!("files/{file_id}/url"))
            .query(&[("expiry", expiry_hours)]);
        let response: api::SignedUrl = api::send(request).await?;
        Ok(response.url)
    }

    /// Deletes a file. Returns whether the API reports it as deleted.
    pub async fn delete(&self, file_id: &str) -> Result<bool, Error> {
        let response: api::DeleteResponse =
            api::send(self.inner.delete(&format!("files/{file_id}"))).await?;
        Ok(response.deleted)
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;
    use wiremock::{
        Mock, MockServer, ResponseTemplate,
        matchers::{body_string_contains, header_regex, method, path, query_param},
    };

    use super::*;
    use crate::mistral::test_client;

    #[tokio::test]
    async fn upload_sends_purpose_and_visibility() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/files"))
            .and(header_regex("content-type", "^multipart/form-data"))
            .and(body_string_contains("name=\"purpose\"\r\n\r\nocr"))
            .and(body_string_contains("name=\"visibility\"\r\n\r\nuser"))
            .and(body_string_contains("filename=\"doc.pdf\""))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "id": "file-1", "object": "file", "bytes": null, "filename": "doc.pdf",
                "purpose": "ocr", "sample_type": "ocr_input", "source": "upload"
            })))
            .expect(1)
            .mount(&server)
            .await;

        let client = test_client(&server);
        let file = client
            .files()
            .upload_bytes("doc.pdf", b"%PDF".to_vec(), &UploadOptions::default())
            .await
            .unwrap();
        assert_eq!(file.id, "file-1");
        assert_eq!(file.filename, "doc.pdf");
        assert_eq!(file.size_bytes, None);
    }

    #[tokio::test]
    async fn signed_url_passes_expiry() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/v1/files/file-1/url"))
            .and(query_param("expiry", "2"))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(json!({"url": "https://signed"})),
            )
            .expect(1)
            .mount(&server)
            .await;

        let url = test_client(&server)
            .files()
            .signed_url("file-1", 2)
            .await
            .unwrap();
        assert_eq!(url, "https://signed");
    }
}
