use crate::{dependencies::Resolve, extension::{Extension, InitExtensionError}};

pub struct Session {
    extensions: Vec<Box<dyn Extension>>,
}

impl Session {
    pub fn new() -> Self {
        Self { extensions: vec![] }
    }

    pub async fn add_extension<E: Extension + 'static>(&mut self, extension: E) -> Result<(), InitExtensionError> {
        // extension.initialize().await;
        self.extensions.push(Box::new(extension));
        Ok(())
    }

    pub async fn resolve_extension<E: Extension + Resolve + 'static>(&mut self) -> Result<(), InitExtensionError> {
        let extension = E::first(self)?;
        self.add_extension(extension).await
    }

    pub fn extensions(&self) -> &[Box<dyn Extension>] {
        &self.extensions
    }
}

