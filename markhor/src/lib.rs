use std::sync::Arc;

use markhor_core::storage::{Storage, Workspace};

pub mod cli;
pub mod commands;
// pub mod config;   // Add later
// pub mod error;    // Add later if needed
// pub mod workspace; // Add later

pub struct AppContext {
    pub storage: Arc<Storage>,
    pub workspace: anyhow::Result<Arc<Workspace>>,
}