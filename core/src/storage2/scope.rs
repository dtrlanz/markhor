use std::{collections::HashSet, path::PathBuf};

use crate::storage2::{AccessStorageError, Document, Tag, Workspace, folder::ReadRecursive};



pub struct Scope {
    // Absolute path to the folder
    folder_path: PathBuf,
    tags: HashSet<Tag>,
}

impl Scope {
    pub fn contains(&self, document: &Document) -> bool {
        // Check path
        if !document.path().starts_with(&self.folder_path) {
            return false;
        }
        // Check tags
        for tag in &self.tags {
            if !document.tags().any(|t| t == tag) {
                return false;
            }
        }
        true
    }

    pub async fn docs(&self, workspace: &Workspace) -> Result<Docs<'_>, AccessStorageError> {
        // Check if workspace contains the scope's path
        if !self.folder_path.starts_with(workspace.path()) {
            return Err(AccessStorageError::NotInWorkspace(self.folder_path.clone()));
        }
        let folder = workspace.folder(&self.folder_path).await?;
        let read_recursive = folder.read_recursive().await?;
        Ok(Docs {
            scope: self,
            doc_stream: read_recursive,
        })
    }
}

pub struct Docs<'a> {
    scope: &'a Scope,
    doc_stream: ReadRecursive,
}

impl<'a> Docs<'a> {
    pub async fn next_doc(&mut self) -> Result<Option<Document>, AccessStorageError> {
        while let Some(doc) = self.doc_stream.next_entry().await? {
            if self.scope.contains(&doc) {
                return Ok(Some(doc));
            }
        }
        Ok(None)
    }
}



/// A preliminary filter that can be applied to a scope to quickly determine if it may include 
/// a specific document.
/// 
/// This is used to avoid expensive operations when we can determine that a document is 
/// definitely not included in the scope. The filter may return false positives (indicating that 
/// a document may be included when it is not), but is guaranteed not to return false negatives 
/// (indicating that a document is not included when it is).
/// 
/// # To do
/// 
/// This is a placeholder implementation that always returns true. Actual implementation should be
/// fairly simple, probably involving pigeonholes, bitwise operations, and nice things like that.
#[derive(Debug, Clone)]
pub struct PrelimFilter {
    // TODO
}

impl PrelimFilter {
    /// Returns true if the document from which this filter was created may be included in the 
    /// given scope, false if it is definitely not.
    pub fn maybe_matches(&self, scope: &Scope) -> bool {
        // TODO
        true
    }
}

impl From<&Document> for PrelimFilter {
    fn from(document: &Document) -> Self {
        // TODO
        PrelimFilter {}
    }
}