//! Provides filesystem storage abstractions for managing workspaces, folders, and documents.
//!
//! This module defines the core structures and logic for interacting with the
//! application's data model on disk. It establishes conventions for how
//! workspaces, organizational folders, and multi-file documents are represented
//! in the file system.
//!
//! # Core Concepts
//!
//! *   **[`Workspace`]:** The root container for all managed data. A workspace corresponds
//!     to a directory on the filesystem. It contains documents, folders, and a special
//!     `.markhor` subdirectory for internal workspace configuration (like `config.json`)
//!     and potential future caches or indexes. Users typically start by [`Workspace::create`]ing
//!     or [`Workspace::open`]ing a workspace.
//! *   **[`Folder`]:** Represents a standard directory within a workspace used for
//!     organizing documents and other folders. Folders are discovered via methods like
//!     [`Workspace::list_folders`] or [`Folder::list_folders`].
//! *   **[`Document`]:** A logical representation of a single piece of content that may
//!     consist of multiple related files. For example, an imported PDF might result in
//!     a document containing the original `.pdf` file, a generated `.md` transcription,
//!     and perhaps an `.html` version.
//!     *   Documents are identified by a metadata file with a `.markhor` extension
//!         (e.g., `my_report.markhor`). This file contains metadata like a unique ID.
//!     *   All other files belonging to that document reside in the *same directory* and
//!         share the *same base name* as the `.markhor` file, but with different extensions.
//! *   **[`DocumentFile`]:** Represents an individual file that is part of a [`Document`]
//!     (excluding the `.markhor` metadata file itself). Instances are obtained via
//!     [`Document::files`] or [`Document::files_by_extension`].
//!
//! # File Naming Conventions
//!
//! The association between a document's metadata (`basename.markhor`) and its content
//! files is determined by naming convention:
//!
//! *   **Standard Files:** Files like `basename.pdf`, `basename.txt`.
//! *   **Suffixed Files:** In cases where a single document component might produce multiple
//!     files of the same type (e.g., splitting tabs into separate files), a hexadecimal
//!     suffix is used: `basename.{hex}.extension` (e.g., `basename.a1.md`, `basename.a2.md`).
//!
//! The library automatically discovers these files based on the document's base name.
//!
//! # Conflict Detection
//!
//! To prevent ambiguity and data corruption, strict conflict detection rules are enforced
//! when creating ([`Document::create`]) or moving ([`Document::move_to`]) documents. These
//! rules prevent:
//!
//! *   Direct overwrites of existing document metadata files.
//! *   Accidental "adoption" of unrelated files that happen to match a document's naming pattern.
//! *   Ambiguity between base documents (e.g., `doc.markhor`) and suffixed documents
//!     (e.g., `doc.4.markhor`) regarding ownership of files like `doc.4.txt`.
//!
//! Operations will return a [`ConflictError`] if any rule is violated.
//!
//! ## Conflict Rules for Creating/Moving Documents
//! 
//! A conflict exists in the target directory if **any** of the following conditions are met:
//! 
//! 1.  **Direct Markhor Conflict:** The file `target_basename.markhor` already exists.
//!     *   *Example:* Trying to create `doc.markhor` when `doc.markhor` exists.
//!     *   *Reason:* Cannot have two identical document definitions.
//! 2.  **Orphan File Conflict:** An existing file (which is *not* a `.markhor` file) already matches the file pattern for the *potential document*. This file would be implicitly "adopted" by the new document, potentially misrepresenting its origin.
//!     *   *Example:* Trying to create `doc.markhor` when `doc.txt` exists.
//!     *   *Example:* Trying to create `doc.markhor` when `doc.a1.pdf` exists.
//!     *   *Example:* Trying to create `report.1a.markhor` when `report.1a.csv` exists.
//!     *   *Reason:* Avoids accidentally associating unrelated files with the new document. It forces explicit action if these files *should* belong to the new document (e.g., renaming or moving them first).
//! 3.  **Suffix-Base Ambiguity Conflict:** The potential document has a hex suffix (`target_basename = true_base.{hex}`), AND the corresponding base document (`true_base.markhor`) already exists.
//!     *   *Example:* Trying to create `doc.4.markhor` when `doc.markhor` exists.
//!     *   *Reason:* Files like `doc.4.txt` could potentially belong to *either* `doc.markhor` (as `doc.{hex}.txt`) *or* `doc.4.markhor` (as `doc.4.txt`). This creates ambiguity about ownership, even for files not yet created.
//! 4.  **Base-Suffix Ambiguity Conflict:** The potential document *does not* have a hex suffix (`target_basename = true_base`), AND *any* suffixed document (`true_base.{hex}.markhor`) already exists.
//!     *   *Example:* Trying to create `doc.markhor` when `doc.4.markhor` exists.
//!     *   *Reason:* Similar to rule 3, files like `doc.4.txt` would have ambiguous ownership between `doc.markhor` and `doc.4.markhor`.
//!
//! # Asynchronous API
//!
//! All filesystem I/O operations within this module are `async` and rely on the `tokio`
//! runtime. Methods that perform I/O return `Result<T, Error>`, where [`Error`] encapsulates
//! potential issues like I/O errors, serialization errors, or conflicts.
//!
//! # Example Usage
//!
//! ```rust,no_run
//! use markhor_core::storage::{Workspace, Document, Error}; // Adjust path as needed
//! use tempfile::tempdir; // For example purposes
//!
//! #[tokio::main]
//! async fn main() -> Result<(), Box<dyn std::error::Error>> {
//!     // Create a temporary directory for the workspace
//!     let temp_dir = tempdir()?;
//!     let ws_path = temp_dir.path().to_path_buf();
//!
//!     // Create a new workspace
//!     let ws = Workspace::create(ws_path.clone()).await?;
//!     println!("Workspace created at: {}", ws.path().display());
//!
//!     // Create a new document within the workspace
//!     let doc = ws.create_document("my_doc").await?;
//!     println!("Document created with ID: {}", doc.id());
//!
//!     // List documents in the workspace root
//!     let root_docs = ws.list_documents().await?;
//!     println!("Found {} documents in workspace root.", root_docs.len());
//!     assert_eq!(root_docs.len(), 1);
//!
//!     // Clean up (in real code, workspace persists)
//!     drop(temp_dir);
//!     Ok(())
//! }
//! ```

