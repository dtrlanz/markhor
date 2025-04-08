use std::{any, path::{Path, PathBuf}, pin::Pin, sync::Arc};

use async_once_cell::Lazy;
use markhor_core::{extension::Extension, job::{Job, RunJobError}, storage::{Content, Document, Folder, Storage, Workspace}};
use tokio::io::{AsyncRead, AsyncReadExt, BufReader};
use tracing::error;

mod chat;
pub use chat::chat;

pub struct Markhor {
    pub storage: Arc<Storage>,
    pub workspace: anyhow::Result<Arc<Workspace>>,
    pub folder: Option<Folder>,
    pub extensions: Vec<Arc<dyn Extension>>,
}

impl Markhor {
    pub async fn import(&self, file: &Path) -> Result<Document, anyhow::Error> {
        let folder = if let Some(folder) = &self.folder {
            folder
        } else {
            anyhow::bail!("Cannot import document without a target folder");
        };

        // Create document in the target folder
        let doc = folder.create_document(file.file_stem().unwrap().to_str().unwrap()).await?;

        let original_extension = match file.extension().and_then(|s| s.to_str()) {
            Some(ext) => ext,
            None => {
                anyhow::bail!("Files without extension are not supported");
            }
        };

        // Add original file to the document
        let mut original = BufReader::new(tokio::fs::File::open(file).await?);
        doc.add_file(original_extension, &mut original).await?;

        // Convert the file to markdown using the extensions
        let input = Content::File(file.to_path_buf());
        let output_type = "text/markdown".parse().unwrap();
        let mut job: Job<Vec<Box<dyn AsyncRead + Unpin>>, _> = Job::new(async |assets| {
            let output = assets.convert(input, output_type).await?;
            Ok(output)
        });
        for ext in &self.extensions {
            job.add_extension(ext.clone());
        }

        let result = job.run().await;
        match result {
            Ok(vec) => {
                for mut reader in vec {
                    doc.add_file("md", &mut reader).await?;
                }
            }
            Err(e) => {
                error!("Error during conversion: {:?}", e);
            }
        }

        Ok(doc)
    }

    pub fn use_extension(&mut self, extension: Arc<dyn Extension>) -> &mut Self {
        self.extensions.push(extension);
        self
    }

    pub async fn chat(&self, documents: Vec<PathBuf>) -> Result<(), anyhow::Error> {
        let mut job  = Job::new(&chat);

        for ext in &self.extensions {
            job.add_extension(ext.clone());
        }

        let ws = match &self.workspace {
            Ok(ws) => ws,
            Err(e) => {
                anyhow::bail!("Error getting workspace: {}", e);
            }
        };

        for path in documents {
            let doc = ws.document(&*path).await?;
            job.add_document(doc);
        }
        
        job.run().await?;

        Ok(())
    }
}
