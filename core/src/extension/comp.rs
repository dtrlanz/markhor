use crate::{chat::{chat::ChatModel, prompter::Prompter}, chunking::Chunker, convert::Converter, dependencies::{Resolve, ResolveDependencyError, Session}, embedding::EmbeddingModel, extension::Extension, tool::Tool};

use std::{any::TypeId, fmt::Display, ops::{Deref, DerefMut}, sync::Arc};
use serde::{Deserialize, Serialize};


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
            x if x == const { TypeId::of::<dyn ChatModel>() } => ComponentType::ChatModel,
            x if x == const { TypeId::of::<dyn EmbeddingModel>() } => ComponentType::EmbeddingModel,
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

macro_rules! impl_resolve_comp {
    ($($trait:ident => $method:ident),*  $(,)? ) => {
        $(
            impl Resolve for Comp<dyn $trait> {
                type Item = Self;

                fn iter(session: &Session) -> Result<impl Iterator<Item = Self>, ResolveDependencyError> {
                    let extensions = session.extensions();
                    let comps = extensions.iter()
                        .flat_map(|ext| ext.$method().into_iter().map(|comp| Self::new(Arc::clone(ext), comp)));
                    Ok(comps)
                }

                fn iter_from_items<I>(items: I) -> Result<impl Iterator<Item = Self>, ResolveDependencyError>
                where
                    I: Iterator<Item = Self::Item>
                {
                    Ok(items)
                }
            }
        )*
    };
}

impl_resolve_comp! {
    ChatModel => chat_models,
    EmbeddingModel => embedding_models,
    Chunker => chunkers,
    Converter => converters,
    Prompter => prompters,
    Tool => tools,
}

#[derive(Debug, Clone, Copy, Hash, PartialEq, Eq, Serialize, Deserialize)]
pub enum ComponentType {
    ChatModel,
    EmbeddingModel,
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
    use crate::{chunking::test_chunker::FixedSizeChunkerExtension, embedding::test_utils::MockEmbedderExtension};

    #[test]
    fn comp() {
        let extension = FixedSizeChunkerExtension::new(10);
        let chunker = extension.chunkers().into_iter().next().unwrap();

        let comp = Comp::new(
            Arc::new(extension),
            chunker,
        );
        
        assert_eq!(comp.component_type(), ComponentType::Chunker);
        assert_eq!(comp.extension().uri(), "markhor://chunker/fixed-size");
        assert_eq!(comp.metadata_id(), "markhorchunkerfixed-size chunker");
    }

    #[tokio::test]
    async fn resolve() {
        let mut session = Session::new();
        let chunker_ext_5 = FixedSizeChunkerExtension::new(5);
        let chunker_ext_10 = FixedSizeChunkerExtension::new(10);
        let embedder_ext = MockEmbedderExtension::new(vec!["the", "cat", "sat", "on", "mat"]);
        session.add_extension(chunker_ext_5).await.unwrap();
        session.add_extension(chunker_ext_10).await.unwrap();
        session.add_extension(embedder_ext).await.unwrap();

        // Test resolving single components
        let chunker: Comp<dyn Chunker> = Resolve::first(&session).unwrap();
        assert_eq!(chunker.component_type(), ComponentType::Chunker);
        assert_eq!(chunker.metadata_id(), "markhorchunkerfixed-size chunker");

        let embedder: Comp<dyn EmbeddingModel> = Resolve::first(&session).unwrap();
        assert_eq!(embedder.component_type(), ComponentType::EmbeddingModel);
        assert_eq!(embedder.metadata_id(), "markhorembeddermock embedding-model");

        // Test resolving multiple components of the same type
        let chunkers: Vec<Comp<dyn Chunker>> = Resolve::first(&session).unwrap();
        assert_eq!(chunkers.len(), 2);
        assert_eq!(chunkers[0].component_type(), ComponentType::Chunker);
        assert_eq!(chunkers[1].component_type(), ComponentType::Chunker);

        // Make sure we're getting two different chunkers
        let chunked = chunkers.iter()
            .map(|comp| comp.chunk("0123456789").unwrap())
            .collect::<Vec<_>>();
        assert_eq!(chunked[0].len(), 2); // 2 chunks of size 5
        assert_eq!(chunked[1].len(), 1); // 1 chunk of size 10
    }
}