use std::sync::Arc;

use crate::{chat::{chat::ChatApi, prompter::Prompter, ChatError}, chunking::Chunker, convert::{ConversionError, Converter}, embedding::{Embedder, EmbeddingError}, extension::{ActiveExtension, Extension, F11y, UseExtensionError}, storage::{self, Content, Document, Folder, Scope}};
use mime::Mime;
use thiserror::Error;
use tokio::{io::AsyncRead, sync::mpsc::{error::SendError, UnboundedReceiver, UnboundedSender}, task::JoinHandle};
use tracing::instrument;

pub mod search;
mod chat;

pub use chat::{chat, simple_rag};

/// A unit of work that can be executed asynchronously.
/// 
/// A job combines an asynchronous function with documents and extensions. The function is given
/// access to these assets and can use them to perform some work.
pub struct Job<T, F: AsyncFnOnce(&mut Assets) -> Result<T, RunJobError> + Send> {
    callback: F,
    documents: Vec<Document>,
    scopes: Vec<Scope>,
    asset_channel: Option<(AssetSender, UnboundedReceiver<AssetItem>)>,
    extensions: Arc<Vec<ActiveExtension>>,
}

impl<T, F: AsyncFnOnce(&mut Assets) -> Result<T, RunJobError> + Send> Job<T, F> {
    /// Create a new job with the given callback function.
    pub fn new(callback: F) -> Self {
        Self {
            callback,
            documents: Vec::new(),
            scopes: Vec::new(),
            asset_channel: None,
            extensions: Arc::new(Vec::new()),
        }
    }

    pub fn and_then<T2, C: AsyncFnOnce(&mut Assets, T) -> Result<T2, RunJobError> + Send>(self, callback: C) -> Job<T2, impl AsyncFnOnce(&mut Assets) -> Result<T2, RunJobError> + Send> {
        let callback0 = self.callback;
        Job {
            callback: async move |assets| {
                let result0 = callback0(assets).await?;
                callback(assets, result0).await
            },
            documents: self.documents,
            scopes: self.scopes,
            asset_channel: self.asset_channel,
            extensions: self.extensions,
        }
    }

    pub fn and_chain<T2, F2: AsyncFnOnce(&mut Assets) -> Result<T2, RunJobError> + Send, C: FnOnce(T) -> Job<T2, F2> + Send>(self, callback: C) -> Job<T2, impl AsyncFnOnce(&mut Assets) -> Result<T2, RunJobError> + Send> {
        let callback0 = self.callback;
        Job {
            callback: async move |assets| {
                let result0 = callback0(assets).await?;
                let mut next_job = callback(result0)
                    // apply same extensions to the next job
                    .with_extensions(assets.extensions.iter().cloned());

                // Add assets to chained job
                for doc in assets.documents.drain(..) {
                    next_job.add_document(doc);
                }
                next_job.run().await
            },
            documents: self.documents,
            scopes: self.scopes,
            asset_channel: self.asset_channel,
            extensions: self.extensions,
        }
    }

    pub fn and_chain_async<T2, F2: AsyncFnOnce(&mut Assets) -> Result<T2, RunJobError> + Send, C: AsyncFnOnce(T) -> Job<T2, F2> + Send>(self, callback: C) -> Job<T2, impl AsyncFnOnce(&mut Assets) -> Result<T2, RunJobError> + Send> {
        let callback0 = self.callback;
        Job {
            callback: async move |assets| {
                let result0 = callback0(assets).await?;
                let mut next_job = callback(result0).await
                    // apply same extensions to the next job
                    .with_extensions(assets.extensions.iter().cloned());

                // Add assets to chained job
                for doc in assets.documents.drain(..) {
                    next_job.add_document(doc);
                }
                next_job.run().await
            },
            documents: self.documents,
            scopes: self.scopes,
            asset_channel: self.asset_channel,
            extensions: self.extensions,
        }
    }

