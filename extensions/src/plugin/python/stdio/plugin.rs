use std::{collections::HashMap, path::PathBuf, sync::Arc};
use markhor_core::{chat::ChatModel, extension::Extension};
use super::{chat::PythonStdioChatModel, wrapper::StdioWrapper};

pub struct PythonStdioPlugin {
    description: String,
    stdio_wrapper: Arc<StdioWrapper>,
    chat_models: Vec<Arc<PythonStdioChatModel>>,
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
            StdioWrapper::new(uri, name, directory, script_name, python_executable, env_vars)
        );
        let chat_models = vec![
            Arc::new(PythonStdioChatModel::new(&stdio_wrapper))
        ];
        Self {
            description,
            stdio_wrapper,
            chat_models
        }
    }
}

impl Extension for PythonStdioPlugin {
    fn uri(&self) -> &str {
        &self.stdio_wrapper.plugin_uri
    }
    fn name(&self) -> &str {
        &self.stdio_wrapper.plugin_name
    }
    fn description(&self) -> &str {
        &self.description
    }
    // fn chat_model(&self) -> Option<Arc<dyn ChatModel>> {
    //     Some(self.chat_models.first().unwrap().clone())
    // }
}
