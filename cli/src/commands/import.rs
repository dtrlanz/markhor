//! Imports a visual document into a workspace, recognizing its text with Mistral OCR.

use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::{Args, ValueEnum};
use markhor::import::{ImportOptions, PageMode, import};
use markhor_core::{
    dependencies::Session,
    extension::{Comp, ExtensionConfig},
    library::Folder,
    ocr::{OcrInput, OcrModel, OcrOptions},
};
use markhor_extensions::mistral::{Client, ClientConfig};

/// Import a document, recognizing its text with OCR.
#[derive(Args)]
pub struct ImportArgs {
    /// A local file, or an http(s) URL that Markhor downloads.
    input: String,

    /// Folder to import into. Defaults to the current directory. It must be inside a workspace.
    #[arg(long)]
    folder: Option<PathBuf>,

    /// Zero-based pages to recognize, e.g. `0,2,3`. Defaults to all pages.
    #[arg(long, value_delimiter = ',')]
    pages: Option<Vec<u32>>,

    /// Do not store the images found in the document. They are stored by default.
    #[arg(long)]
    no_images: bool,

    /// How pages are laid out in the text.
    #[arg(long, value_enum, default_value_t = PageModeArg::Milestones)]
    page_mode: PageModeArg,
}

/// Command-line names for [`PageMode`].
#[derive(Clone, Copy, ValueEnum)]
enum PageModeArg {
    Milestones,
    Parts,
    Merged,
}

impl From<PageModeArg> for PageMode {
    fn from(arg: PageModeArg) -> Self {
        match arg {
            PageModeArg::Milestones => PageMode::Milestones,
            PageModeArg::Parts => PageMode::PartPerPage,
            PageModeArg::Merged => PageMode::Merged,
        }
    }
}

pub async fn run(args: ImportArgs) -> Result<()> {
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
    let options = ImportOptions {
        ocr: OcrOptions {
            pages: args.pages,
            include_images: !args.no_images,
        },
        page_mode: args.page_mode.into(),
    };

    let doc = import(&folder, input, &*model, &options)
        .await
        .context("import failed")?;
    println!("{}", doc.path().display());
    Ok(())
}