    /// Configure the extensions available to the job.
    /// 
    /// Note that any `AssetSender`s created before this method is called will be invalidated.
    pub fn with_extensions<I: Iterator<Item = ActiveExtension>>(self, extensions: I) -> Self {
        Job {
            callback: self.callback,
            documents: self.documents,
            scopes: self.scopes,
            asset_channel: None,
            extensions: Arc::new(extensions.collect()),
        }
    }

    /// Add a document to the job's assets.
    pub fn add_document(&mut self, document: Document) -> &mut Self {
        self.documents.push(document);
        self
    }

    /// Add all documents in a folder to the job's assets.
    pub async fn add_folder(&mut self, folder: Folder) -> Result<&mut Self, storage::Error> {
        for doc in folder.list_documents().await? {
            self.add_document(doc);
        }
        for folder in folder.list_folders().await? {
            Box::pin(self.add_folder(folder)).await?;
        }
        Ok(self)
    }

    /// Get the assets available to the job.
    pub fn documents(&self) -> &[Document] {
        &self.documents
    }

    /// Get the scopes available to the job.
    pub fn scopes(&self) -> &[Scope] {
        &self.scopes
    }

    /// Get an asset sender for this job.
    /// 
    /// The asset sender can be used to send documents, folders, and extensions to the job's 
    /// assets. In this way, it is possible to add assets to the job after calling `Job::run` 
    /// (which consumes the job).
    /// 
    /// Note that any assets sent after the job has started will not be available to the callback
    /// function until it calls `Assets::refresh`.
    pub fn asset_sender(&mut self) -> AssetSender {
        if let Some(sender) = &self.asset_channel {
            return sender.0.clone();
        }
        let (sender, receiver) = tokio::sync::mpsc::unbounded_channel::<AssetItem>();
        
        let sender = AssetSender {
            inner: sender,
        };
        self.asset_channel = Some((sender.clone(), receiver));
        sender
    }

    /// Run the job.
    /// 
    /// This method will execute the callback function with the job's assets.
    /// 
    /// Returns the result of the callback function that was used to create the job.
    pub async fn run(mut self) -> Result<T, RunJobError> {
        let mut assets = Assets {
            documents: self.documents,
            scopes: self.scopes,
            extensions: self.extensions,
            asset_channel: self.asset_channel.take(),
        };

        // Call the callback function with the assets
        (self.callback)(&mut assets).await
    }
}

/// A collection of assets that can be used by a job.
pub struct Assets {
    documents: Vec<Document>,
    scopes: Vec<Scope>,
    extensions: Arc<Vec<ActiveExtension>>,
    asset_channel: Option<(AssetSender, UnboundedReceiver<AssetItem>)>,
}

impl Assets {
    /// Refresh the available assets, ensuring that any newly sent assets are included.
    /// 
    /// This method checks if any new documents or scopes have been sent to the job since it 
    /// started running or since the last refresh. If so, it adds them to the job's assets.
    /// 
    /// Returns an iterator over the newly added assets.
    #[instrument(skip(self))]
    pub fn refresh(&mut self) -> Refresh<'_> {
        let doc_idx = self.documents.len();
        let scope_idx = self.scopes.len();

        // Check if there are any new assets to add
        if let Some((_, receiver)) = &mut self.asset_channel {
            while let Ok(item) = receiver.try_recv() {
                match item {
                    AssetItem::Document(document) => self.documents.push(document),
                    AssetItem::Scope(scope) => self.scopes.push(scope),
                }
            }
        }

        tracing::debug!("Added {} documents", self.documents.len() - doc_idx);
        tracing::debug!("Added {} scopes", self.scopes.len() - scope_idx);

