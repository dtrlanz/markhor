use std::{
    collections::HashMap,
    io::{self, Write},
    sync::Mutex,
};

use async_trait::async_trait;
use markhor_core::chat::{
    ChatUi, Msg, MsgEvent, ReasoningEvent, ToolCallEvent, WorkerEvent, common::ContentPart,
};

/// Plain-text terminal UI that prints every event it receives.
#[derive(Default)]
pub struct ConsoleUi {
    /// Messages that have begun but not yet been completed, keyed by message id.
    open: Mutex<HashMap<usize, OpenMsg>>,
}

struct OpenMsg {
    sender: String,
    header_printed: bool,
    streamed: bool,
}

impl ConsoleUi {
    /// Prints the `[id] sender:` header the first time any output for a message appears.
    fn ensure_header(&self, id: usize, open: &mut HashMap<usize, OpenMsg>) {
        if let Some(msg) = open.get_mut(&id).filter(|m| !m.header_printed) {
            msg.header_printed = true;
            println!();
            print!("[{id}] {}: ", msg.sender);
            flush();
        }
    }
}

#[async_trait]
impl ChatUi for ConsoleUi {
    async fn prompt_with_commands(&self, _commands: Vec<String>) -> String {
        print!("you> ");
        flush();
        tokio::task::spawn_blocking(|| {
            let mut line = String::new();
            match io::stdin().read_line(&mut line) {
                Ok(0) | Err(_) => "/exit".to_string(),
                Ok(_) => line.trim().to_string(),
            }
        })
        .await
        .unwrap_or_else(|_| "/exit".to_string())
    }

    async fn select(&self, options: &Vec<String>) -> Option<usize> {
        for (i, option) in options.iter().enumerate() {
            println!("  {}) {option}", i + 1);
        }
        print!("select> ");
        flush();
        let count = options.len();
        tokio::task::spawn_blocking(move || {
            let mut line = String::new();
            io::stdin().read_line(&mut line).ok()?;
            let n: usize = line.trim().parse().ok()?;
            (1..=count).contains(&n).then(|| n - 1)
        })
        .await
        .ok()
        .flatten()
    }

    fn handle_message(&self, event: &MsgEvent) {
        let mut open = self.open.lock().unwrap();
        match event {
            MsgEvent::Begin(id, sender) => {
                open.insert(
                    **id,
                    OpenMsg {
                        sender: sender.to_string(),
                        header_printed: false,
                        streamed: false,
                    },
                );
            }
            MsgEvent::Part(id, part) => {
                self.ensure_header(**id, &mut open);
                if let Some(msg) = open.get_mut(&**id) {
                    msg.streamed = true;
                }
                print!("{}", text_of(std::slice::from_ref(part)));
                flush();
            }
            MsgEvent::Full(id, msg) => {
                let id = **id;
                let streamed = open.get(&id).is_some_and(|m| m.streamed);
                match msg {
                    // The user's own text is already on screen from the prompt.
                    Msg::User { .. } => {}
                    Msg::Platform(text) => {
                        self.ensure_header(id, &mut open);
                        if !streamed {
                            print!("{text}");
                        }
                        println!();
                    }
                    Msg::Agent { content, .. } => {
                        self.ensure_header(id, &mut open);
                        if !streamed {
                            print!("{}", text_of(content));
                        }
                        println!();
                    }
                    other => {
                        self.ensure_header(id, &mut open);
                        println!("{other:?}");
                    }
                }
                open.remove(&id);
            }
            MsgEvent::Log(id, text) => {
                println!("  (msg {} log) {text}", **id);
            }
        }
    }

    fn handle_reasoning(&self, event: &ReasoningEvent) {
        match event {
            ReasoningEvent::Begin(id) => println!("  <thinking msg={}>", **id),
            ReasoningEvent::Part(_, text) => {
                print!("{text}");
                flush();
            }
            ReasoningEvent::Summary(_, title, prose) => println!("\n  > {title}: {prose}"),
            ReasoningEvent::Full(_, _) => println!("\n  </thinking>"),
            ReasoningEvent::Log(id, text) => println!("  (thinking log {}) {text}", **id),
        }
    }

    fn handle_tool_call(&self, event: &ToolCallEvent) {
        match event {
            ToolCallEvent::Begin(msg, call, request) => println!(
                "  [tool {} via msg {}] {}({})",
                **call, **msg, request.name, request.arguments
            ),
            ToolCallEvent::Part(call, text) => {
                println!("  [tool {}] > {}", **call, text.trim_end())
            }
            ToolCallEvent::Full(call, result) => {
                println!("  [tool {}] => {}", **call, result.content)
            }
            ToolCallEvent::Log(call, text) => println!("  [tool {} log] {text}", **call),
        }
    }

    fn handle_worker(&self, event: &WorkerEvent) {
        match event {
            WorkerEvent::Begin(member) => println!("  [{member}] started"),
            WorkerEvent::Summary(member, title, prose) => {
                println!("  [{member}] {title}: {prose}")
            }
            WorkerEvent::Full(member, text) if text.is_empty() => {
                println!("  [{member}] done")
            }
            WorkerEvent::Full(member, text) => println!("  [{member}] done: {text}"),
        }
    }
}

fn text_of(parts: &[ContentPart]) -> String {
    parts
        .iter()
        .map(|part| match part {
            ContentPart::Text(text) => text.clone(),
            ContentPart::Image { mime_type, .. } => format!("[image: {mime_type}]"),
        })
        .collect()
}

fn flush() {
    let _ = io::stdout().flush();
}
