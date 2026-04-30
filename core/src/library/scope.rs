use std::{collections::HashSet, path::{Path, PathBuf}};

use crate::library::{AccessLibraryError, Document, Folder, Tag, Workspace, folder::ReadRecursive};



#[derive(Debug, Clone)]
pub struct Scope {
    // Absolute path to the folder
    folder_path: PathBuf,
    tags: HashSet<Tag>,
}

impl Scope {
    pub fn contains(&self, document: &Document) -> bool {
        // Check path
        if !document.workspace().path().join(document.path()).starts_with(&self.folder_path) {
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

    pub async fn docs(&self, workspace: &Workspace) -> Result<Docs<'_>, AccessLibraryError> {
        // Check if workspace contains the scope's path
        if !self.folder_path.starts_with(workspace.path()) {
            return Err(AccessLibraryError::NotInWorkspace(self.folder_path.clone()));
        }
        let folder = workspace.folder(&self.folder_path).await?;
        let read_recursive = folder.read_recursive().await?;
        Ok(Docs {
            scope: self,
            doc_stream: read_recursive,
        })
    }
}

impl From<Folder> for Scope {
    fn from(folder: Folder) -> Self {
        Scope {
            folder_path: folder.workspace().path().join(folder.path()),
            tags: HashSet::new(),
        }
    }
}

impl From<&Path> for Scope {
    fn from(path: &Path) -> Self {
        Scope {
            folder_path: path.to_path_buf(),
            tags: HashSet::new(),
        }
    }
}

#[derive(Debug)]
pub struct Docs<'a> {
    scope: &'a Scope,
    doc_stream: ReadRecursive,
}

impl<'a> Docs<'a> {
    pub async fn next_doc(&mut self) -> Result<Option<Document>, AccessLibraryError> {
        while let Some(doc) = self.doc_stream.next_entry().await? {
            if self.scope.contains(&doc) {
                return Ok(Some(doc));
            }
        }
        Ok(None)
    }

    pub async fn into_vec(mut self) -> Result<Vec<Document>, AccessLibraryError> {
        let mut docs = Vec::new();
        while let Some(doc) = self.next_doc().await? {
            docs.push(doc);
        }
        Ok(docs)
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


#[cfg(test)]
mod tests {
    use super::*;
    use crate::library::fs_test_utils::{TempTree, fs_tree};

    #[tokio::test]
    async fn docs() {
        let dir = TempTree::new(fs_tree! {
            "doc.md" => "foo",
            "child" => {
                "nested.md" => "nested",
            },
        }).await.unwrap();

        let ws = Workspace::open(&dir).await.unwrap();
        let scope = Scope::from(ws.root());
        let mut docs = scope.docs(&ws).await.unwrap().into_vec().await.unwrap();
        assert_eq!(docs.len(), 2);

        // Order of entries is not guaranteed, so we need to sort them
        docs.sort_by_key(|doc| doc.path().to_owned());

        assert_eq!(docs[0].path(), "child/nested.md");
        assert_eq!(docs[0].text().await.unwrap().export(&mut None).as_deref(), Some("nested"));
        assert_eq!(docs[1].path(), "doc.md");
        assert_eq!(docs[1].text().await.unwrap().export(&mut None).as_deref(), Some("foo"));
    }
}