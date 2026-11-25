//! Creates a collection in the current folder.

use anyhow::{Context, Result};
use clap::Args;
use markhor_core::library::Collection;

/// Create a collection in the current folder.
#[derive(Args)]
pub struct InitArgs {}

pub async fn run(_args: InitArgs) -> Result<()> {
    let folder = std::env::current_dir().context("cannot determine the current folder")?;
    Collection::create(&folder)
        .await
        .with_context(|| format!("cannot create a collection in {}", folder.display()))?;
    println!("Created a collection in {}", folder.display());
    Ok(())
}