        Refresh {
            assets: self,
            doc_idx,
            scope_idx,
        }
    }

    /// Get the documents available to the job.
    pub fn documents(&self) -> &[Document] {
        &self.documents
    }

    /// Get the folders available to the job.
    pub fn scopes(&self) -> &[Scope] {
        &self.scopes
    }

    /// Get the extensions available to the job.
    pub fn extensions(&self) -> &Vec<ActiveExtension> {
        &self.extensions
    }

    /// Get an asset sender for this job.
    /// 
    /// The asset sender can be used to send documents, folders, and extensions to the job's 
    /// assets. In this way, it is possible to add assets from within the job's callback function.
    /// 
    /// Note that any assets sent will not be available until `Assets::refresh` is called.
    pub fn asset_sender(&mut self) -> AssetSender {
        if let Some(sender) = &self.asset_channel {
            return sender.0.clone();
        }
        let (sender, receiver) = tokio::sync::mpsc::unbounded_channel::<AssetItem>();
        
        let sender = AssetSender {
            inner: sender,
        };
        self.asset_channel = Some((sender.clone(), receiver));
        sender
    }

    /// Convert a document using the available extensions.
    /// 
    /// This method will try to convert the input content to the specified output type using the
    /// available extensions. If no extension is able to perform the conversion, an error will be 
    /// returned.
    pub async fn convert(&self, input: Content, output_type: Mime) -> Result<Vec<Box<dyn AsyncRead + Unpin>>, ConversionError> {
        tracing::debug!("Converting content to {}", output_type);
        let converters = self.extensions.iter()
            .filter_map(|ext| 
                if let Some(converter) = ext.converters().nth(0) {
                    Some(converter)
                } else {
                    None
                }
            )
            .collect::<Vec<_>>();

        tracing::debug!("Found {} converters", converters.len());
        for c in converters {
            match c.convert(input.clone(), output_type.clone()).await {
                Ok(result) => return Ok(result),
                Err(e) => match e {
                    ConversionError::UnsupportedMimeType(_) => continue,
                    _ => return Err(e),
                }
            }
        }

        Err(ConversionError::UnsupportedMimeType(output_type))
    }

    pub async fn chat_model(&self, model: Option<String>) -> Result<F11y<dyn ChatApi>, ChatError> {
        tracing::debug!("Getting chat model");
        // Iterate through extensions and find the specified model
        for ext in self.extensions.iter() {
            tracing::debug!("Checking extension {}", ext.name());
            if let Some(chat_client) = ext.chat_providers().nth(0) {
                tracing::debug!("Found chat model in extension {}", ext.name());
                if let Some(requested_model) = &model {
                    tracing::debug!("Looking for model {}", requested_model);
                    // TODO reconsider error variant
                    for model in chat_client.list_models().await.map_err(|e| ChatError::Provider(Box::new(e)))? {
                        if *model.id == *requested_model {
                            tracing::debug!("Found model {}", requested_model);
                            return Ok(chat_client);
                        }
                    }
                } else {
                    tracing::debug!("No model specified, returning default model");
                    return Ok(chat_client);
                }
            }
        }
        // TODO reconsider error variant
        Err(ChatError::Provider("No chat model found".into()))
    }

    pub fn embedders(&self) -> Vec<F11y<dyn Embedder>> {
        tracing::debug!("Getting embedders");
        let mut embedders = Vec::new();
        for ext in self.extensions.iter() {
            if let Some(embedder) = ext.embedders().nth(0) {
                embedders.push(embedder);
            }
        }
        embedders
    }

    pub fn chunkers(&self) -> Vec<F11y<dyn Chunker>> {
        tracing::debug!("Getting chunkers");
        let mut chunkers = Vec::new();
        for ext in self.extensions.iter() {
            if let Some(chunker) = ext.chunkers().nth(0) {
                chunkers.push(chunker);
            }
        }
        chunkers
    }

    /// Get the available prompters from the extensions.
    #[instrument(skip(self))]
    pub fn prompters(&self) -> Vec<F11y<dyn Prompter>> {
        tracing::debug!("Getting prompters");
        let mut prompters = Vec::new();
        for ext in self.extensions.iter() {
            let len_before = prompters.len();
            prompters.extend(ext.prompters());
            let len_after = prompters.len();
            tracing::debug!("Found {} prompters in extension {}", len_after - len_before, ext.name());
        }
        prompters
    }
}

