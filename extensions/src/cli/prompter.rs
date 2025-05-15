use std::ops::Range;

use async_trait::async_trait;

use markhor_core::{chat::prompter::{PromptError, Prompter}, job::AssetSender, storage::Folder};
use nu_ansi_term::{Color, Style};
use reedline::{default_emacs_keybindings, ColumnarMenu, Completer, DefaultPrompt, DefaultPromptSegment, Emacs, Highlighter, KeyCode, KeyModifiers, MenuBuilder, Reedline, ReedlineEvent, ReedlineMenu, Signal, Span, StyledText, Suggestion};
use tokio::sync::Mutex;

pub struct ConsolePrompter {
    folder: Option<Folder>,
    asset_sender: Option<AssetSender>,
}

impl ConsolePrompter {
    pub fn new(folder: Option<Folder>) -> Self {
        Self { 
            folder,
            asset_sender: None,
        }
    }

    pub fn with_attach_callback(self, callback: Box<dyn Fn(&[&str]) + Send + Sync>) -> Self {
        Self {
            folder: self.folder,
            asset_sender: None,
        }
    }
    

    pub fn isolate_document_names(input: &str) -> Vec<Range<usize>> {
        let mut result = vec![];
        
        let mut next_slash = input.find('/');
        while let Some(start) = next_slash {
            // Check if name is enclosed in quotation marks
            if input[start..].chars().nth(1) == Some('"') {
                // If next char is quotation mark, look for closing quotation mark
                if let Some(end) = input[start + 2..].find('"').map(|i| i + start + 2) {
                    result.push(start + 2..end);
                    next_slash = input[end..].find('/').map(|i| i + end);
                } else {
                    return result;
                }
            } else {
                // If not, look for the next space or end of string
                let end = input[start..].find(' ').map(|i| i + start)
                    .unwrap_or(input.len());
                if end > start + 1 {
                    result.push(start + 1..end);
                }
                next_slash = input[end..].find('/').map(|i| i + end);
            }
        }
        result
    }

    pub fn isolate_document_names_with_prefix_suffix(input: &str) -> Vec<(Range<usize>, Range<usize>, Range<usize>)> {
        let mut result = vec![];
        
        let mut next_slash = input.find('/');
        while let Some(start) = next_slash {
            // Check if name is enclosed in quotation marks
            if input[start..].chars().nth(1) == Some('"') {
                // If next char is quotation mark, look for closing quotation mark
                if let Some(end) = input[start + 2..].find('"').map(|i| i + start + 2) {
                    result.push((start..start + 2, start + 2..end, end..end + 1));
                    next_slash = input[end..].find('/').map(|i| i + end);
                } else {
                    return result;
                }
            } else {
                // If not, look for the next space or end of string
                let end = input[start..].find(' ').map(|i| i + start)
                    .unwrap_or(input.len());
                if end > start + 1 {
                    result.push((start..start + 1, start + 1..end, end..end));
                }
                next_slash = input[end..].find('/').map(|i| i + end);
            }
        }
        result
    }
}

