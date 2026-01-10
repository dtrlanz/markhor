use serde::{Deserialize, Serialize, de::DeserializeOwned};
use tokio::io::{AsyncRead, AsyncWriteExt};
use uuid::Uuid;
use tokio::fs::{self, OpenOptions};
use tracing::{debug, info, instrument, warn};
use std::{ffi::OsStr, ops::{Deref, DerefMut}, path::{Path, PathBuf}, sync::Arc};

use crate::{markdown::{Markdown, ToMarkdown, WITH_MILESTONES}, storage::{Error, Workspace, metadata}};


const METADATA_EXTENSION: &str = "mark";
const ATTACHMENTS_DIR: &str = "attachments";

pub struct Doc {
    /// Absolute path to the source file
    pub(crate) absolute_path: PathBuf,

    /// Workspace owning this document
    workspace: Arc<Workspace>,
    metadata: DocMetadata,
    metadata_location: MetadataLocation,
    text_parts: Vec<(String, String)>,
}

impl Doc {
    pub fn path(&self) -> &Path {
        &self.absolute_path.strip_prefix(&self.workspace.absolute_path)
            .expect("Internal error: Document is not in workspace")
    }

    fn metadata_path(&self) -> PathBuf {
        self.absolute_path.with_added_extension(METADATA_EXTENSION)
    }

    pub fn name(&self) -> &str {
        self.absolute_path.file_stem()
            .and_then(OsStr::to_str).unwrap()
    }

    pub fn id(&self) -> &Uuid {
        &self.metadata.id
    }

    pub fn text(&self) -> Option<&String> {
        self.text_parts.first().map(|(_, text)| text)
    }

    pub fn text_mut(&mut self) -> TextMut {
        if self.text_parts.len() > 0 {
            let text = &mut self.text_parts[0].1;
            TextMut::occupied(text)
        } else {
            TextMut::vacant(self, "")
        }
    }

    pub fn text_by_id(&self, id: &str) -> Option<&String> {
        self.text_parts.iter().find(|(part_id, _)| part_id == id).map(|(_, text)| text)
    }

    pub fn text_mut_by_id<'a>(&'a mut self, id: &'a str) -> TextMut<'a> {
        if let Some((idx, _)) = self.text_parts.iter().enumerate().find(|(_i, (part_id, _))| part_id == id) {
            let text = &mut self.text_parts[idx].1;
            TextMut::occupied(text)
        } else {
            TextMut::vacant(self, id)
        }
    }

    fn new_internal(
        absolute_path: PathBuf,
        workspace: Arc<Workspace>,
        metadata: DocMetadata,
        metadata_location: MetadataLocation,
        text: Option<String>,
    ) -> Result<Self, Error> {

        let mut doc = Self {
            absolute_path,
            workspace,
            metadata,
            metadata_location,
            text_parts: vec![],
        };

        match (text, doc.metadata.doc_parts.as_ref()) {
            // Text representation of document only has one part
            (Some(text), None) => {
                doc.text_mut().or_insert(text);
            },
            // Text representation has multiple parts (e.g., spreadsheet converted to multiple
            // tables in markdown or CSV)
            (Some(text), Some(_parts)) => {
                for r in text.to_markdown(WITH_MILESTONES).regions() {
                    if &*r.unit == "part" {
                        let mut id = r.attribute("id").flatten().map(|s| s.to_string());
                        if let Some(part_id) = id {
                            doc.text_mut_by_id(&part_id).or_insert(r.as_ref().content.to_string());
                        } else {
                            doc.text_mut().or_insert(r.as_ref().content.to_string());
                        }
                    }
                }
            },
            // No text representation available
            (None, _) => (),
        };

        Ok(doc)
    }

    pub(crate) async fn open(
        absolute_path: PathBuf,
        workspace: Arc<Workspace>,
    ) -> Result<Self, Error> {
        // Attempt to load metadata from metadata file
        let md_path = absolute_path.with_added_extension(METADATA_EXTENSION);
        match read_markdown_file::<DocMetadata>(&md_path).await {
            Ok((text, Ok(metadata))) => {
                let mut text_option = None;
                match metadata.text_location {
                    TextLocation::SourceFile => {
                        // Read source file
                        text_option = Some(fs::read_to_string(&absolute_path).await?);
                    },
                    TextLocation::MetadataFile => {
                        text_option = Some(text);
                    },
                    TextLocation::Inferred => {
                        if text.trim_start().is_empty() {
                            text_option = Some(fs::read_to_string(&absolute_path).await?);
                        }
                    },
                    TextLocation::None => (),
                }
                return Self::new_internal(
                    absolute_path,
                    workspace,
                    metadata,
                    MetadataLocation::MetadataFile,
                    text_option,
                );
            },
            Ok((text, Err(e))) => {
                return Err(Error::Metadata2(e));
            },
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                // Metadata file not found, try source file
                debug!("Metadata file not found for document {:?}, trying source file", absolute_path);
            },
            Err(e) => {
                return Err(Error::Io(e));
            }
        }

