use std::{path::PathBuf, sync::Arc};

use crate::storage2::Folder;

#[derive(Debug, Clone)]
pub struct Workspace {
    inner: Arc<WorkspaceInner>,
}

#[derive(Debug)]
struct WorkspaceInner {
    absolute_path: PathBuf,
}

impl Workspace {
    /// Returns the root path of the workspace.
    pub fn path(&self) -> &PathBuf {
        &self.inner.absolute_path
    }

    /// Returns the root folder of the workspace.
    pub async fn root(&self) -> Folder {
        Folder::new(self.inner.absolute_path.clone(), self.clone())
    }

}

