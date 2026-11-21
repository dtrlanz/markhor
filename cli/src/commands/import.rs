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
    permissions::{Permission, Restricted},
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

    /// Who may access the document, e.g. `on-device`. Separate several with commas. Defaults to
    /// the workspace default. Redundant permissions are not stored.
    #[arg(short, long, value_delimiter = ',', conflicts_with = "unrestricted")]
    access: Option<Vec<String>>,

    /// Let anyone access the document. Cannot be combined with `--access`.
    #[arg(long)]
    unrestricted: bool,
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
    let requested = if args.unrestricted {
        Some(Vec::new())
    } else {
        let available = folder.workspace().available_permissions();
        args.access
            .as_deref()
            .map(|names| resolve_permissions(names, &available))
            .transpose()?
    };
    let options = ImportOptions {
        ocr: OcrOptions {
            pages: args.pages,
            include_images: !args.no_images,
        },
        page_mode: args.page_mode.into(),
        required_permissions: requested.clone(),
    };

    let doc = import(&folder, input, &*model, &options)
        .await
        .context("import failed")?;
    if let Some(requested) = &requested {
        report_implied(requested, doc.permissions_required());
    }
    println!("{}", doc.path().display());
    Ok(())
}

/// Looks up names among the `available` permissions. An unknown name is an error that lists the
/// valid names.
fn resolve_permissions(names: &[String], available: &[Permission]) -> Result<Vec<Permission>> {
    names
        .iter()
        .map(|name| {
            available
                .iter()
                .find(|permission| permission.name() == name.as_str())
                .cloned()
                .with_context(|| {
                    let valid: Vec<_> = available.iter().map(Permission::name).collect();
                    format!(
                        "unknown permission `{name}`; valid permissions are {}",
                        valid.join(", ")
                    )
                })
        })
        .collect()
}

/// Notes each requested permission that was not stored because a stored one implies it.
fn report_implied(requested: &[Permission], stored: &[Permission]) {
    for permission in requested {
        if stored.contains(permission) {
            continue;
        }
        if let Some(implier) = stored.iter().find(|s| *s > permission) {
            eprintln!(
                "note: `{}` is implied by `{}` and was not stored",
                permission.name(),
                implier.name()
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use markhor_core::permissions::{ON_DEVICE, PUBLIC};

    fn parse(args: &[&str]) -> Result<ImportArgs, clap::Error> {
        let matches =
            ImportArgs::augment_args(clap::Command::new("import")).try_get_matches_from(args)?;
        <ImportArgs as clap::FromArgMatches>::from_arg_matches(&matches)
    }

    #[test]
    fn access_splits_comma_separated_names() {
        let args = parse(&["import", "scan.pdf", "--access", "on-device,public"]).unwrap();
        assert_eq!(
            args.access,
            Some(vec!["on-device".to_owned(), "public".to_owned()])
        );
    }

    #[test]
    fn unrestricted_conflicts_with_access() {
        assert!(
            parse(&["import", "scan.pdf", "--unrestricted"])
                .unwrap()
                .unrestricted
        );
        assert!(parse(&["import", "scan.pdf", "--unrestricted", "--access", "public"]).is_err());
    }

    #[test]
    fn unknown_names_are_rejected_and_the_valid_ones_are_listed() {
        let available = [ON_DEVICE, PUBLIC];
        assert_eq!(
            resolve_permissions(&["public".to_owned()], &available).unwrap(),
            vec![PUBLIC]
        );
        let message = resolve_permissions(&["gdpr".to_owned()], &available)
            .unwrap_err()
            .to_string();
        assert!(message.contains("`gdpr`"), "{message}");
        assert!(message.contains("on-device, public"), "{message}");
    }
}