        // Read text and metadata from source file
        let mut metadata_location = MetadataLocation::SourceFile;
        let (text, metadata_result) = read_markdown_file::<DocMetadata>(&absolute_path).await?;
        let metadata = match metadata_result {
            Ok(metadata) => metadata,
            Err(e) => {
                debug!("Failed to read metadata from source file {:?}: {}", absolute_path, e);
                // Create metadata file
                let metadata = DocMetadata::new(&absolute_path);
                write_markdown_file(&md_path, "", &metadata).await?;
                metadata_location = MetadataLocation::MetadataFile;
                info!("Created missing metadata file for document {:?}", absolute_path);
                metadata
            }
        };
        
        Self::new_internal(
            absolute_path,
            workspace,
            metadata,
            metadata_location,
            Some(text),
        )
    }
}

async fn read_markdown_file<T: DeserializeOwned>(path: &Path) -> Result<(String, Result<T, serde_yaml_ng::Error>), std::io::Error> {
    let content = fs::read_to_string(path).await?;
    let markdown = content.to_markdown(WITH_MILESTONES);
    let metadata_result = markdown.metadata::<T>();
    let text = String::from(markdown.skip_metadata().as_ref());
    Ok((text, metadata_result))
}

async fn write_markdown_file<T: Serialize + ?Sized>(path: &Path, text: &str, metadata: &T) -> Result<(), Error> {
    let metadata = serde_yaml_ng::to_string(metadata)?;
    let markdown = format!("{}\n{}", metadata, text);
    let mut file = OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(path)
        .await?;
    file.write_all(markdown.as_bytes()).await?;
    Ok(())
}

pub enum MetadataLocation {
    SourceFile,
    MetadataFile,
    None,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DocMetadata {
    pub id: Uuid,
    pub mime: String,
    #[serde(default)] #[serde(skip_serializing_if = "is_default")]
    pub text_location: TextLocation,
    #[serde(default)] #[serde(skip_serializing_if = "is_default")]
    pub doc_parts: Option<String>,
    #[serde(flatten)]
    other_fields: serde_yaml_ng::Mapping,
}

impl DocMetadata {
    pub fn new(path: &Path) -> Self {
        Self {
            id: Uuid::new_v4(),
            // TODO: infer mime type from extension
            mime: "text/markdown".to_string(),
            text_location: Default::default(),
            doc_parts: Default::default(),
            other_fields: Default::default(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum TextLocation {
    SourceFile,
    MetadataFile,
    Inferred,
    None,
}

impl Default for TextLocation {
    fn default() -> Self {
        TextLocation::Inferred
    }
}

fn is_default<T: Default + PartialEq>(value: &T) -> bool {
    value == &T::default()
}

#[derive(Debug, Clone)]
pub struct Text {
    id: String,
    content: Arc<String>,
}

impl Text {
    pub fn id(&self) -> &str {
        &self.id
    }

    pub fn as_str(&self) -> &str {
        &self.content
    }

    pub fn make_mut(&mut self) -> &mut String {
        Arc::make_mut(&mut self.content)
    }
}

pub struct TextMut<'a> {
    inner: TextMutInner<'a>,
}

enum TextMutInner<'a> {
    Occupied(Option<&'a mut String>),
    Vacant(&'a mut Doc, &'a str, Option<&'a mut String>),
}

impl<'a> TextMut<'a> {
    fn occupied(text: &'a mut String) -> Self {
        Self {
            inner: TextMutInner::Occupied(Some(text)),
        }
    }

    fn vacant(doc: &'a mut Doc, id: &'a str) -> Self {
        Self {
            inner: TextMutInner::Vacant(doc, id, None),
        }
    }

    pub fn or_insert(&mut self, text: String) -> &mut String {
        match &mut self.inner {
            TextMutInner::Occupied(Some(entry)) => {
                entry
            },
            TextMutInner::Vacant(doc, id, _) => {
                doc.text_parts.push((id.to_string(), text));
                &mut doc.text_parts.last_mut().unwrap().1
            },
            TextMutInner::Occupied(None) => {
                // TextMutInner::Occupied(None) is never constructed
                unreachable!();
            },
        }
    }
}

impl<'a> Deref for TextMut<'a> {
    type Target = Option<&'a mut String>;

    fn deref(&self) -> &Option<&'a mut String> {
        match &self.inner {
            TextMutInner::Occupied(entry) => entry,
            TextMutInner::Vacant(_, _, entry) => entry,
        }
    }
}

impl<'a> DerefMut for TextMut<'a> {
    fn deref_mut(&mut self) -> &mut Option<&'a mut String> {
        match &mut self.inner {
            TextMutInner::Occupied(entry) => entry,
            TextMutInner::Vacant(_, _, entry) => entry,
 
        }
    }
}
