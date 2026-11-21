//! Imports a visual document into a workspace, recognizing its text with Mistral OCR.

use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::Parser;
use markhor_core::{
    dependencies::Session,
    extension::{Comp, ExtensionConfig},
    library::{Folder, import},
    ocr::{OcrInput, OcrModel, OcrOptions},
};
use markhor_extensions::mistral::{Client, ClientConfig};
use tracing_subscriber::EnvFilter;

/// Import a document, recognizing its text with OCR.
#[derive(Parser)]
#[command(version, about)]
struct Args {
    /// A local file, or an http(s) URL that Markhor downloads.
    input: String,

    /// Folder to import into. Defaults to the current directory. It must be inside a workspace.
    #[arg(long)]
    folder: Option<PathBuf>,

    /// Zero-based pages to recognize, e.g. `0,2,3`. Defaults to all pages.
    #[arg(long, value_delimiter = ',')]
    pages: Option<Vec<u32>>,

    /// Also store the images found in the document as attachments.
    #[arg(long)]
    include_images: bool,
}

#[tokio::main]
async fn main() -> Result<()> {
    dotenv::dotenv().ok();
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("warn")),
        )
        .with_writer(std::io::stderr)
        .init();
    let args = Args::parse();

    let mut session = Session::new();
    session
        .add_api_keys_from_env()
        .context("invalid API key environment variables")?;
    session
        .add_extension::<ClientConfig, Client>(ClientConfig::default(), ExtensionConfig::default())
        .await
        .context("failed to add the Mistral client; check MISTRAL_API_KEY and its _PERMISSIONS")?;
    let model = session
        .resolve::<Comp<dyn OcrModel>>()
        .context("no OCR model is available; set MISTRAL_API_KEY")?;

    let folder_path = std::path::absolute(match args.folder {
        Some(folder) => folder,
        None => std::env::current_dir()?,
    })?;
    let folder = Folder::open(&folder_path)
        .await
        .with_context(|| format!("cannot open {}", folder_path.display()))?;

    let input = if args.input.starts_with("http://") || args.input.starts_with("https://") {
        OcrInput::Url {
            url: args.input,
            mime: None,
        }
    } else {
        OcrInput::Path(PathBuf::from(args.input))
    };
    let options = OcrOptions {
        pages: args.pages,
        include_images: args.include_images,
    };

    let doc = import(&folder, input, &*model, &options)
        .await
        .context("import failed")?;
    println!("{}", doc.path().display());
    Ok(())
}
