use std::path::PathBuf;
use markhor_core::chat::ChatError;
use thiserror::Error;

#[derive(Error, Debug)]
pub enum PluginError {
    #[error("Python interpreter '{0:?}' not found or is not executable")]
    PythonNotFound(String),

    #[error("Plugin directory '{0:?}' not found")]
    PluginDirNotFound(PathBuf),

    #[error("Plugin script '{0:?}' not found in '{1:?}'")]
    PluginScriptNotFound(String, PathBuf),

    #[error("Requirements.txt not found in plugin directory '{0:?}'")]
    RequirementsNotFound(PathBuf),

    #[error("Failed to access virtual environment path: {0}")]
    VenvPathError(PathBuf),

    #[error("Failed to create virtual environment at '{0:?}': {1}")]
    VenvCreationError(PathBuf, std::io::Error),

    #[error("Failed to install dependencies from '{0:?}': {1}")]
    DependencyInstallError(PathBuf, String), // String captures stderr

    #[error("Failed to spawn plugin process '{0:?}': {1}")]
    ProcessSpawnError(String, std::io::Error),

    #[error("Failed to send data to plugin stdin: {0}")]
    StdinWriteError(std::io::Error),

    #[error("Failed to read data from plugin stdout: {0}")]
    StdoutReadError(std::io::Error),

    #[error("Failed to read data from plugin stderr: {0}")]
    StderrReadError(std::io::Error),

    #[error("Plugin process exited with non-zero status {0}. Stderr: {1}")]
    ProcessFailed(std::process::ExitStatus, String),

    #[error("Failed to deserialize JSON response from plugin: {0}. Response text: {1}")]
    ResponseDeserializationError(serde_json::Error, String),

    #[error("Response from plugin is invalid: {0}")]
    ResponseInvalid(String), // For cases where the response format is not as expected

    #[error("Failed to serialize JSON request for plugin: {0}")]
    RequestSerializationError(serde_json::Error),

    #[error("Plugin reported an error: {0}")]
    PluginReportedError(String),

    #[error("Required environment variable '{0}' is not set")]
    MissingEnvironmentVariable(String),

    #[error("Plugin initialization failed: {0}")]
    InitializationError(String), // Generic init error

    #[error(transparent)]
    IoError(#[from] std::io::Error), // Catch-all for other IO errors
}

impl Into<ChatError> for PluginError {
    fn into(self) -> ChatError {
        ChatError::PluginError(Box::new(self))
    }
}