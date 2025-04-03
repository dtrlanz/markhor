pub mod chat;
pub mod error;
pub mod wrapper;
pub mod plugin;
use serde::{Deserialize, Serialize};

// --- Data structures for internal communication ---

#[derive(Serialize, Debug)]
pub(crate) struct PluginRequest<'a, T: Serialize> {
    pub method: &'a str,
    pub params: T,
}

#[derive(Deserialize, Debug)]
#[serde(tag = "status")] // Expect 'status': 'success' or 'status': 'error'
#[serde(rename_all = "snake_case")]
pub(crate) enum PluginResponse<T, E = PluginErrorDetail> {
    Success { result: T },
    Error { message: String, #[serde(flatten)] details: Option<E> }, // Allow extra error fields
}

#[derive(Deserialize, Debug)]
pub(crate) struct PluginErrorDetail {
   // Add specific error codes or details if the python side sends them
   // pub error_code: Option<String>,
}
