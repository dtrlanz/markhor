use std::{fs, path::{Path, PathBuf}, sync::{Arc, atomic::{AtomicBool, Ordering}}};

use markhor_core::{chat::chat::Message, extension::{ActiveExtension, Extension}, job::{self, Job}, storage2::{Document, Folder, Part, Workspace}};
use markhor_extensions::cli::CliExtension;
use tokio::io::{AsyncRead, AsyncReadExt, BufReader};
use tracing::error;
use console::Term;
use textwrap::wrap;

use crate::cli::ChatArgs;


pub struct Markhor {
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

        // Copy document to target folder (if not already there)
        let folder_path = folder.workspace().path().join(folder.path());
        let file_path = tokio::fs::canonicalize(file).await?;
        let file_path = if file_path.starts_with(&folder_path) {
            file_path
        } else {
            let target_path = folder_path.join(file.file_name().unwrap());
            tokio::fs::copy(&file_path, &target_path).await?;
            target_path
        };

        let original_extension = match file.extension().and_then(|s| s.to_str()) {
            Some(ext) => ext,
            None => {
                anyhow::bail!("Files without extension are not supported");
            }
        };

        // Open document
        let mut doc = folder.document(file_path).await?;

        // Convert the file to markdown using the extensions
        let input = file.to_path_buf();
        let output_type = "text/markdown".parse().unwrap();
        let job: Job<Vec<Box<dyn AsyncRead + Unpin>>, _> = 
            Job::new(async |assets| {
                let output = assets.convert(input, output_type).await?;
                Ok(output)
            })
            .with_extensions(self.extensions.iter().cloned());

        let result = job.run().await;
        let mut part_idx = 0;
        match result {
            Ok(vec) => {
                for mut reader in vec {
                    // Read to string
                    let mut reader = BufReader::new(reader);
                    let mut contents = String::new();
                    reader.read_to_string(&mut contents).await?;

                    doc.text_mut().await.push_part(Part::new(format!("part-{}", part_idx), contents));
                    part_idx += 1;
                }
            }
            Err(e) => {
                error!("Error during conversion: {:?}", e);
            }
        }

        Ok(doc)
    }

    // pub async fn search(&self, query: &str, limit: usize, paths: Vec<PathBuf>) -> Result<(), anyhow::Error> {
    //     let mut job = job::search::search_job(query, limit)
    //         .with_extensions(self.extensions.iter().cloned());

    //     let ws = self.workspace.as_ref()
    //         .map_err(|e| anyhow::anyhow!("Error getting workspace: {}", e))?;

    //     if paths.is_empty() {
    //         // Add all documents in the workspace to the job
    //         job.add_folder(ws.root().await).await?;
    //     } else {
    //         // Add specific documents or folders to the job
    //         for path in paths {
    //             // Check if the path is a file or folder
    //             if path.is_file() {
    //                 // If it's a file, add it
    //                 let doc = ws.document(&*path).await?;
    //                 job.add_document(doc);
    //             } else if path.is_dir() {
    //                 // If it's a folder, add the documents in the folder
    //                 let folder = ws.folder(&*path).await?;
    //                 job.add_folder(folder).await?;
    //             } else {
    //                 println!("Path is neither a file nor a folder: {}", path.display());
    //             }
    //         }
    //     }

    //     println!("Searching for: {}", query);
    //     println!("Searching {} document(s)...", job.documents().len());

    //     // Run search
    //     let result = job.run().await?;

    //     // Print results
    //     for doc in result.documents() {
    //         println!("Document: {}", doc.document().path().display());
    //         for file in doc.files() {
    //             println!("  File: {}", file.file_name());
    //             for chunk in file.chunks().await? {
    //                 println!("    Chunk #{} (similarity {})", chunk.rank(), chunk.similarity());
    //                 println!("      Text: {}", chunk.chunk().text());
    //             }
    //         }
    //     }
    //     Ok(())
    // }

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

        println!();
        let printer = MessagePrinter::new();
        
        // Prep messages
        let mut messages = vec![Message::system("You are a helpful assistant.")];
        if let Some(prompt) = prompt {
            messages.push(Message::user(prompt.to_string()));
        }

        let mut job  = job::chat(
                messages, 
                |msg| printer.print_message(msg), 
                |docs| printer.print_attachment(docs),
            ).with_extensions(self.extensions.iter().cloned());

        // Load document contents
        let docs = self.open_documents(docs).await?;
        for doc in docs {
            job.add_document(doc);
        }
        
        let _messages = job.run().await?;

        Ok(())
    }
}

struct MessagePrinter;

impl MessagePrinter {
    fn new() -> Self {
        Self
    }

    fn print_message(&self, msg: &Message) {
        match msg {
            Message::Assistant {..} => {
                println!();
                let term_width = Term::stdout().size().1 as usize;
                for line in wrap(&msg.text_content(), term_width) {
                    println!("{}", line);
                }
            }
            Message::User(..) => {
                // We don't need to print user messages since they are already printed by the 
                // prompter.
            }
            _ => (),
        }
    }

    fn print_attachment(&self, documents: &[Document]) {
        println!();
        for doc in documents {
            println!(" 📄{}", doc.path().display());
        }
    }
}