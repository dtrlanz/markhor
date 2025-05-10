use tokio::sync::mpsc::Sender;

use crate::{chat::{chat::{ContentPart, Message}, prompter::PromptError}, extension::UseExtensionError};

use super::{search::{search_job, SearchResults}, Assets, Job, RunJobError};


pub fn chat<F: FnMut(&Message) + Send>(mut messages: Vec<Message>, mut on_message: F) -> Job<Vec<Message>, impl AsyncFnOnce(&mut Assets) -> Result<Vec<Message>, RunJobError> + Send> {
    Job::new(async move |assets| {
        let chat_model = assets.chat_model(None).await?;
        let prompter = assets.prompters().into_iter().nth(0)
            .ok_or(UseExtensionError::PrompterNotAvailable)?;

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
                        Ok(input) => Message::user(input),
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

        chat(messages, on_message)
    })
}