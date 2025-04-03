use std::sync::Arc;

use crate::{chat::ChatError, extension::{Extension, UseExtensionError}, storage::{Document, Folder}};
use thiserror::Error;
use tokio::sync::mpsc::{error::SendError, UnboundedReceiver, UnboundedSender};


/// A unit of work that can be executed asynchronously.
/// 
/// A job combines an asynchronous function with documents and extensions. The function is given
/// access to these assets and can use them to perform some work.
pub struct Job<T, F: AsyncFnOnce(&mut Assets) -> Result<T, RunJobError> + Send> {
    callback: F,
    assets: Assets,
    asset_sender: Option<AssetSender>,
}

impl<T, F: AsyncFnOnce(&mut Assets) -> Result<T, RunJobError> + Send> Job<T, F> {
    /// Create a new job with the given callback function.
    pub fn new(callback: F) -> Self {
        Self {
            callback,
            assets: Assets {
                documents: Vec::new(),
                folders: Vec::new(),
                extensions: Vec::new(),
                receiver: None,
            },
            asset_sender: None,
        }
    }

    /// Add a document to the job's assets.
    pub fn add_document(&mut self, document: Document) -> &mut Self {
        self.assets.documents.push(document);
        self
    }

    /// Add a folder to the job's assets.
    pub fn add_folder(&mut self, folder: Folder) -> &mut Self {
        self.assets.folders.push(folder);
        self
    }

    /// Add an extension to the job's assets.
    pub fn add_extension(&mut self, extension: Arc<dyn Extension>) -> &mut Self {
        self.assets.extensions.push(extension);
        self
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
        if let Some(sender) = &self.asset_sender {
            return sender.clone();
        }
        let (sender, receiver) = tokio::sync::mpsc::unbounded_channel::<AssetItem>();
        
        self.assets.receiver = Some(receiver);
        let sender = AssetSender {
            inner: sender,
        };
        self.asset_sender = Some(sender.clone());
        sender
    }

    /// Run the job.
    /// 
    /// This method will execute the callback function with the job's assets.
    /// 
    /// Returns the result of the callback function that was used to create the job.
    pub async fn run(mut self) -> Result<T, RunJobError> {
        // Refresh the assets before running the callback
        self.assets.refresh();

        // Call the callback function with the assets
        (self.callback)(&mut self.assets).await
    }
}

/// A collection of assets that can be used by a job.
pub struct Assets {
    documents: Vec<Document>,
    folders: Vec<Folder>,
    extensions: Vec<Arc<dyn Extension>>,
    receiver: Option<UnboundedReceiver<AssetItem>>,
}

impl Assets {
    /// Refresh the available assets, ensuring that any newly sent assets are included.
    pub fn refresh(&mut self) {
        if let Some(receiver) = &mut self.receiver {
            while let Ok(item) = receiver.try_recv() {
                match item {
                    AssetItem::Document(document) => self.documents.push(document),
                    AssetItem::Folder(folder) => self.folders.push(folder),
                    AssetItem::Extension(extension) => self.extensions.push(extension),
                }
            }
        }
    }

    /// Get the documents available to the job.
    pub fn documents(&self) -> &[Document] {
        &self.documents
    }

    /// Get the folders available to the job.
    pub fn folders(&self) -> &[Folder] {
        &self.folders
    }

