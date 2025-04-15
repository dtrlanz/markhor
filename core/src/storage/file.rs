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

    pub fn file_name(&self) -> &str {
        self.file_path.file_name().unwrap().to_str().unwrap()
    }

    pub fn extension(&self) -> &str {
        self.file_path.extension().unwrap().to_str().unwrap()
    }
}

#[derive(Debug, Clone)]
pub enum Content {
    File(PathBuf),
}

impl Content {
    pub fn path(&self) -> &Path {
        match self {
            Content::File(path) => path,
        }
    }
}