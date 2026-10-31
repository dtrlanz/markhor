use base64::{Engine, engine::general_purpose::STANDARD};
use markhor_core::chat::common::ContentPart;
use serde::{Deserialize, Serialize, Serializer};
use serde_json::{Map, Value};

/// One entry in the conversation history.
///
/// Google calls this `Content`, but it is really a role plus a list of parts, and
/// the permitted parts differ by role, so each role gets its own type.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "role", rename_all = "lowercase")]
pub(crate) enum Turn {
    User(UserTurn),
    Model(ModelTurn),
}

impl From<UserTurn> for Turn {
    fn from(turn: UserTurn) -> Self {
        Turn::User(turn)
    }
}

impl From<ModelTurn> for Turn {
    fn from(turn: ModelTurn) -> Self {
        Turn::Model(turn)
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct UserTurn {
    pub parts: Vec<UserPart>,
}

/// A turn produced by the model, as received in a candidate or sent back as history.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct ModelTurn {
    /// The API omits `parts` when generation stopped before producing any output.
    #[serde(default)]
    pub parts: Vec<ModelPart>,
}

/// Instructions steering the model. Google currently accepts text only (no role, no media),
/// so this is deliberately not a [`Turn`]. Each string is sent as a separate part.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct SystemInstruction(pub Vec<String>);

impl Serialize for SystemInstruction {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        #[derive(Serialize)]
        struct Text<'a> {
            text: &'a str,
        }
        #[derive(Serialize)]
        struct Wire<'a> {
            parts: Vec<Text<'a>>,
        }
        Wire {
            parts: self.0.iter().map(|text| Text { text }).collect(),
        }
        .serialize(serializer)
    }
}

/// Something the user (or the application on their behalf) can say.
///
/// Serialized as an object with a single key naming the variant, e.g. `{"text": "hi"}`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) enum UserPart {
    Text(String),
    InlineData(Blob),
    FunctionResponse(FunctionResponse),
}

/// A part of a model turn: what was said, plus the optional reasoning signature.
///
// TODO(tool support): signatures must be echoed back on the same part they arrived on
// (mandatory for function calls on Gemini 3+, recommended otherwise). `ContentPart` and
// `Msg::Agent` currently have no per-part slot for them, so they are dropped when converting
// a response to core types. When adding tool support, extend `Msg::Agent` (e.g., a
// provider-opaque value per content part / tool call, or via `reasoning_content`) and use it
// to rebuild `ModelPart`s when mapping history back to Google turns.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(try_from = "RawModelPart", into = "RawModelPart")]
pub(crate) struct ModelPart {
    pub kind: ModelPartKind,
    pub signature: Option<ThoughtSignature>,
}

impl From<ModelPartKind> for ModelPart {
    fn from(kind: ModelPartKind) -> Self {
        Self {
            kind,
            signature: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum ModelPartKind {
    Text(String),
    /// A summary of the model's reasoning (only returned if requested).
    Thought(String),
    InlineData(Blob),
    FunctionCall(FunctionCall),
    /// A part kind not (yet) modelled here, kept verbatim so it can be sent back unchanged.
    Unsupported(Map<String, Value>),
}

/// Encrypted representation of the model's reasoning for a part.
///
/// Must be passed back exactly as received, attached to the same part.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub(crate) struct ThoughtSignature(String);

impl std::fmt::Debug for ThoughtSignature {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "ThoughtSignature(<{} bytes>)", self.0.len())
    }
}

/// Raw media bytes, base64-encoded on the wire.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Blob {
    pub mime_type: String,
    #[serde(with = "base64_bytes")]
    pub data: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct FunctionCall {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    pub name: String,
    #[serde(default, skip_serializing_if = "Value::is_null")]
    pub args: Value,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct FunctionResponse {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    pub name: String,
    pub response: Value,
}

/// Wire format of a model part: a flat object where the data kind is implied by
/// which field is present. Only used to (de)serialize [`ModelPart`].
#[derive(Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawModelPart {
    #[serde(skip_serializing_if = "Option::is_none")]
    text: Option<String>,
    #[serde(default, skip_serializing_if = "is_false")]
    thought: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    inline_data: Option<Blob>,
    #[serde(skip_serializing_if = "Option::is_none")]
    function_call: Option<FunctionCall>,
    #[serde(skip_serializing_if = "Option::is_none")]
    thought_signature: Option<ThoughtSignature>,
    #[serde(flatten)]
    other: Map<String, Value>,
}

fn is_false(value: &bool) -> bool {
    !*value
}

impl TryFrom<RawModelPart> for ModelPart {
    type Error = &'static str;