    /// Get the extensions available to the job.
    pub fn extensions(&self) -> &Vec<Arc<dyn Extension>> {
        &self.extensions
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

    /// Send a folder to the job.
    /// 
    /// The folder will be added to the assets of the job when the job is run or when the job's
    /// callback function calls `Assets::refresh`.
    pub fn send_folder(&self, folder: Folder) -> Result<(), SendError<Folder>> {
        self.inner.send(AssetItem::Folder(folder)).map_err(|e| match e.0 {
            AssetItem::Folder(folder) => SendError(folder),
            _ => unreachable!(),
        })
    }

    /// Send an extension to the job.
    /// 
    /// The extension will be added to the assets of the job when the job is run or when the job's
    /// callback function calls `Assets::refresh`.
    pub fn send_extension(&self, extension: Arc<dyn Extension>) -> Result<(), SendError<Arc<dyn Extension>>> {
        self.inner.send(AssetItem::Extension(extension)).map_err(|e| match e.0 {
            AssetItem::Extension(extension) => SendError(extension),
            _ => unreachable!(),
        })
    }
}

/// An item that can be sent to a job's assets.
enum AssetItem {
    Document(Document),
    Folder(Folder),
    Extension(Arc<dyn Extension>),
}

#[derive(Debug, Error)]
pub enum RunJobError {
    #[error("Job failed due to extension error: {0}")]
    Extension(#[from] UseExtensionError),

    #[error("Job failed due to document error: {0}")]
    Chat(#[from] ChatError),

    #[error("Job failed: {0}")]
    Other(Box<dyn std::error::Error + Send + Sync>),
}

#[cfg(test)]
mod tests {
    use async_trait::async_trait;

    use super::*;
    use crate::{chat::{ChatModel, Message}, extension::{Extension, Functionality}};
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct TestChatModel {
        idx: AtomicUsize,
    }

    impl TestChatModel {
        fn new() -> Self {
            Self { idx: AtomicUsize::new(0) }
        }
    }

    impl Functionality for TestChatModel {
        fn extension_uri(&self) -> &str { "test" }
        fn id(&self) -> &str { "test" }
        fn name(&self) -> &str { "Test Chat Model" }
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

    impl Extension for TestExtension {
        fn uri(&self) -> &str {
            "test"
        }
        fn name(&self) -> &str {
            "Test Extension"
        }
        fn description(&self) -> &str {
            "Test Extension"
        }
        fn chat_model(&self) -> Option<Arc<dyn ChatModel>> {
            Some(self.model.clone())
        }
    }

    #[tokio::test]
    async fn test_job_run() {
        let mut job = Job::new(async |assets: &mut Assets| {
            let model = assets.extensions().first().unwrap().chat_model().unwrap();
            let messages = vec![Message::user("Hello")];
            let response = model.generate(&messages).await?;
            Ok(response)
            // Ok(())
        });
        let extension = Arc::new(TestExtension::new());
        job.add_extension(extension);
        let result = job.run().await.unwrap();
        assert_eq!(result, "Test Chat Model 0");
    }

    #[tokio::test]
    async fn test_job_asset_sender() {
        let extension = Arc::new(TestExtension::new());

        // Create a new job depending on an extension
        let mut job = Job::new(async |assets: &mut Assets| {
            let model = assets.extensions().first().unwrap().chat_model().unwrap();
            let messages = vec![Message::user("Hello")];
            let response = model.generate(&messages).await?;
            Ok(response)
        });
        // Send the extension to the job's assets *before* running the job
        let asset_sender = job.asset_sender();
        asset_sender.send_extension(extension.clone()).unwrap();
        let result = job.run().await.unwrap();
        assert_eq!(result, "Test Chat Model 0");

        // Create a new job depending on an extension with delay
        let mut job = Job::new(async |assets: &mut Assets| {
            // wait for the extension to be sent
            tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
            // Refresh the assets to include any newly sent extensions
            assets.refresh();
            // Now we can use the extension
            let model = assets.extensions().first().unwrap().chat_model().unwrap();
            let messages = vec![Message::user("Hello")];
            let response = model.generate(&messages).await?;
            Ok(response)
        });
        // Start job in the background
        let asset_sender = job.asset_sender();
        let job_handle = tokio::spawn(async move {
            job.run().await.unwrap()
        });
        tokio::time::sleep(tokio::time::Duration::from_millis(1)).await;
        // Send the extension to the job's assets *after* starting the job
        asset_sender.send_extension(extension).unwrap();
        // Wait for the job to finish
        let result = job_handle.await.unwrap();
        assert_eq!(result, "Test Chat Model 1");
    }


}