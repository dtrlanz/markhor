use std::{path::{Path, PathBuf}, sync::Arc};
use tokio::fs::File;

use crate::{extension, storage::{Error, Result}};

use super::{document, Document, Workspace};

/// Represents a file belonging to a Document (excluding the .markhor file).
#[derive(Debug, Clone)]
pub struct ContentFile<'a> {
    file_path: PathBuf,
    document: &'a Document,
}

impl<'a> ContentFile<'a> {
    pub(crate) fn new(path: PathBuf, document: &'a Document) -> Self {
        ContentFile { file_path: path, document }
    }

    /// Returns the absolute path to the file.
    pub fn path(&self) -> &Path {
        &self.file_path
    }

    /// Asynchronously reads the entire content of the file into a byte vector.
    pub async fn read_content(&self) -> Result<Vec<u8>> {
        tokio::fs::read(&self.file_path)
            .await
            .map_err(|e| {
                if e.kind() == std::io::ErrorKind::NotFound {
                    Error::FileNotFound(self.file_path.clone())
                } else {
                    Error::Io(e)
                }
            })
    }

    /// Asynchronously reads the entire content of the file into a String.
    pub async fn read_string(&self) -> Result<String> {
        tokio::fs::read_to_string(&self.file_path)
            .await
             .map_err(|e| {
                if e.kind() == std::io::ErrorKind::NotFound {
                    Error::FileNotFound(self.file_path.clone())
                } else {
                    Error::Io(e)
                }
            })
    }

    pub fn file_name(&self) -> Option<&str> {
        self.file_path.file_name()?.to_str()
    }

    pub fn extension(&self) -> Option<&str> {
        self.file_path.extension()?.to_str()
    }
}

pub struct Content {
    file_path: PathBuf,
    document: Document,
}

pub struct ContentBuilder {
    path: PathBuf,
    name: String,
    files: Vec<PathBuf>,
}

impl ContentBuilder {
    pub fn new(content: &Content) -> Self {
        ContentBuilder {
            path: content.file_path.clone(),
            name: content.document.name().to_string(),
            files: Vec::new(),
        }
    }

    pub async fn add_file(&mut self, extension: &str, ) -> Result<File> {
        // Find available file name, appending hex digits if necessary
        for suffix in 0..100 {
            let suffix_string = if suffix == 0 {
                String::new()
            } else {
                format!(".{:x}", suffix)
            };
            let file_name = format!("{}{}.{}", self.name, suffix_string, extension);
            let path = self.path.with_file_name(file_name);
            let result = File::create(&path).await;
            match result {
                Ok(file) => {
                    self.files.push(path);
                    return Ok(file);
                },
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(e) => return Err(Error::Io(e)),
            }
        }

        return Err(Error::ContentFileNotCreated("Too many conflicts with other content files".to_string()));
    }
}
