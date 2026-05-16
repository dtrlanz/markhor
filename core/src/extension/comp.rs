use serde::{Deserialize, Serialize};

use crate::{chat::{chat::ChatApi, prompter::Prompter}, chunking::Chunker, convert::Converter, embedding::Embedder, extension::Extension, tool::Tool};

use std::{any::TypeId, fmt::Display, ops::{Deref, DerefMut}, sync::Arc};


/// A boxed extension component along with metadata.
/// 
/// Extension components are the trait objects returned by the `Extension` trait methods.
/// 
// Name: This is the successor to `F11y`. The name is less obviously temporary than that was but
// not necessarily final yet.
pub struct Comp<T: ?Sized> {
    extension: Arc<dyn Extension>,
    component: Box<T>,
}

impl<T: ?Sized> Deref for Comp<T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        &self.component
    }
}

impl<T: ?Sized> DerefMut for Comp<T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.component
    }
}

impl<T: ?Sized + 'static> Comp<T> {
    pub fn new(extension: Arc<dyn Extension>, component: Box<T>) -> Self {
        Self { extension, component }
    }

    pub fn extension(&self) -> &Arc<dyn Extension> {
        &self.extension
    }

    pub fn component_type(&self) -> ComponentType {
        let t_id = TypeId::of::<T>();
        match TypeId::of::<T>() {
            x if x == const { TypeId::of::<dyn ChatApi>() } => ComponentType::ChatModel,
            x if x == const { TypeId::of::<dyn Embedder>() } => ComponentType::EmbeddingModel,
            x if x == const { TypeId::of::<dyn Chunker>() } => ComponentType::Chunker,
            x if x == const { TypeId::of::<dyn Converter>() } => ComponentType::Converter,
            x if x == const { TypeId::of::<dyn Prompter>() } => ComponentType::Prompter,
            x if x == const { TypeId::of::<dyn Tool>() } => ComponentType::Tool,

            // `Comp` is never constructed with anything other than extension components. If we
            // end up here, we've made a mistake somewhere in the code that constructs `Comp`s,
            // or we need to update the above matches to include a new component type.
            _ => unreachable!("Non-existent component type: {:?}", t_id),
        }
    }

    pub fn metadata_id(&self) -> String {
        let suffix = match self.component_type() {
            _ => "",
        };
        format!("{} {}{}", 
            sanitize_filename::sanitize(self.extension.uri()),
            self.component_type(), 
            suffix)
    }
}

#[derive(Debug, Clone, Copy, Hash, PartialEq, Eq, Serialize, Deserialize)]
pub enum ComponentType {
    ChatModel,      // anticipating pending change in name & definition
    EmbeddingModel, // "
    Chunker,
    Converter,
    Prompter,
    Tool,
}

impl Display for ComponentType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            ComponentType::ChatModel => "chat-model",
            ComponentType::EmbeddingModel => "embedding-model",
            ComponentType::Chunker => "chunker",
            ComponentType::Converter => "converter",
            ComponentType::Prompter => "prompter",
            ComponentType::Tool => "tool",
        };
        write!(f, "{}", s)
    }
}


#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn comp() {
        let extension = crate::chunking::test_chunker::FixedSizeChunkerExtension::new(100);
        let chunker = extension.chunker().unwrap();

        let comp = Comp::new(
            Arc::new(extension),
            chunker,
        );
        
        assert_eq!(comp.component_type(), ComponentType::Chunker);
        assert_eq!(comp.extension().uri(), "markhor://chunker/fixed-size");
        assert_eq!(comp.metadata_id(), "markhorchunkerfixed-size chunker");
    }
}