    fn try_from(raw: RawModelPart) -> Result<Self, Self::Error> {
        let kind = match (raw.text, raw.inline_data, raw.function_call) {
            (Some(text), None, None) if raw.thought => ModelPartKind::Thought(text),
            (Some(text), None, None) => ModelPartKind::Text(text),
            (None, Some(blob), None) => ModelPartKind::InlineData(blob),
            (None, None, Some(call)) => ModelPartKind::FunctionCall(call),
            (None, None, None) if !raw.other.is_empty() => ModelPartKind::Unsupported(raw.other),
            (None, None, None) => return Err("part has no data"),
            _ => return Err("part has more than one kind of data"),
        };
        Ok(ModelPart {
            kind,
            signature: raw.thought_signature,
        })
    }
}

impl From<ModelPart> for RawModelPart {
    fn from(part: ModelPart) -> Self {
        let mut raw = RawModelPart {
            thought_signature: part.signature,
            ..Default::default()
        };
        match part.kind {
            ModelPartKind::Text(text) => raw.text = Some(text),
            ModelPartKind::Thought(text) => {
                raw.text = Some(text);
                raw.thought = true;
            }
            ModelPartKind::InlineData(blob) => raw.inline_data = Some(blob),
            ModelPartKind::FunctionCall(call) => raw.function_call = Some(call),
            ModelPartKind::Unsupported(other) => raw.other = other,
        }
        raw
    }
}

mod base64_bytes {
    use super::*;
    use serde::Deserializer;

    pub fn serialize<S: Serializer>(bytes: &[u8], serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&STANDARD.encode(bytes))
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Vec<u8>, D::Error> {
        let s = String::deserialize(deserializer)?;
        STANDARD.decode(s).map_err(serde::de::Error::custom)
    }
}

impl From<ContentPart> for UserPart {
    fn from(part: ContentPart) -> Self {
        match part {
            ContentPart::Text(text) => UserPart::Text(text),
            ContentPart::Image { mime_type, data } => UserPart::InlineData(Blob { mime_type, data }),
        }
    }
}

impl From<ContentPart> for ModelPartKind {
    fn from(part: ContentPart) -> Self {
        match part {
            ContentPart::Text(text) => ModelPartKind::Text(text),
            ContentPart::Image { mime_type, data } => {
                ModelPartKind::InlineData(Blob { mime_type, data })
            }
        }
    }
}

impl TryFrom<ModelPartKind> for ContentPart {
    /// Kinds that aren't plain content (thoughts, function calls, ...) are handed back unchanged.
    // TODO(tool support): the caller must route `FunctionCall` kinds to `ToolCallRequest`
    // (Google's call id is optional, so one may need to be synthesized), carrying along the
    // part's signature; see `ModelPart`.
    type Error = ModelPartKind;

