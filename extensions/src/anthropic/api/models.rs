use serde::Deserialize;

/// Body of `GET /v1/models`, paginated by cursor (`after_id`, `before_id`, `limit`).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub(crate) struct ListModelsResponse {
    pub data: Vec<ModelInfo>,
    #[serde(default)]
    pub has_more: bool,
    #[serde(default)]
    pub first_id: Option<String>,
    /// Pass as `after_id` to fetch the next page.
    #[serde(default)]
    pub last_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub(crate) struct ModelInfo {
    /// The identifier to use as `model` in requests.
    pub id: String,
    #[serde(default)]
    pub display_name: String,
    /// RFC 3339 release date.
    #[serde(default)]
    pub created_at: Option<String>,
    #[serde(default)]
    pub capabilities: Option<ModelCapabilities>,
}

/// The subset of model capabilities that Markhor looks at. Absent keys are `None`.
#[derive(Debug, Clone, PartialEq, Eq, Default, Deserialize)]
pub(crate) struct ModelCapabilities {
    #[serde(default)]
    pub image_input: Option<CapabilitySupport>,
    #[serde(default)]
    pub pdf_input: Option<CapabilitySupport>,
    #[serde(default)]
    pub structured_outputs: Option<CapabilitySupport>,
    #[serde(default)]
    pub thinking: Option<CapabilitySupport>,
    #[serde(default)]
    pub effort: Option<CapabilitySupport>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
pub(crate) struct CapabilitySupport {
    #[serde(default)]
    pub supported: bool,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn model_list_page_deserializes() {
        let page: ListModelsResponse = serde_json::from_value(json!({
            "data": [
                {
                    "type": "model",
                    "id": "claude-sonnet-4-6",
                    "display_name": "Claude Sonnet 4.6",
                    "created_at": "2026-02-17T00:00:00Z",
                    "capabilities": {
                        "batch": {"supported": true},
                        "image_input": {"supported": true},
                        "pdf_input": {"supported": true},
                        "thinking": {"supported": true, "types": {"adaptive": {"supported": true}}},
                        "effort": {"supported": false, "high": {"supported": false}}
                    }
                },
                {"type": "model", "id": "claude-old", "display_name": "Old", "capabilities": null}
            ],
            "has_more": true,
            "first_id": "claude-sonnet-4-6",
            "last_id": "claude-old"
        }))
        .unwrap();

        assert_eq!(page.data.len(), 2);
        assert!(page.has_more);
        assert_eq!(page.last_id.as_deref(), Some("claude-old"));

        let caps = page.data[0].capabilities.as_ref().unwrap();
        assert_eq!(
            caps.image_input,
            Some(CapabilitySupport { supported: true })
        );
        assert_eq!(caps.thinking, Some(CapabilitySupport { supported: true }));
        assert_eq!(caps.effort, Some(CapabilitySupport { supported: false }));
        assert_eq!(caps.structured_outputs, None);

        assert_eq!(page.data[1].capabilities, None);
        assert_eq!(page.data[1].created_at, None);
    }

    #[test]
    fn last_page_without_cursors() {
        let page: ListModelsResponse = serde_json::from_value(json!({
            "data": [],
            "has_more": false,
            "first_id": null,
            "last_id": null
        }))
        .unwrap();
        assert!(page.data.is_empty());
        assert!(!page.has_more);
        assert_eq!(page.last_id, None);
    }
}
