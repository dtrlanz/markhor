use tokio::sync::mpsc::Sender;

use crate::{chat::{chat::{ContentPart, Message}, prompter::PromptError}, extension::UseExtensionError, storage::{self, Document}};

use super::{search::{search_job, SearchResults}, Assets, Job, RunJobError};


pub fn chat<F1: FnMut(&Message) + Send, F2: FnMut(&[Document]) + Send>(mut messages: Vec<Message>, mut on_message: F1, mut on_attachment: F2) -> Job<Vec<Message>, impl AsyncFnOnce(&mut Assets) -> Result<Vec<Message>, RunJobError> + Send> {
    Job::new(async move |assets| {
        let chat_model = assets.chat_model(None).await?;
        let mut prompter = assets.prompters().into_iter().nth(0)
            .ok_or(UseExtensionError::PrompterNotAvailable)?;

        prompter.set_asset_sender(Some(assets.asset_sender())).ok();

        if !assets.documents.is_empty() {
            messages.push(attach_docs("", &assets.documents, &mut on_attachment).await?);
            messages.push(Message::assistant("I have reviewed the documents. How can I assist?"));
        }

        loop {
            let next_message = match messages.last() {
                Some(Message::User(..)) => {
                    // Generate AI response
                    let response = chat_model.generate(&*messages, &Default::default()).await.map_err(|e| RunJobError::Other(Box::new(e)))?;
                    let string = response.content.into_iter()
                        .map(|part| match part {
                            ContentPart::Text(s) => s,
                            ContentPart::Image { .. } => String::from("[image]"),
                        })
                        .collect::<String>();
                    Message::assistant(string)
                }
                Some(Message::Tool(..)) => {
                    tracing::error!("Tool use not implemented yet.");
                    return Err(RunJobError::Extension(UseExtensionError::ToolNotAvailable));
                }
                _ => {
                    // Get user input
                    match prompter.prompt("").await {
                        Ok(input) => {
                            let new_docs = assets.refresh().documents().collect::<Vec<_>>();
                            if new_docs.is_empty() {
                                Message::user(input)
                            } else {
                                attach_docs(&input, &new_docs, &mut on_attachment).await?
                            }
                        },
                        Err(PromptError::Canceled) => {
                            return Ok(messages);
                        }
                        Err(e) => {
                            return Err(RunJobError::Prompt(e));
                        }
                    }
                }
            };

            on_message(&next_message);
            messages.push(next_message.clone());
        }
    })
}



async fn attach_docs<F: FnMut(&[Document]) + Send>(user_message: &str, docs: &[Document], on_attachment: &mut F) -> Result<Message, storage::Error> {
    let mut doc_contents = vec![];
    for doc in docs {
        let mut content = vec![];
        let files = doc.primary_content_files().await?;
        if files.is_empty() {
            tracing::warn!("No text content files found for document: {}", doc.path().display());
            continue;
        }
        for file in doc.primary_content_files().await? {
            content.push(file.read_string().await.unwrap());
        }
        doc_contents.push(
            format!(
                "<document name=\"{}\">\n{}\n</document>", 
                doc.path().with_extension("").display(),
                content.join("\n\n"),
            )
        );
    }

    on_attachment(&docs);

    let msg = Message::user(format!(
        "<automated_message type=\"file_attachment\">The user has attached the following document(s). If the reason is obvious, respond directly. If not, only confirm very briefly and await further instructions.</automated_message>\n{}\n\n{}", 
        doc_contents.join("\n\n"),
        user_message,
    ));
    Ok(msg)
}

pub fn simple_rag<F: FnMut(&Message) + Send>(prompt: &str, limit: usize, mut on_message: F) -> Job<Vec<Message>, impl AsyncFnOnce(&mut Assets) -> Result<Vec<Message>, RunJobError> + Send> {
    let search_job = search_job(prompt, limit);

    search_job.and_chain_async(async |results| {
        let mut result_string = String::new();
        for doc in results.documents() {
            for file in doc.files() {
                let file_results = file.chunks().await
                    .unwrap()   // TODO fix error handling, e.g. via `Job::error()`
                    .map(|chunk| chunk.chunk().text().to_string())
                    .collect::<Vec<_>>()
                    .join(" ...\n\n");
                result_string.push_str(&format!("File: {}\n\n", file.file_name()));
                result_string.push_str(&file_results);
                result_string.push_str("\n---\n")
            }
        }

        let mut messages = vec![
            Message::system(
                "You are an assistant helping with document management and knowledge work.
                
                The user has requested help reviewing one or more documents. The relevant chunks 
                will be inserted below. After reviewing them, answer questions or complete tasks
                as requested by the user."
            ),
            Message::user(result_string),
            Message::assistant("I have reviewed the content and am ready to help."),
            Message::user(prompt.to_string()),
        ];

        chat(messages, on_message, |_| ())
    })
}