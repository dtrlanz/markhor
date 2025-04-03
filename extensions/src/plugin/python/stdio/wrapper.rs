use serde::{Deserialize, Serialize};
use super::error::PluginError;
use super::{PluginRequest, PluginResponse};
use async_once_cell::OnceCell; // Use OnceCell for async initialization
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::process::Command;
use tokio::sync::Mutex; // Mutex needed if initialization modifies shared state

/// Manages interaction with a Python plugin script via standard input/output.
///
/// ## Communication Protocol:
/// - **Stdin:** Sends JSON requests to the Python script (`PluginRequest`).
///            Format: `{"method": "method_name", "params": {...}}`
/// - **Stdout:** Receives JSON responses from the Python script (`PluginResponse`).
///             Format (Success): `{"status": "success", "result": {...}}`
///             Format (Error):   `{"status": "error", "message": "..."}`
///             *** Stdout is the primary channel for structured result data. ***
/// - **Stderr:** Captures diagnostic logs (INFO, ERROR, tracebacks) from the Python
///             script's `logging` module.
///             *** Stderr is used for debugging/logging, NOT for the primary result. ***
///
/// ## Initialization:
/// Uses `OnceCell` for lazy initialization. The first call to a plugin method
/// triggers the `initialize` function, which ensures Python is found,
/// creates a virtual environment (`venv`) if missing, and installs dependencies
/// from `requirements.txt` using `pip`. Subsequent calls reuse the initialized state.
///
/// ## Error Handling:
/// - If the Python process exits with a non-zero status code, this usually indicates
///   an unhandled exception or a call to `sys.exit(1)` within the script.
/// - The implementation FIRST attempts to parse `stdout` even on failure, as the
///   Python script is expected to write a structured `{"status": "error", ...}`
///   JSON to `stdout` for logical errors (e.g., API errors).
/// - If `stdout` contains a valid error JSON, `PluginError::PluginReportedError` is returned.
/// - If `stdout` is empty, unparseable, or contains unexpected success JSON on failure,
///   the implementation falls back to returning `PluginError::ProcessFailed`, including
///   the exit status and the captured content of `stderr`.
pub struct StdioWrapper {
    pub(crate) plugin_name: String,     // stored here for logging
    plugin_dir: PathBuf,
    script_name: String, // e.g., "gemini_plugin.py"
    python_exec: String, // e.g., "python3" or configured path
    init_state: OnceCell<Result<InitializedState, PluginError>>,
    // Env vars to be provided to the python script
    env_vars: HashMap<String, String>,
    // Use Mutex for initialization coordination if multiple threads might trigger it
    // (Though OnceCell handles the "once" part, Mutex protects the init logic itself)
    init_mutex: Mutex<()>,
}

impl StdioWrapper {
    pub fn new(
        id: String,
        plugin_dir: PathBuf,
        script_name: String,
        python_executable: Option<String>, // Allow overriding default python
        env_vars: HashMap<String, String>,
    ) -> Self {
        StdioWrapper {
            plugin_name: id,
            plugin_dir,
            script_name,
            python_exec: python_executable.unwrap_or_else(|| "python3".to_string()),
            init_state: OnceCell::new(),
            env_vars,
            init_mutex: Mutex::new(()),
        }
    }

