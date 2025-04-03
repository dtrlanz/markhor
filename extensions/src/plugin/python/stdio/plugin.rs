use std::{collections::HashMap, path::PathBuf, sync::Arc};
use markhor_core::{chat::DynChatModel, extension::Extension};
use super::{chat::PythonStdioChatModel, wrapper::StdioWrapper};

pub struct PythonStdioPlugin {
    uri: String,
    description: String,
    stdio_wrapper: Arc<StdioWrapper>,
    chat_models: Vec<Box<DynChatModel<'static>>>,
}

impl PythonStdioPlugin {
    pub fn new(
        uri: String,
        name: String,
        description: String,
        directory: PathBuf,
        script_name: String,
        python_executable: Option<String>, // Allow overriding default python
        env_vars: HashMap<String, String>,
    ) -> Self {
        let stdio_wrapper = Arc::new(
            StdioWrapper::new(name, directory, script_name, python_executable, env_vars)
        );
        let chat_models = vec![
            DynChatModel::boxed(PythonStdioChatModel::new(&stdio_wrapper))
        ];
        Self {
            uri,
            description,
            stdio_wrapper,
            chat_models
        }
    }
}

impl Extension for PythonStdioPlugin {
    fn uri(&self) -> &str {
        &self.uri
    }
    fn name(&self) -> &str {
        &self.stdio_wrapper.plugin_name
    }
    fn description(&self) -> &str {
        &self.description
    }
    fn chat_model(&self) -> Option<&markhor_core::chat::DynChatModel> {
        self.chat_models.first().map(Box::as_ref)
    }
}
