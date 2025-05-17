use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::{chat::{chat::ChatApi, prompter::Prompter}, chunking::Chunker, convert::Converter, embedding::Embedder};

use super::{Extension, F11y, FunctionalityType};


#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ExtensionConfig {

}

#[derive(Clone)]
pub struct ActiveExtension {
    inner: Arc<(Box<dyn Extension>, ExtensionConfig)>,
}

impl ActiveExtension {
    pub fn new(extension: impl Extension + 'static, config: ExtensionConfig) -> Self {
        Self {
            inner: Arc::new((Box::new(extension), config)),
        }
    }

    fn extension(&self) -> &dyn Extension {
        &*self.inner.0
    }

    fn config(&self) -> &ExtensionConfig {
        &self.inner.1
    }

    pub fn uri(&self) -> &str {
        self.extension().uri()
    }

    pub fn name(&self) -> &str {
        self.extension().name()
    }

    pub fn description(&self) -> &str {
        self.extension().description()
    }

    pub fn chat_providers(&self) -> impl Iterator<Item = F11y<dyn ChatApi>> {
        self.extension().chat_model().into_iter().map(|model| F11y {
            trait_object: model,
            functionality_type: FunctionalityType::ChatProvider,
            extension: self.clone(),
        })
    }

    pub fn embedders(&self) -> impl Iterator<Item = F11y<dyn Embedder>> {
        self.extension().embedding_model().into_iter().map(|model| F11y {
            trait_object: model,
            functionality_type: FunctionalityType::Embedder,
            extension: self.clone(),
        })
    }

    pub fn chunkers(&self) -> impl Iterator<Item = F11y<dyn Chunker>> {
        self.extension().chunker().into_iter().map(|model| F11y {
            trait_object: model,
            functionality_type: FunctionalityType::Chunker,
            extension: self.clone(),
        })
    }

    pub fn converters(&self) -> impl Iterator<Item = F11y<dyn Converter>> {
        self.extension().converter().into_iter().map(|model| F11y {
            trait_object: model,
            functionality_type: FunctionalityType::Converter,
            extension: self.clone(),
        })
    }

    pub fn prompters(&self) -> impl Iterator<Item = F11y<dyn Prompter>> {
        self.extension().prompters().into_iter().map(|model| F11y {
            trait_object: model,
            functionality_type: FunctionalityType::Prompter,
            extension: self.clone(),
        })
    }
}

