use serde::Deserialize;

/// The `purpose` form field for files that will be used with OCR.
pub(crate) const PURPOSE_OCR: &str = "ocr";

/// Response of `POST /v1/files`.
#[derive(Debug, Deserialize)]
pub(crate) struct FileObject {
    pub id: String,
    pub filename: String,
    /// Null in some documented examples.
    #[serde(default)]
    pub bytes: Option<u64>,
    #[serde(default)]
    pub created_at: Option<i64>,
    /// Unix timestamp; only present if an expiry was requested.
    #[serde(default)]
    pub expires_at: Option<i64>,
    #[serde(default)]
    pub mimetype: Option<String>,
    #[serde(default)]
    pub visibility: Option<String>,
}

/// Response of `GET /v1/files/{id}/url`.
#[derive(Debug, Deserialize)]
pub(crate) struct SignedUrl {
    pub url: String,
}

/// Response of `DELETE /v1/files/{id}`.
#[derive(Debug, Deserialize)]
pub(crate) struct DeleteResponse {
    pub id: String,
    pub deleted: bool,
}