    // Lazy Initializer function - finds python, ensures venv, installs deps
    async fn initialize(&self) -> Result<InitializedState, PluginError> {
        // Acquire mutex to prevent concurrent initialization attempts
        let _guard = self.init_mutex.lock().await;

        // --- 1. Check Plugin Directory and Script ---
        if !self.plugin_dir.is_dir() {
            return Err(PluginError::PluginDirNotFound(self.plugin_dir.clone()));
        }
        let plugin_script_path = self.plugin_dir.join(&self.script_name);
        if !plugin_script_path.is_file() {
            return Err(PluginError::PluginScriptNotFound(
                self.script_name.clone(),
                self.plugin_dir.clone(),
            ));
        }
        let requirements_path = self.plugin_dir.join("requirements.txt");
        if !requirements_path.is_file() {
            // Optional: Allow plugins without requirements?
            return Err(PluginError::RequirementsNotFound(self.plugin_dir.clone()));
        }

        // --- 2. Ensure Python Interpreter ---
        // For simplicity, check if default_python_exec works. A real impl might search PATH.
        let python_status = Command::new(&self.python_exec)
            .arg("--version")
            .output()
            .await;
        let python_exec = match python_status {
            Ok(output) if output.status.success() => self.python_exec.clone(),
            _ => return Err(PluginError::PythonNotFound(self.python_exec.clone())),
        };
        tracing::info!("Using Python interpreter: {}", python_exec);


        // --- 3. Ensure Virtual Environment ---
        let venv_path = self.plugin_dir.join("venv");
        let venv_python_path = get_venv_python_path(&venv_path);

        if !venv_path.is_dir() || !venv_python_path.is_file() {
            tracing::info!("Virtual environment not found at {:?}. Creating...", venv_path);
            self.create_venv(&python_exec, &venv_path).await?;
        } else {
            tracing::info!("Virtual environment found at {:?}", venv_path);
            // Optional: Could add a check here to see if deps *might* need reinstalling
            // (e.g., if requirements.txt changed), but pip handles this fairly well.
        }

        // --- 4. Ensure Dependencies are Installed ---
        tracing::info!("Ensuring dependencies are installed from {:?}", requirements_path);
        self.install_dependencies(&venv_path, &requirements_path).await?;


        tracing::info!("Plugin '{}' initialized successfully.", self.plugin_name);
        Ok(InitializedState {
            venv_python_path,
            plugin_script_path,
        })
    }

