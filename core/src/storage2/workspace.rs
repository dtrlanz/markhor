use std::path::PathBuf;

#[derive(Debug)]
pub struct Workspace {
    pub(crate) absolute_path: PathBuf,
}
