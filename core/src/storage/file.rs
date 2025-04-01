use std::path::{Path, PathBuf};
use tokio::fs::File;

use crate::storage::{Error, Result};

/// Represents a file belonging to a Document (excluding the .markhor file).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ContentFile {
    path: PathBuf,
}

impl ContentFile {
    // Constructor is likely internal, created by Document methods
    pub(crate) fn new(path: PathBuf) -> Self {
        ContentFile { path }
    }

    /// Returns the absolute path to the file.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Asynchronously reads the entire content of the file into a byte vector.
    pub async fn read_content(&self) -> Result<Vec<u8>> {
        tokio::fs::read(&self.path)
            .await
            .map_err(|e| {
                if e.kind() == std::io::ErrorKind::NotFound {
                    Error::FileNotFound(self.path.clone())
                } else {
                    Error::Io(e)
                }
            })
    }

    /// Asynchronously reads the entire content of the file into a String.
    pub async fn read_string(&self) -> Result<String> {
        tokio::fs::read_to_string(&self.path)
            .await
             .map_err(|e| {
                if e.kind() == std::io::ErrorKind::NotFound {
                    Error::FileNotFound(self.path.clone())
                } else {
                    Error::Io(e)
                }
            })
    }

    // Add other methods as needed, e.g., getting extension, filename
    pub fn file_name(&self) -> Option<&str> {
        self.path.file_name()?.to_str()
    }

    pub fn extension(&self) -> Option<&str> {
        self.path.extension()?.to_str()
    }
}

pub struct Content {
    file: File,
}

pub struct ContentBuilder {
    file: File,
}