//! Wire-level types and plumbing for the Mistral API (`/v1/ocr`, `/v1/files`).
//!
//! Only the fields this crate uses are modelled; unknown response fields are ignored. Fields the
//! API documents as nullable or optional are `Option`s. Every endpoint goes through [`send`], so
//! status handling and error parsing live in one place.

mod error;
mod files;
mod ocr;

use reqwest::RequestBuilder;
use serde::de::DeserializeOwned;

use crate::mistral::Error;

pub(crate) use error::ErrorResponse;
pub(crate) use files::{DeleteResponse, FileObject, PURPOSE_OCR, SignedUrl};
pub(crate) use ocr::{Document, Image, OcrRequest, OcrResponse};

pub(crate) const DEFAULT_BASE_URL: &str = "https://api.mistral.ai/v1";

/// Sends a request and deserializes a successful JSON response.
pub(crate) async fn send<T: DeserializeOwned>(request: RequestBuilder) -> Result<T, Error> {
    let response = request.send().await.map_err(network_error)?;
    let status = response.status();
    let body = response.bytes().await.map_err(network_error)?;
    if !status.is_success() {
        return Err(ErrorResponse::into_error(status, &body));
    }
    Ok(serde_json::from_slice(&body)?)
}

/// Wraps a transport error, dropping the URL so that it can't leak into logs or messages.
fn network_error(error: reqwest::Error) -> Error {
    Error::Network(error.without_url())
}