#[async_trait]
impl Prompter for ConsolePrompter {
    async fn prompt(&self, message: &str) -> Result<String, PromptError> {

        // ** Auto-complete the document names **

        // Create a list of document names from the folder
        let doc_names = if let Some(folder) = self.folder.as_ref() {
            folder.list_documents().await.unwrap_or_default()
                .iter().map(|doc| doc.name().to_lowercase())
                .collect::<Vec<_>>()
        } else {
            vec![]
        };

        // Set up completer
        let completer = Box::new(DocNameCompleter { doc_names });

        // Use the interactive menu to select options from the completer
        let completion_menu = Box::new(ColumnarMenu::default().with_name("completion_menu"));
        
        // Set up keybindings
        // TAB to select completion
        let mut keybindings = default_emacs_keybindings();
        keybindings.add_binding(
            KeyModifiers::NONE,
            KeyCode::Tab,
            ReedlineEvent::UntilFound(vec![
                ReedlineEvent::Menu("completion_menu".to_string()),
                ReedlineEvent::MenuNext,
            ]),
        );
        // ESC to cancel
        keybindings.add_binding(
            KeyModifiers::NONE,
            KeyCode::Esc, 
            ReedlineEvent::CtrlC,
        );

        let edit_mode = Box::new(Emacs::new(keybindings));

        let mut line_editor = Reedline::create()
            .with_completer(completer)
            .with_menu(ReedlineMenu::EngineCompleter(completion_menu))
            .with_edit_mode(edit_mode)
            .with_highlighter(Box::new(DocNameHighlighter));

        let prompt = DefaultPrompt {
            left_prompt: DefaultPromptSegment::Basic(message.to_string()),
            right_prompt: DefaultPromptSegment::Empty,
        };
        
        let mut result = tokio::task::spawn_blocking(move || {
            match line_editor.read_line(&prompt) {
                Ok(Signal::Success(line)) => Ok(line),
                Ok(Signal::CtrlD) => Err(PromptError::Canceled),
                Ok(Signal::CtrlC) => Err(PromptError::Canceled),
                Err(err) => Err(PromptError::Io(err)),
            }
        }).await?;

        if let (
                Ok(input), 
                Some(folder), 
                Some(sender)
            ) = (
                result.as_mut(), 
                self.folder.as_ref(), 
                self.asset_sender.as_ref()
            ) {
            let mut prefix_suffix = vec![];
            for (prefix,range, suffix) in  ConsolePrompter::isolate_document_names_with_prefix_suffix(&input) {
                prefix_suffix.push((prefix, suffix));
                let file_name = &input[range];
                match folder.document_by_name(file_name).await {
                    Ok(doc) => {
                        // Send document to job via the asset sender
                        sender.send_document(doc).unwrap_or_else(|e| {
                            tracing::warn!("Could not attach document: {} ({})", file_name, e);
                        });
                    }
                    Err(e) => {
                        tracing::warn!("Could not attach document: {} ({})", file_name, e);
                    }
                }
            }
            // Replace our slash-based convention with something that more clearly communicates
            // intent to reference a document
            //   /my_doc   -> [[my_doc]]
            //   /"my_doc" -> [[my_doc]]
            while let Some((prefix, suffix)) = prefix_suffix.pop() {
                input.replace_range(suffix, "]]");
                input.replace_range(prefix, "[[");
            }
        }
        result
    }

    fn set_asset_sender(&mut self, sender: Option<AssetSender>) -> Result<(), PromptError> {
        self.asset_sender = sender;
        Ok(())
    }
}


struct DocNameCompleter {
    doc_names: Vec<String>,
}

impl Completer for DocNameCompleter {
    fn complete(&mut self, line: &str, pos: usize) -> Vec<Suggestion> {
        // Find last slash before the cursor position
        if let Some(last_slash) = line[..pos].rfind('/') {
            // Get the substring after the last slash
            let prefix = &line[last_slash + 1..pos].to_lowercase();

            // Filter the document names based on the prefix
            let suggestions = self.doc_names.iter()
                .filter(|name| name.starts_with(prefix))
                .filter_map(|name| {
                    // Check if name contains whitespace
                    if name.contains(' ') {
                        // Check if name contains quotation mark
                        if name.contains('"') {
                            // Nothing we can do here
                            return None;
                        } else {
                            // Escape the name with quotation marks
                            return Some(format!("\"{}\"", name));
                        }
                    } else {
                        // No need to escape
                        return Some(name.clone());
                    }
                })
                .map(|name| {
                    Suggestion {
                        value: name,
                        description: None,
                        style: None,
                        extra: None,
                        span: Span {
                            start: last_slash + 1,
                            end: pos,
                        },
                        append_whitespace: true,
                    }
                })
                .collect();

            suggestions
        } else {
            return vec![];
        }
    }
}

struct DocNameHighlighter;

impl Highlighter for DocNameHighlighter {
    fn highlight(&self, line: &str, cursor: usize) -> reedline::StyledText {
        let ranges = ConsolePrompter::isolate_document_names(line);
        let mut buffer = vec![];
        let mut last_end = 0;
        for mut range in ranges {
            range.start = range.start.max(last_end);
            let (open, close) = if line[range.start - 1..range.start].chars().last() == Some('/') {
                (range.start - 1..range.start, range.end..range.end)
            } else {
                (range.start - 2..range.start, range.end..range.end + 1)
            };

            buffer.push((
                Style {
                    ..Default::default()
                },
                line[last_end..open.start].to_string()
            ));
            buffer.push((
                Style {
                    foreground: Some(Color::Cyan),
                    ..Default::default()
                },
                line[open].to_string()
            ));
            buffer.push((
                Style {
                    foreground: Some(Color::Cyan),
                    ..Default::default()
                },
                line[range.start..range.end].to_string()
            ));
            buffer.push((
                Style {
                    foreground: Some(Color::Cyan),
                    ..Default::default()
                },
                line[close.clone()].to_string()
            ));
            last_end = close.end;
        }
        buffer.push((
            Style {
                ..Default::default()
            },
            line[last_end..].to_string()
        ));
        StyledText { buffer }
    }
}