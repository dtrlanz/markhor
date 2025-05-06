use std::sync::Arc;

use serde::{Deserialize, Serialize};

use super::Extension;

pub struct ActiveExtension {
    extension: Arc<dyn Extension>,
    config: ExtensionConfig,
}

impl ActiveExtension {
    pub fn new(extension: impl Extension + 'static, config: ExtensionConfig) -> Self {
        Self {
            extension: Arc::new(extension),
            config,
        }
    }

    pub fn extension(&self) -> &Arc<dyn Extension> {
        &self.extension
    }

    pub fn config(&self) -> &ExtensionConfig {
        &self.config
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ExtensionConfig {

}