    async fn create_venv(&self, python_exec: &str, venv_path: &Path) -> Result<(), PluginError> {
        let output = Command::new(python_exec)
            .arg("-m")
            .arg("venv")
            .arg(venv_path)
            .output()
            .await
            .map_err(|e| PluginError::VenvCreationError(venv_path.to_path_buf(), e))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr).to_string();
            tracing::error!("Failed to create venv. Stderr: {}", stderr);
            Err(PluginError::VenvCreationError(
                venv_path.to_path_buf(),
                // Create a generic IO error as cause isn't directly available
                 std::io::Error::new(std::io::ErrorKind::Other, stderr),
            ))
        } else {
            tracing::info!("Successfully created virtual environment at {:?}", venv_path);
            Ok(())
        }
    }

    async fn install_dependencies(
        &self,
        venv_path: &Path,
        requirements_path: &Path,
    ) -> Result<(), PluginError> {
        let pip_path = get_venv_pip_path(venv_path);
        if !pip_path.is_file() {
            // This shouldn't happen if venv creation succeeded, but check anyway
             return Err(PluginError::DependencyInstallError(
                 requirements_path.to_path_buf(),
                 format!("pip executable not found at {:?}", pip_path)
             ));
         }

        // Use --disable-pip-version-check for cleaner logs, -q for quieter install
        let output = Command::new(pip_path)
            .arg("install")
            .arg("--disable-pip-version-check")
            // .arg("-q") // Add -q for less verbose output, remove for debugging
            .arg("-r")
            .arg(requirements_path)
            .output()
            .await
            .map_err(|e| PluginError::DependencyInstallError(
                requirements_path.to_path_buf(), e.to_string()
            ))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr).to_string();
            tracing::error!(
                "Failed to install dependencies from {:?}. Stderr: {}",
                requirements_path, stderr
            );
            Err(PluginError::DependencyInstallError(
                requirements_path.to_path_buf(),
                stderr,
            ))
        } else {
            let stdout = String::from_utf8_lossy(&output.stdout);
            // Log stdout only if not quiet or if debugging
            if tracing::event_enabled!(tracing::Level::DEBUG) && !stdout.trim().is_empty() {
                tracing::debug!("pip install stdout: {}", stdout);
            } else if !stdout.contains("Requirement already satisfied") && !stdout.trim().is_empty() {
                // Log if anything interesting seemed to happen (heuristic)
                tracing::info!("pip install output: {}", stdout.lines().last().unwrap_or(""));
            }
            tracing::info!("Dependencies installed successfully from {:?}", requirements_path);
            Ok(())
        }
    }

    // Helper to run a method call against the python script
    pub async fn run_method<Req: Serialize, Resp: for<'de> Deserialize<'de>>(
        &self,
        method: &str,
        params: Req,
    ) -> Result<Resp, PluginError> {

        // Ensure initialized (gets existing or calls self.initialize)
        // .as_ref().map_err adjusts the error type returned by OnceCell's get_or_try_init
        let init_state = self.init_state
            .get_or_init(self.initialize())
            .await
            .as_ref()
            .map_err(|e| PluginError::InitializationError(e.to_string()))?
            .clone(); // Clone the simple state struct

        // Todo: move to core
        // --- Check Required Env Vars ---
        // let mut env_vars = HashMap::new();
        // for var_name in &self.required_env_vars {
        //     match std::env::var(var_name) {
        //         Ok(value) => {
        //             env_vars.insert(var_name.clone(), value);
        //         }
        //         Err(_) => return Err(PluginError::MissingEnvironmentVariable(var_name.clone())),
        //     }
        // }

        // --- Prepare Request ---
        // Create request
        let request = PluginRequest { method, params };
        // Serialize the request to be sent via stdin
        let request_json = serde_json::to_vec(&request)
            .map_err(PluginError::RequestSerializationError)?;

        // --- Spawn Process ---
        // Spawn the Python process from the virtual environment
        let mut child = Command::new(&init_state.venv_python_path)
            .arg(&init_state.plugin_script_path)
            .envs(&self.env_vars) // Pass required env vars
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| PluginError::ProcessSpawnError(self.script_name.clone(), e))?;

        // --- Write to Stdin ---
        // Get stdin handle. Use `take` to prevent holding `child` borrow across await point.
        let mut stdin = child.stdin.take().ok_or_else(|| PluginError::IoError(
             std::io::Error::new(std::io::ErrorKind::Other, "Failed to open stdin")
        ))?;
        // Spawn a task to write stdin to avoid potential deadlocks if buffers fill
        let write_task = tokio::spawn(async move {
            // Send the serialized request JSON
            stdin.write_all(&request_json).await?;
            // Close stdin to signal the end of input to the Python script
            stdin.shutdown().await?;
            Ok::<(), std::io::Error>(())
        });


        // --- Read Stdout & Stderr Concurrently ---
        let mut stdout_handle = child.stdout.take().ok_or_else(|| PluginError::IoError(
            std::io::Error::new(std::io::ErrorKind::Other, "Failed to open stdout")
        ))?;
        let mut stderr_handle = child.stderr.take().ok_or_else(|| PluginError::IoError(
            std::io::Error::new(std::io::ErrorKind::Other, "Failed to open stderr")
        ))?;

        let mut stdout_buf = Vec::new();    // Buffer to capture stdout (expected: JSON result)
        let mut stderr_buf = Vec::new();    // Buffer to capture stderr (expected: diagnostic logs)

        // Read both streams until EOF
        let (stdout_res, stderr_res) = tokio::join!(
            stdout_handle.read_to_end(&mut stdout_buf),
            stderr_handle.read_to_end(&mut stderr_buf)
        );

        // --- Wait for Process Exit and Stdin Write ---
        let status = child.wait().await?;

        // Check stdin write result
        write_task.await.unwrap().map_err(PluginError::StdinWriteError)?; // Handle task join error + IO Error

        // Capture stderr string regardless of exit status for logging/debugging
        let stderr_str = String::from_utf8_lossy(&stderr_buf).to_string();
        // Log stderr content if present (especially useful on errors)
        if !stderr_str.trim().is_empty() {
            // Use DEBUG level for successful runs, WARN for failures
            if status.success() {
                tracing::debug!("Plugin '{}' stderr: {}", self.plugin_name, stderr_str.trim_end());
            } else {
                tracing::warn!("Plugin '{}' stderr: {}", self.plugin_name, stderr_str.trim_end());
            }
        }

        // Propagate IO errors from reading stdout/stderr
        stdout_res.map_err(PluginError::StdoutReadError)?;
        stderr_res.map_err(PluginError::StderrReadError)?;

        // --- Handle based on Exit Status ---
        if !status.success() {
            // --- Process Failed ---
            // The Python script likely crashed or exited with sys.exit(1).

            // FIRST: Try to parse stdout for a structured PluginResponse::Error
            // The Python script SHOULD output a '{"status": "error", ...}' JSON to stdout for 
            // logical errors.
            if !stdout_buf.is_empty() {
                // Use Value for Resp placeholder, we only care about the error case here
                match serde_json::from_slice::<PluginResponse<serde_json::Value>>(&stdout_buf) {
                    Ok(PluginResponse::Error { message, .. }) => {
                        // Successfully parsed a structured error from stdout! Use this.
                        tracing::warn!(
                            "Plugin '{}' exited non-zero and reported error: {}",
                            self.plugin_name, message
                        );
                        return Err(PluginError::PluginReportedError(message));
                    }
                    Ok(PluginResponse::Success { .. }) => {
                        // This is weird: non-zero exit but success JSON? Log it.
                        tracing::error!(
                            "Plugin '{}' exited non-zero ({}) but produced success JSON on stdout. Falling back to ProcessFailed.",
                            self.plugin_name, status
                        );
                        // Fall through to return ProcessFailed below
                    }
                    Err(parse_err) => {
                        // Stdout wasn't a valid PluginResponse. Log this.
                        let stdout_preview = String::from_utf8_lossy(&stdout_buf);
                        tracing::warn!(
                            "Plugin '{}' exited non-zero ({}) and stdout was not a valid PluginResponse JSON ({}). Stdout preview: '{}'",
                            self.plugin_name, status, parse_err, stdout_preview.chars().take(100).collect::<String>()
                        );
                        // Fall through to return ProcessFailed below
                    }
                }
            } else {
                tracing::warn!("Plugin '{}' exited non-zero ({}) and stdout was empty.", self.plugin_name, status);
                // Fall through to return ProcessFailed below
            }

            // Fallback: If stdout was empty, unparseable, or contained Success JSON,
            // report the process failure with the status and stderr content.
            return Err(PluginError::ProcessFailed(status, stderr_str));

        } else {
            // --- Process Succeeded (Exit Code 0) ---
            // Expect a valid JSON response (Success or potentially Error) on stdout.
            let response_str = String::from_utf8_lossy(&stdout_buf); // For error reporting if needed
            // Deserialize the JSON from stdout into the expected PluginResponse structure
            let response: PluginResponse<Resp> = serde_json::from_slice(&stdout_buf)
                .map_err(|e| PluginError::ResponseDeserializationError(e, response_str.to_string()))?;

            match response {
                PluginResponse::Success { result } => {
                    tracing::debug!("Plugin '{}' returned success.", self.plugin_name);
                    Ok(result)
                },
                PluginResponse::Error { message, .. } => {
                    // Note: Python script *should* exit non-zero if it sends this,
                    // but handle defensively in case it doesn't.
                    tracing::warn!(
                        "Plugin '{}' exited successfully (0) but reported error: {}",
                        self.plugin_name, message
                    );
                    Err(PluginError::PluginReportedError(message))
                }
            }
        }
    }
}