/// An iterator over the assets that have been newly added to a job.
/// 
/// This struct is returned by the `Assets::refresh` method.
pub struct Refresh<'a> {
    assets: &'a Assets,
    doc_idx: usize,
    scope_idx: usize,
}

impl<'a> Refresh<'a> {
    /// Get the documents that have been newly added to the job.
    pub fn documents(self) -> impl Iterator<Item = Document> {
        self.filter_map(|item| {
            if let AssetItem::Document(doc) = item {
                Some(doc)
            } else {
                None
            }
        })
    }

    /// Get the scopes that have been newly added to the job.
    pub fn scopes(self) -> impl Iterator<Item = Scope> {
        self.filter_map(|item| {
            if let AssetItem::Scope(scope) = item {
                Some(scope)
            } else {
                None
            }
        })
    }
}

impl<'a> Iterator for Refresh<'a> {
    type Item = AssetItem;

    /// Get the next asset.
    fn next(&mut self) -> Option<Self::Item> {
        if self.doc_idx < self.assets.documents.len() {
            let item = AssetItem::Document(self.assets.documents[self.doc_idx].clone());
            self.doc_idx += 1;
            Some(item)
        } else if self.scope_idx < self.assets.scopes.len() {
            let item = AssetItem::Scope(self.assets.scopes[self.scope_idx].clone());
            self.scope_idx += 1;
            Some(item)
        } else {
            None
        }
    }
}


/// A sender for assets that can be used to send documents, folders, and extensions to a job.
#[derive(Debug, Clone)]
pub struct AssetSender {
    inner: UnboundedSender<AssetItem>,
}

impl AssetSender {
    /// Send a document to the job.
    /// 
    /// The document will be added to the assets of the job when the job is run or when the job's
    /// callback function calls `Assets::refresh`.
    pub fn send_document(&self, document: Document) -> Result<(), SendError<Document>> {
        self.inner.send(AssetItem::Document(document)).map_err(|e| match e.0 {
            AssetItem::Document(document) => SendError(document),
            _ => unreachable!(),
        })
    }

    /// Send a scope to the job.
    /// 
    /// The scope will be added to the assets of the job when the job is run or when the job's
    /// callback function calls `Assets::refresh`.
    /// 
    /// A scope is a set of documents (e.g., a folder and its subfolders).
    pub fn send_scope(&self, scope: Scope) -> Result<(), SendError<Scope>> {
        self.inner.send(AssetItem::Scope(scope)).map_err(|e| match e.0 {
            AssetItem::Scope(folder) => SendError(folder),
            _ => unreachable!(),
        })
    }
}

/// An item that can be sent to a job's assets.
pub enum AssetItem {
    Document(Document),
    Scope(Scope),
}

