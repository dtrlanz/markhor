use markhor_core::{chat::prompter::Prompter, extension::Extension, storage::Folder};

pub mod prompter;


pub struct CliExtension {
    folder: Option<Folder>,
}

impl CliExtension {
    pub fn new(folder: Option<Folder>) -> Self {
        Self { folder }
    }
}

impl Default for CliExtension {
    fn default() -> Self {
        Self { folder: None }
    }
}

impl Extension for CliExtension {
    fn name(&self) -> &str {
        "cli"
    }

    fn description(&self) -> &str {
        "Command line interface extension"
    }

    fn uri(&self) -> &str {
        "markhor://cli"
    }

    fn prompters(&self) -> Vec<Box<dyn Prompter>> {
        vec![Box::new(prompter::ConsolePrompter::new(self.folder.clone()))]
    }
}