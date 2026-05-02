use crate::extension::{Extension, InitExtensionError};

mod require;

pub use require::{Require, MeetRequirementError};


pub struct Assets {
    extensions: Vec<Box<dyn Extension>>,
}

impl Assets {
    pub fn new() -> Self {
        Self { extensions: vec![] }
    }

    pub async fn add_extension<E: Extension + 'static>(&mut self, extension: E) -> Result<(), InitExtensionError> {
        // extension.initialize().await;
        self.extensions.push(Box::new(extension));
        Ok(())
    }

    pub async fn require_extension<E: Extension + Require + 'static>(&mut self) -> Result<(), InitExtensionError> {
        let extension = E::require(self)?;
        self.add_extension(extension).await
    }

    pub fn extensions(&self) -> &[Box<dyn Extension>] {
        &self.extensions
    }
}

