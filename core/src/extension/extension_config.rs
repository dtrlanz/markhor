use crate::permissions::{self, Permission};


#[derive(Debug, Clone)]
pub struct ExtensionConfig {
    pub permissions: Vec<Permission>,
}

impl Default for ExtensionConfig {
    fn default() -> Self {
        Self {
            permissions: permissions::all_permissions(),
        }
    }
}