    fn try_from(kind: ModelPartKind) -> Result<Self, Self::Error> {
        match kind {
            ModelPartKind::Text(text) => Ok(ContentPart::Text(text)),
            // The API may return any media type; only images are representable in `ContentPart`.
            ModelPartKind::InlineData(Blob { mime_type, data }) if mime_type.starts_with("image/") => {
                Ok(ContentPart::Image { mime_type, data })
            }
            other => Err(other),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn roundtrip<T>(value: &T, expected: Value)
    where
        T: Serialize + for<'de> Deserialize<'de> + PartialEq + std::fmt::Debug,
    {
        let json = serde_json::to_value(value).unwrap();
        assert_eq!(json, expected);
        assert_eq!(&serde_json::from_value::<T>(json).unwrap(), value);
    }

    #[test]
    fn user_turn_roundtrip() {
        let turn = Turn::from(UserTurn {
            parts: vec![
                UserPart::Text("hello".into()),
                ContentPart::Image {
                    mime_type: "image/png".into(),
                    data: vec![1, 2, 3, 250],
                }
                .into(),
            ],
        });
        roundtrip(
            &turn,
            json!({"role": "user", "parts": [
                {"text": "hello"},
                {"inlineData": {"mimeType": "image/png", "data": "AQID+g=="}},
            ]}),
        );
    }

    #[test]
    fn function_response_is_a_user_part() {
        let part = UserPart::FunctionResponse(FunctionResponse {
            id: None,
            name: "f".into(),
            response: json!({"ok": true}),
        });
        roundtrip(
            &part,
            json!({"functionResponse": {"name": "f", "response": {"ok": true}}}),
        );
    }

    #[test]
    fn model_turn_with_signature_roundtrip() {
        let turn = Turn::Model(ModelTurn {
            parts: vec![ModelPart {
                kind: ModelPartKind::Text("answer".into()),
                signature: Some(ThoughtSignature("c2ln".into())),
            }],
        });
        roundtrip(
            &turn,
            json!({"role": "model", "parts": [{"text": "answer", "thoughtSignature": "c2ln"}]}),
        );
    }

    #[test]
    fn thought_summary_roundtrip() {
        let part = ModelPart::from(ModelPartKind::Thought("thinking".into()));
        roundtrip(&part, json!({"text": "thinking", "thought": true}));
    }

    #[test]
    fn function_call_without_id_or_args() {
        let part: ModelPart =
            serde_json::from_value(json!({"functionCall": {"name": "f"}})).unwrap();
        assert_eq!(
            part.kind,
            ModelPartKind::FunctionCall(FunctionCall {
                id: None,
                name: "f".into(),
                args: Value::Null
            })
        );
        assert_eq!(
            serde_json::to_value(&part).unwrap(),
            json!({"functionCall": {"name": "f"}})
        );
    }

    #[test]
    fn unsupported_part_is_preserved() {
        let json = json!({
            "executableCode": {"language": "PYTHON", "code": "print(1)"},
            "thoughtSignature": "c2ln"
        });
        let part: ModelPart = serde_json::from_value(json.clone()).unwrap();
        assert!(matches!(part.kind, ModelPartKind::Unsupported(_)));
        assert!(part.signature.is_some());
        assert_eq!(serde_json::to_value(&part).unwrap(), json);
    }

    #[test]
    fn invalid_model_parts_are_rejected() {
        assert!(serde_json::from_value::<ModelPart>(json!({})).is_err());
        assert!(
            serde_json::from_value::<ModelPart>(
                json!({"text": "a", "functionCall": {"name": "f"}})
            )
            .is_err()
        );
    }

    #[test]
    fn model_turn_deserializes_with_role_and_without_parts() {
        let turn: ModelTurn = serde_json::from_value(json!({"role": "model"})).unwrap();
        assert!(turn.parts.is_empty());
    }

    #[test]
    fn system_instruction_is_text_only_without_role() {
        let instruction = SystemInstruction(vec!["be nice".into(), "be brief".into()]);
        assert_eq!(
            serde_json::to_value(&instruction).unwrap(),
            json!({"parts": [{"text": "be nice"}, {"text": "be brief"}]})
        );
    }

    #[test]
    fn content_part_conversions() {
        let image = ContentPart::Image {
            mime_type: "image/jpeg".into(),
            data: vec![9],
        };
        let kind = ModelPartKind::from(image.clone());
        assert_eq!(ContentPart::try_from(kind), Ok(image));

        let audio = ModelPartKind::InlineData(Blob {
            mime_type: "audio/wav".into(),
            data: vec![0],
        });
        assert_eq!(ContentPart::try_from(audio.clone()), Err(audio));

        let thought = ModelPartKind::Thought("hmm".into());
        assert_eq!(ContentPart::try_from(thought.clone()), Err(thought));
    }

    #[test]
    fn signature_debug_does_not_leak_contents() {
        let debug = format!("{:?}", ThoughtSignature("secretsecret".into()));
        assert!(!debug.contains("secret"));
    }
}
