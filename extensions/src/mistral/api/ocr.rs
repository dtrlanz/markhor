use serde::{Deserialize, Serialize};

/// Body of `POST /v1/ocr`.
#[derive(Debug, Serialize)]
pub(crate) struct OcrRequest<'a> {
    pub model: &'a str,
    pub document: &'a Document,
    /// Zero-based page numbers to process. All pages if unset.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pages: Option<&'a [u32]>,
    pub include_image_base64: bool,
    /// Maximum number of images to extract. `0` is required to opt out of images for `.docx`,
    /// which rejects `include_image_base64: false` otherwise.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub image_limit: Option<u32>,
}

/// The document to process, tagged by `"type"`.
///
/// `document_url` and `image_url` accept either an http(s) URL or a base64 `data:` URL.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub(crate) enum Document {
    /// PDF, PPTX or DOCX.
    DocumentUrl { document_url: String },
    /// PNG, JPEG, AVIF and other image formats.
    ImageUrl { image_url: String },
    /// A file previously uploaded through the Files API.
    File { file_id: String },
}

/// Response of `POST /v1/ocr`.
///
/// Not modelled (ignored when present): tables, hyperlinks, headers/footers, blocks, confidence
/// scores and annotations.
#[derive(Debug, Deserialize)]
pub(crate) struct OcrResponse {
    pub pages: Vec<Page>,
    #[serde(default)]
    pub model: String,
    #[serde(default)]
    pub usage_info: UsageInfo,
}

#[derive(Debug, Deserialize)]
pub(crate) struct Page {
    pub index: u32,
    pub markdown: String,
    #[serde(default)]
    pub images: Vec<Image>,
    /// Nullable in the API; some responses contain an empty object.
    #[serde(default)]
    pub dimensions: Option<Dimensions>,
}

#[derive(Debug, Default, Deserialize)]
pub(crate) struct Dimensions {
    pub dpi: Option<u32>,
    pub height: Option<u32>,
    pub width: Option<u32>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct Image {
    /// Matches the file name referenced by the page markdown, e.g. `img-0.jpeg`.
    pub id: String,
    pub top_left_x: Option<i64>,
    pub top_left_y: Option<i64>,
    pub bottom_right_x: Option<i64>,
    pub bottom_right_y: Option<i64>,
    /// Only present when `include_image_base64` was set. Usually a `data:` URL.
    #[serde(default)]
    pub image_base64: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
pub(crate) struct UsageInfo {
    #[serde(default)]
    pub pages_processed: u32,
    #[serde(default)]
    pub doc_size_bytes: Option<u64>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn document_serializes_with_type_tag() {
        let doc = Document::DocumentUrl {
            document_url: "https://example.com/a.pdf".into(),
        };
        assert_eq!(
            serde_json::to_value(&doc).unwrap(),
            json!({"type": "document_url", "document_url": "https://example.com/a.pdf"})
        );
    }

    #[test]
    fn response_tolerates_null_and_empty_dimensions() {
        let response: OcrResponse = serde_json::from_value(json!({
            "pages": [
                {"index": 0, "markdown": "a", "images": [], "dimensions": null},
                {"index": 1, "markdown": "b", "images": [{
                    "id": "img-0.jpeg", "top_left_x": null, "top_left_y": null,
                    "bottom_right_x": null, "bottom_right_y": null, "image_base64": null
                }], "dimensions": {}}
            ],
            "model": "mistral-ocr-latest",
            "usage_info": {"pages_processed": 2, "doc_size_bytes": null}
        }))
        .unwrap();
        assert!(response.pages[0].dimensions.is_none());
        assert!(response.pages[1].images[0].image_base64.is_none());
        assert_eq!(response.usage_info.pages_processed, 2);
    }
}