// impl Extension for StdioWrapper {
//     fn uri(&self) -> &str {
//         &self.uri
//     }
//     fn name(&self) -> &str {
//         "Python via stdio"
//     }
//     fn description(&self) -> &str {
//         "Plugins implemented in Python"
//     }
//     fn chat_model(&self) -> Option<&markhor_core::chat::DynChatModel> {
//         self.chat_models.get().unwrap().first().map(Box::as_ref)
//     }
// }

// Structure to hold the state after successful initialization
#[derive(Debug, Clone)]
struct InitializedState {
    venv_python_path: PathBuf,
    plugin_script_path: PathBuf,
    // Add other static info derived during init if needed
}

// --- Platform-specific Venv Paths ---

#[cfg(target_os = "windows")]
fn get_venv_python_path(venv_path: &Path) -> PathBuf {
    venv_path.join("Scripts").join("python.exe")
}

#[cfg(not(target_os = "windows"))]
fn get_venv_python_path(venv_path: &Path) -> PathBuf {
    venv_path.join("bin").join("python")
}

#[cfg(target_os = "windows")]
fn get_venv_pip_path(venv_path: &Path) -> PathBuf {
    venv_path.join("Scripts").join("pip.exe")
}

#[cfg(not(target_os = "windows"))]
fn get_venv_pip_path(venv_path: &Path) -> PathBuf {
    venv_path.join("bin").join("pip")
}