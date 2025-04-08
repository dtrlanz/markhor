use markhor_core::{chat::chat::{ContentPart, Message}, extension::Extension, job::{Assets, Job, RunJobError}, storage::{Content, Document, Folder, Storage, Workspace}};


pub async fn chat(assets: &mut Assets) -> Result<(), RunJobError> {

    let docs = assets.documents();

    let mut messages = if docs.is_empty() {
        vec![Message::system("You are a helpful assistant.")]
    }  else { 
        let mut vec = vec![Message::system(
            "You are an assistant helping with document management and knowledge work.
            
            The user has requested help reviewing one or more documents. The documents will
            be inserted below. After reviewing these documents, please answer questions about
            them or complete tasks as requested by the user."
        )];
        for doc in docs {
            for file in doc.files().await.map_err(|e| RunJobError::Other(Box::new(e)))? {
                if file.extension() == Some("md") {
                    let content = file.read_string().await.map_err(|e| RunJobError::Other(Box::new(e)))?;
                    vec.push(Message::user(format!("[{}]\n\n{}", file.file_name().unwrap(), content)));
                    vec.push(Message::assistant("I have reviewed the document and am ready to help."));
                }
            }
        }
        vec
    };

    let chat_model = assets.chat_model(None).await?;

    let mut input = prompt().await.unwrap();
    
    while (input != "exit") {
        messages.push(Message::user(input.clone()));
        let response = chat_model.generate(&messages, &Default::default()).await.map_err(|e| RunJobError::Other(Box::new(e)))?;
        
        println!();
        for part in response.content {
            match &part {
                ContentPart::Text(s) => {
                    print!("{}", s);
                }
                ContentPart::Image { .. } => {
                    print!("[image]");
                }
            }
            tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;
        }

        input = prompt().await.unwrap();
    }

    Ok(())
}

use dialoguer::{Confirm, Input, Password, Select, theme::ColorfulTheme};
use anyhow::Context; // If used within functions returning anyhow::Result

// Example Usage (wrap in spawn_blocking in async contexts):
async fn prompt() -> anyhow::Result<String> {
    let result = tokio::task::spawn_blocking(|| {
        let i: Result<String, _> = Input::with_theme(&ColorfulTheme::default())
            .with_prompt("Enter prompt ('exit' to cancel)")
            .interact_text()
            .context("Failed to read name"); // Use anyhow context
        return i;
    }).await;

    let input = result.context("Blocking task failed (panic)")??; // Double '?'
    Ok(input)
}
