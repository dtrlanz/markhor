use std::{path::{Path, PathBuf}, sync::Arc};

use markhor_core::{chat::chat::Message, extension::{ActiveExtension, Extension}, job::{self, Job}, storage::{Content, Document, Folder, Storage, Workspace}};
use tokio::io::{AsyncRead, BufReader};
use tracing::error;
use console::Term;
use textwrap::wrap;

use crate::cli::ChatArgs;


pub struct Markhor {
    pub storage: Arc<Storage>,
    pub workspace: anyhow::Result<Arc<Workspace>>,
    pub folder: Option<Folder>,
    pub extensions: Vec<ActiveExtension>,
}

impl Markhor {
    /// Opens documents in the workspace.
    pub async fn open_documents(&self, docs: Vec<String>) -> Result<Vec<Document>, anyhow::Error> {
        let mut documents = Vec::new();
        let ws = self.workspace.as_ref()
            .map_err(|e| anyhow::anyhow!("Error getting workspace: {}", e))?;
        for doc in docs {
            let doc = ws.document(&Path::new(&doc)).await?;
            documents.push(doc);
        }
        Ok(documents)
    }


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
        let job: Job<Vec<Box<dyn AsyncRead + Unpin>>, _> = 
            Job::new(async |assets| {
                let output = assets.convert(input, output_type).await?;
                Ok(output)
            })
            .with_extensions(self.extensions.iter().cloned());

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

    pub async fn search(&self, query: &str, limit: usize, paths: Vec<PathBuf>) -> Result<(), anyhow::Error> {
        let mut job = job::search::search_job(query, limit)
            .with_extensions(self.extensions.iter().cloned());

        let ws = self.workspace.as_ref()
            .map_err(|e| anyhow::anyhow!("Error getting workspace: {}", e))?;

        if paths.is_empty() {
            // Add all documents in the workspace to the job
            job.add_folder(ws.root().await).await?;
        } else {
            // Add specific documents or folders to the job
            for path in paths {
                // Check if the path is a file or folder
                if path.is_file() {
                    // If it's a file, add it
                    let doc = ws.document(&*path).await?;
                    job.add_document(doc);
                } else if path.is_dir() {
                    // If it's a folder, add the documents in the folder
                    let folder = ws.folder(&*path).await?;
                    job.add_folder(folder).await?;
                } else {
                    println!("Path is neither a file nor a folder: {}", path.display());
                }
            }
        }

        println!("Searching for: {}", query);
        println!("Searching {} document(s)...", job.documents().len());

        // Run search
        let result = job.run().await?;

        // Print results
        for doc in result.documents() {
            println!("Document: {}", doc.document().path().display());
            for file in doc.files() {
                println!("  File: {}", file.file_name());
                for chunk in file.chunks().await? {
                    println!("    Chunk #{} (similarity {})", chunk.rank(), chunk.similarity());
                    println!("      Text: {}", chunk.chunk().text());
                }
            }
        }
        Ok(())
    }

    pub fn use_extension(&mut self, extension: impl Extension + 'static) -> &mut Self {
        self.extensions.push(ActiveExtension::new(extension, Default::default()));
        self
    }

    pub async fn chat(&self, args: ChatArgs) -> Result<(), anyhow::Error> {
        if args.model.is_some() {
            anyhow::bail!("Model override is not yet implemented");
        }
        if !args.plugins.is_empty() {
            anyhow::bail!("Plugins are not yet implemented");
        }
        let ChatArgs { prompt, docs, scope, .. } = args;

        println!("\nEnter empty string to exit\n");
        
        // Prep messages
        let mut messages = vec![Message::system("You are a helpful assistant.")];
        if let Some(prompt) = prompt {
            messages.push(Message::user(prompt.to_string()));
        }

        
        let mut job  = job::chat(
                messages, 
                print_assistant_message, 
                print_attachment_message
            ).with_extensions(self.extensions.iter().cloned());

        // Load document contents
        let docs = self.open_documents(docs).await?;
        for doc in docs {
            job.add_document(doc);
        }
        
        job.run().await?;

        Ok(())
    }
}

fn print_assistant_message(msg: &Message) {
    match msg {
        Message::Assistant {..} => {
            let term_width = Term::stdout().size().1 as usize;
            println!();
            for line in wrap(&msg.text_content(), term_width) {
                println!("{}", line);
            }
        }
        _ => (),
    }
}

fn print_attachment_message(documents: &[Document]) {
    for doc in documents {
        println!(" 📄{}", doc.path().display());
    }
    println!();
}