use markhor_core::{chat::prompter::Prompter, extension::Extension};

pub mod prompter;


pub struct CliExtension;

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
        vec![Box::new(prompter::ConsolePrompter)]
    }
}