pub use self::document::Document;
pub use self::file::DocumentFile;
pub use self::folder::Folder;
pub use self::workspace::Workspace;

mod document;
mod folder;
mod workspace;
mod file;
mod metadata;

use std::path::PathBuf;
use thiserror::Error;

pub const MARKHOR_EXTENSION: &str = "markhor";
pub const INTERNAL_DIR_NAME: &str = ".markhor";


#[derive(Debug, Error)]
pub enum ConflictError {
    #[error("Target document file already exists: {0}")]
    MarkhorFileExists(PathBuf), // Rule 1

    #[error("Existing file would be ambiguously owned by the new document: {0}")]
    ExistingFileWouldBeAdopted(PathBuf), // Rule 2

    #[error("Document '{0}.{1}.markhor' would be ambiguous with existing base document '{0}.markhor'")]
    SuffixBaseAmbiguity(String, String), // Rule 3 (true_base, hex_suffix)

    #[error("Document '{0}.markhor' would be ambiguous with existing suffixed document '{1}.markhor'")]
    BaseSuffixAmbiguity(String, String), // Rule 4 (true_base, conflicting_suffixed_basename)
}

#[derive(Debug, Error)]
pub enum Error {
    #[error("Filesystem conflict")]
    Conflict(#[from] ConflictError),

    #[error("Invalid path provided: {0}")]
    InvalidPath(String),

    #[error("Path does not refer to a '.markhor' file: {0}")]
    NotMarkhorFile(PathBuf),

    #[error("Path does not have a valid parent directory: {0}")]
    NoParentDirectory(PathBuf),

    #[error("Path does not have a valid file stem: {0}")]
    NoFileStem(PathBuf),

    #[error("Metadata serialization/deserialization error")]
    Metadata(#[from] serde_json::Error),

    #[error("IO error")]
    Io(#[from] std::io::Error),

    #[error("File not found: {0}")]
    FileNotFound(PathBuf),

    #[error("Directory not found: {0}")]
    DirectoryNotFound(PathBuf),

    #[error("Could not parse basename into true base and hex suffix: {0}")]
    BasenameParseError(String),

    #[error("Path is not a directory: {0}")]
    NotADirectory(PathBuf),

    #[error("Path is not a valid workspace (missing '.markhor' subdirectory): {0}")]
    NotAWorkspace(PathBuf),

    #[error("Cannot create workspace: path exists and is not an empty directory: {0}")]
    WorkspaceCreationConflict(PathBuf), // Covers non-empty or existing .markhor dir

    #[error("Cannot create workspace: path exists and is a file: {0}")]
    PathIsFile(PathBuf),

    #[error("Cannot create directory: path already exists as a file: {0}")]
    CannotCreateDirNotAFile(PathBuf),

    #[error("Directory is not empty: {0}")]
    DirectoryNotEmpty(PathBuf), // Could be useful, maybe subsumed by WorkspaceCreationConflict for now    

    #[error("Workspace configuration file is missing or invalid: {0}")]
    InvalidWorkspaceConfig(PathBuf), // Covers missing or malformed config.json    
}

// Define a standard Result type for the library
pub type Result<T> = std::result::Result<T, Error>;