#[derive(Debug, Error)]
pub enum RunJobError {
    #[error("Job failed due to extension error: {0}")]
    Extension(#[from] UseExtensionError),

    #[error("Job failed due to chat error: {0}")]
    Chat(#[from] ChatError),

    #[error("Job failed due to embedding error: {0}")]
    Embedding(#[from] EmbeddingError),

    #[error("Job failed due to conversion error: {0}")]
    Conversion(#[from] ConversionError),

    #[error("Job failed due to prompt error: {0}")]
    Prompt(#[from] crate::chat::prompter::PromptError),

    #[error("Job failed due to storage error: {0}")]
    Storage(#[from] storage::Error),

    #[error("Job failed: {0}")]
    Other(Box<dyn std::error::Error + Send + Sync>),
}

#[cfg(test)]
mod tests {
    use async_trait::async_trait;
    
    use super::*;
    use crate::chat::{ChatModel, Message};
    use std::sync::{atomic::{AtomicUsize, Ordering}, Arc};

    struct TestChatModel {
        idx: AtomicUsize,
    }

    impl TestChatModel {
        fn new() -> Self {
            Self { idx: AtomicUsize::new(0) }
        }
    }

    #[async_trait]
    impl ChatModel for TestChatModel {
        async fn generate(&self, _messages: &Vec<Message>) -> Result<String, crate::chat::ChatError> {
            let idx = self.idx.fetch_add(1, Ordering::SeqCst);
            Ok(format!("Test Chat Model {}", idx))
        }

        async fn chat(
            &self,
            messages: &[Message],
            model:Option<&str>,
            config:Option<std::collections::HashMap<String,serde_json::Value>>,
        ) -> Result<crate::chat::Completion, ChatError> {
            let idx = self.idx.fetch_add(1, Ordering::SeqCst);
            Ok(crate::chat::Completion {
                message: Message::assistant(format!("Test Chat Model {}", idx)),
                usage: None,
            })
        }
    }

    struct TestExtension {
        model: Arc<TestChatModel>,
    }

    impl TestExtension {
        fn new() -> Self {
            Self { model: Arc::from(TestChatModel::new()) }
        }
    }

    // impl Extension for TestExtension {
    //     fn uri(&self) -> &str {
    //         "test"
    //     }
    //     fn name(&self) -> &str {
    //         "Test Extension"
    //     }
    //     fn description(&self) -> &str {
    //         "Test Extension"
    //     }
    //     fn chat_model(&self) -> Option<Arc<dyn ChatModel>> {
    //         Some(self.model.clone())
    //     }
    // }

    // #[tokio::test]
    // #[traced_test]
    // async fn test_job_run() {
    //     let mut job = Job::new(async |assets: &mut Assets| {
    //         let model = assets.extensions().first().unwrap().chat_model().unwrap();
    //         let messages = vec![Message::user("Hello")];
    //         let response = model.generate(&messages).await?;
    //         Ok(response)
    //         // Ok(())
    //     });
    //     let extension = Arc::new(TestExtension::new());
    //     job.add_extension(extension);
    //     let result = job.run().await.unwrap();
    //     assert_eq!(result, "Test Chat Model 0");
    // }

    // #[tokio::test]
    // #[traced_test]
    // async fn test_job_asset_sender() {
    //     let extension = Arc::new(TestExtension::new());

    //     // Create a new job depending on an extension
    //     let mut job = Job::new(async |assets: &mut Assets| {
    //         let model = assets.extensions().first().unwrap().chat_model().unwrap();
    //         let messages = vec![Message::user("Hello")];
    //         let response = model.generate(&messages).await?;
    //         Ok(response)
    //     });
    //     // Send the extension to the job's assets *before* running the job
    //     let asset_sender = job.asset_sender();
    //     asset_sender.send_extension(extension.clone()).unwrap();
    //     let result = job.run().await.unwrap();
    //     assert_eq!(result, "Test Chat Model 0");

    //     // Create a new job depending on an extension with delay
    //     let mut job = Job::new(async |assets: &mut Assets| {
    //         // wait for the extension to be sent
    //         tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
    //         // Refresh the assets to include any newly sent extensions
    //         assets.refresh();
    //         // Now we can use the extension
    //         let model = assets.extensions().first().unwrap().chat_model().unwrap();
    //         let messages = vec![Message::user("Hello")];
    //         let response = model.generate(&messages).await?;
    //         Ok(response)
    //     });
    //     // Start job in the background
    //     let asset_sender = job.asset_sender();
    //     let job_handle = tokio::spawn(async move {
    //         job.run().await.unwrap()
    //     });
    //     tokio::time::sleep(tokio::time::Duration::from_millis(1)).await;
    //     // Send the extension to the job's assets *after* starting the job
    //     asset_sender.send_extension(extension).unwrap();
    //     // Wait for the job to finish
    //     let result = job_handle.await.unwrap();
    //     assert_eq!(result, "Test Chat Model 1");
    // }


}
