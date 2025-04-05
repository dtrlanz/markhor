use crate::storage::{metadata, ConflictError, Error, Result};
use crate::storage::metadata::DocumentMetadata;
use crate::storage::ContentFile;
use regex::Regex;
use uuid::Uuid;
use std::ffi::OsStr;
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::fs;
use tracing::{debug, instrument, warn};

use super::Workspace; // Optional: for logging

const MARKHOR_EXTENSION: &str = "markhor";

/// Represents a Markhor document, defined by a `.markhor` metadata file
/// and consisting of associated files in the same directory.
#[derive(Debug, Clone)]
pub struct Document {
    // Absolute path to the .markhor file
    pub(crate) absolute_path: PathBuf,
    // Workspace owning this document
    workspace: Arc<Workspace>,
    metadata: DocumentMetadata,
}

impl Document {
    /// Opens an existing document by reading its `.markhor` file.
    ///
    /// Checks if the file exists and is accessible.
    #[instrument(skip(absolute_path), fields(path = %absolute_path.display()))]
    pub(crate) async fn open(absolute_path: PathBuf, workspace: Arc<Workspace>) -> Result<Self> {
        validate_markhor_path(&absolute_path)?;

        // Ensure the file exists and we can read it (basic check)
        // `read_metadata` will perform the actual read.
        if !fs::try_exists(&absolute_path).await.map_err(Error::Io)? {
            return Err(Error::FileNotFound(absolute_path));
        }
        if !fs::metadata(&absolute_path).await.map_err(Error::Io)?.is_file() {
            return Err(Error::InvalidPath(format!("Path is not a file: {}", absolute_path.display())));
        }

        // Try reading metadata to confirm it's a valid document structure
        let metadata = Self::read_metadata_internal(&absolute_path).await?;

        debug!("Document opened successfully");
        Ok(Document { absolute_path, workspace, metadata })
    }

    /// Creates a new document with a `.markhor` file at the specified path.
    ///
    /// Performs conflict checks to ensure the new document doesn't clash
    /// with existing files or documents in the target directory according
    /// to the defined ambiguity rules.
    #[instrument(skip(absolute_path), fields(path = %absolute_path.display()))]
    pub(crate) async fn create(absolute_path: PathBuf, workspace: Arc<Workspace>) -> Result<Self> {
        validate_markhor_path(&absolute_path)?;
        let (dir, basename) = get_dir_and_basename(&absolute_path)?;

        // --- Conflict Detection ---
        check_for_conflicts(&dir, &basename).await?;
        // --- End Conflict Detection ---

        debug!("Conflict check passed. Creating new document.");
        let metadata = DocumentMetadata::new();
        let content = serde_json::to_string_pretty(&metadata)?;

        fs::write(&absolute_path, content)
            .await
            .map_err(Error::Io)?;

        debug!("Document metadata file created successfully.");
        Ok(Document { absolute_path, workspace, metadata })
    }

    /// Returns the relative path to the document's `.markhor` file within the workspace.
    pub fn path(&self) -> &Path {
        &self.absolute_path.strip_prefix(&self.workspace.absolute_path)
            .expect("Internal error: Document is not in workspace")
    }

    pub fn name(&self) -> &str {
        self.absolute_path.file_stem()
            .and_then(OsStr::to_str).unwrap()
    }

    pub fn id(&self) -> &Uuid {
        &self.metadata.id
    }

    /// Reads and deserializes the document's metadata from its `.markhor` file.
    #[instrument(skip(self))]
    pub(crate) async fn read_metadata(&self) -> Result<DocumentMetadata> {
        Self::read_metadata_internal(&self.absolute_path).await
    }

    /// Internal helper for reading metadata
    async fn read_metadata_internal(path: &Path) -> Result<DocumentMetadata> {
         debug!("Reading metadata from {}", path.display());
         let content = fs::read(path)
             .await
             .map_err(|e| {
                if e.kind() == std::io::ErrorKind::NotFound {
                    Error::FileNotFound(path.to_path_buf())
                } else {
                    Error::Io(e)
                }
             })?;
         let metadata: DocumentMetadata = serde_json::from_slice(&content)?;
         Ok(metadata)
    }


    /// Serializes and writes the provided metadata to the document's `.markhor` file.
    #[instrument(skip(self, metadata))]
    pub(crate) async fn save_metadata(&self, metadata: &DocumentMetadata) -> Result<()> {
        debug!("Saving metadata to {}", self.absolute_path.display());
        let content = serde_json::to_string_pretty(metadata)?;
        fs::write(&self.absolute_path, content)
            .await
            .map_err(Error::Io)?;
        debug!("Metadata saved successfully.");
        Ok(())
    }

    /// Moves the document (including its `.markhor` file and all associated files)
    /// to a new location and/or gives it a new basename.
    ///
    /// The `new_markhor_path` must end in `.markhor`.
    /// Performs conflict checks in the destination directory before moving.
    /// Returns an updated `Document` instance pointing to the new location.
    #[instrument(skip(self), fields(new_path = %new_markhor_path.display()))]
    pub async fn move_to(mut self, new_markhor_path: PathBuf) -> Result<Self> {
        validate_markhor_path(&new_markhor_path)?;
        let (_old_dir, old_basename) = get_dir_and_basename(&self.absolute_path)?;
        let (new_dir, new_basename) = get_dir_and_basename(&new_markhor_path)?;

        if self.absolute_path == new_markhor_path {
            debug!("Move target is the same as current path, no action needed.");
            return Ok(self); // No-op
        }

        // --- Conflict Detection in Destination ---
        // Important: Check for conflicts *before* starting the move.
        // Skip check if the file being moved *is* the potential conflict
        // (e.g., moving doc.markhor to doc.markhor in the same dir - already handled)
        check_for_conflicts(&new_dir, &new_basename).await?;
        // --- End Conflict Detection ---

        debug!("Conflict check passed. Proceeding with move.");
        let files_to_move = self.list_all_associated_files().await?;

        // Use a staging approach? For simplicity now, move directly.
        // Note: This is NOT atomic across all files. If one rename fails,
        // the document might be in an inconsistent state.
        // A more robust implementation might move to a temp dir first.

        for old_file_path in files_to_move {
             let file_name = old_file_path
                 .file_name()
                 .ok_or_else(|| Error::InvalidPath(format!("Cannot get filename for {}", old_file_path.display())))?;

             let new_file_path = if old_file_path == self.absolute_path {
                 new_markhor_path.clone() // Handle the .markhor file itself
             } else {
                 // Construct new path based on new basename and original extension/suffix part
                 let original_filename = file_name.to_string_lossy();
                 let suffix_part = original_filename
                     .strip_prefix(&old_basename)
                     .ok_or_else(|| Error::InvalidPath(format!("File {} does not match basename {}", original_filename, old_basename)))?;

                new_dir.join(format!("{}{}", new_basename, suffix_part))
             };

             debug!("Moving {} -> {}", old_file_path.display(), new_file_path.display());
             fs::rename(&old_file_path, &new_file_path)
                .await
                .map_err(|e| {
                    warn!("Failed to move file {} to {}: {}. Document may be in inconsistent state.", old_file_path.display(), new_file_path.display(), e);
                    Error::Io(e)
                })?;
        }

        // Update the document's path internally
        self.absolute_path = new_markhor_path;
        debug!("Move operation completed.");
        Ok(self)
    }


    /// Deletes the document's `.markhor` file and all associated files.
    ///
    /// This operation is potentially destructive and irreversible.
    #[instrument(skip(self))]
    pub async fn delete(self) -> Result<()> {
        debug!("Attempting to delete document: {}", self.absolute_path.display());
        let files_to_delete = self.list_all_associated_files().await?;

        let mut errors = Vec::new();
        for file_path in files_to_delete {
            debug!("Deleting file: {}", file_path.display());
            if let Err(e) = fs::remove_file(&file_path).await {
                // Log error but continue trying to delete others
                warn!("Failed to delete file {}: {}", file_path.display(), e);
                if e.kind() != std::io::ErrorKind::NotFound { // Don't error if already gone
                   errors.push(Error::Io(e));
                }
            }
        }

        if let Some(first_error) = errors.into_iter().next() {
             Err(first_error) // Return the first error encountered
        } else {
             debug!("Document deleted successfully.");
             Ok(())
        }
    }


    /// Returns a list of all files associated with this document
    /// (excluding the `.markhor` file itself).
    #[instrument(skip(self))]
    pub async fn files(&self) -> Result<Vec<ContentFile>> {
         self.list_content_files_internal(None).await
    }

    /// Returns a list of associated files filtered by a specific extension.
    /// The extension should be provided *without* the leading dot (e.g., "pdf", "txt").
    #[instrument(skip(self))]
    pub async fn files_by_extension(&self, extension: &str) -> Result<Vec<ContentFile>> {
        self.list_content_files_internal(Some(extension)).await
    }


    // --- Internal Helpers ---

    /// Lists ContentFile instances, optionally filtering by extension.
    async fn list_content_files_internal(&self, extension_filter: Option<&str>) -> Result<Vec<ContentFile>> {
        let (dir, basename) = get_dir_and_basename(&self.absolute_path)?;
        let mut files = Vec::new();
        let mut read_dir = fs::read_dir(&dir).await.map_err(|e| {
             if e.kind() == std::io::ErrorKind::NotFound {
                Error::DirectoryNotFound(dir.clone())
             } else {
                 Error::Io(e)
             }
         })?;

        while let Some(entry) = read_dir.next_entry().await.map_err(Error::Io)? {
            let path = entry.path();
            if path.is_file() {
                // Skip the .markhor file itself
                if path == self.absolute_path {
                    continue;
                }

                if let Some(file_name) = path.file_name().and_then(OsStr::to_str) {
                     if is_potential_content_file(file_name, &basename) {
                        // Apply extension filter if provided
                        if let Some(filter_ext) = extension_filter {
                            if path.extension().and_then(OsStr::to_str) == Some(filter_ext) {
                                files.push(ContentFile::new(path, self));
                            }
                        } else {
                             // No filter, add the file
                             files.push(ContentFile::new(path, self));
                        }
                    }
                }
            }
        }
        Ok(files)
    }

    /// Lists all files belonging to the document, *including* the .markhor file.
    /// Used internally for move/delete operations.
    async fn list_all_associated_files(&self) -> Result<Vec<PathBuf>> {
         let (dir, basename) = get_dir_and_basename(&self.absolute_path)?;
         let mut paths = vec![self.absolute_path.clone()]; // Start with the metadata file
         let mut read_dir = fs::read_dir(dir).await.map_err(Error::Io)?;

         while let Some(entry) = read_dir.next_entry().await.map_err(Error::Io)? {
            let path = entry.path();
            if path.is_file() && path != self.absolute_path { // Exclude markhor here
                 if let Some(file_name) = path.file_name().and_then(OsStr::to_str) {
                     if is_potential_content_file(file_name, &basename) {
                        paths.push(path);
                    }
                }
            }
        }
        Ok(paths)
    }
}


// --- Standalone Helper Functions ---

/// Validates that a path points to a potential `.markhor` file.
fn validate_markhor_path(path: &Path) -> Result<()> {
    if path.extension().and_then(OsStr::to_str) != Some(MARKHOR_EXTENSION) {
        return Err(Error::NotMarkhorFile(path.to_path_buf()));
    }
    if path.file_stem().is_none() {
        return Err(Error::NoFileStem(path.to_path_buf()));
    }
    if path.parent().is_none() {
        return Err(Error::NoParentDirectory(path.to_path_buf()));
    }
    Ok(())
}

/// Extracts the parent directory and base filename (stem) from a path.
fn get_dir_and_basename(path: &Path) -> Result<(PathBuf, String)> {
    let dir = path.parent()
        .ok_or_else(|| Error::NoParentDirectory(path.to_path_buf()))?
        .to_path_buf();
    let basename = path.file_stem()
        .and_then(OsStr::to_str)
        .ok_or_else(|| Error::NoFileStem(path.to_path_buf()))?
        .to_string();
    Ok((dir, basename))
}

/// Parses a file stem into its "true base" and an optional hex suffix.
/// E.g., "doc.1a" -> ("doc", Some("1a")), "mydoc" -> ("mydoc", None)
fn parse_basename(stem: &str) -> Result<(String, Option<String>)> {
    // Use lazy_static or once_cell for better performance if called frequently
    let hex_suffix_re = Regex::new(r"^(.*)\.([0-9a-fA-F]+)$").unwrap(); // Handle potential regex error better in real code

    if let Some(captures) = hex_suffix_re.captures(stem) {
        // Check if the "base" part itself could be misinterpreted (e.g. "doc.1.2")
        // For now, assume the regex correctly finds the *last* hex part as the suffix.
        let true_base = captures.get(1).map_or("", |m| m.as_str()).to_string();
        let hex_suffix = captures.get(2).map_or("", |m| m.as_str()).to_string();

        if true_base.is_empty() {
            // Avoid case like ".a1f.markhor" being parsed incorrectly
            Err(Error::BasenameParseError(stem.to_string()))
        } else {
            Ok((true_base, Some(hex_suffix)))
        }
    } else {
        // No hex suffix found
        Ok((stem.to_string(), None))
    }
}

/// Checks if a filename matches the pattern for belonging to a document
/// with the given basename (`basename.*` or `basename.{hex}.*`).
fn is_potential_content_file(filename: &str, doc_basename: &str) -> bool {
    if !filename.starts_with(doc_basename) {
        return false;
    }
    if filename.ends_with(&format!(".{}", MARKHOR_EXTENSION)) {
        return false; // Exclude .markhor files themselves
    }

    let remainder = &filename[doc_basename.len()..];

    // Case 1: filename == doc_basename (should not happen for files other than .markhor?)
    // If it does, it doesn't fit the pattern with an extension.
    if remainder.is_empty() {
        return false;
    }

    // Case 2: Direct extension (e.g., "doc.txt")
    if remainder.starts_with('.') && !remainder.contains('/') && remainder.len() > 1 { // Ensure it has an extension part
         // Check if the part *after* the first dot looks like a hex suffix or not
         let mut parts = remainder[1..].splitn(2, '.');
         let first_part = parts.next().unwrap_or("");
         let second_part = parts.next(); // The actual extension if hex suffix exists

         if second_part.is_some() { // e.g., ".a1.txt"
              // Check if first_part is purely hex
              if !first_part.is_empty() && first_part.chars().all(|c| c.is_ascii_hexdigit()) {
                 return true; // Matches basename.{hex}.*
              } else {
                  // It's something like "basename.something.txt" where "something" is not hex
                  // This should *not* match if we interpret the rule strictly.
                  // Or should it? Let's assume NOT for now based on `basename.{hex}.*`
                  return false;
              }
         } else { // e.g., ".txt"
            // No second part means it's basename.ext
             return true;
         }
    }

    false
}


/// Performs conflict checks before creating or moving a document.
async fn check_for_conflicts(target_dir: &Path, target_basename: &str) -> Result<()> {
    debug!("Checking for conflicts for basename '{}' in directory '{}'", target_basename, target_dir.display());

    let target_markhor_path = target_dir.join(format!("{}.{}", target_basename, MARKHOR_EXTENSION));
    let (target_true_base, target_hex_suffix) = parse_basename(target_basename)?;

    let mut read_dir = match fs::read_dir(target_dir).await {
        Ok(rd) => rd,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            // Target directory doesn't exist, so no conflicts within it.
            // The `create` or `move_to` operation might create it later.
            return Ok(());
        }
        Err(e) => return Err(Error::Io(e)),
    };

    let mut existing_suffixed_documents = Vec::new();
    let mut existing_base_document_found = false;


    while let Some(entry) = read_dir.next_entry().await.map_err(Error::Io)? {
        let path = entry.path();
        let file_name_os = entry.file_name();
        let Some(file_name) = file_name_os.to_str() else { continue; }; // Skip non-unicode filenames

        if !path.is_file() {
            continue; // Skip directories and other non-file entries
        }

        // Rule 1 Check: Direct .markhor conflict
        if path == target_markhor_path {
             debug!("Conflict Rule 1 Triggered: Markhor file exists: {}", path.display());
             return Err(ConflictError::MarkhorFileExists(path).into());
        }

        // Rule 2 Check: Existing file would be adopted?
        // This check ensures *no* file would be implicitly "adopted" by the new document, 
        // potentially misrepresenting its origin.
        if is_potential_content_file(file_name, target_basename) {
            // Found a file (e.g., target_basename.txt or target_basename.a1.pdf)
            // that would match the *new* document's pattern.
            // This is disallowed to prevent accidental adoption.
            debug!("Conflict Rule 2 Triggered: Existing file {} would be adopted by {}", path.display(), target_basename);
            return Err(ConflictError::ExistingFileWouldBeAdopted(path).into());
        }

        // --- Gather info for ambiguity checks ---
        if file_name.ends_with(&format!(".{}", MARKHOR_EXTENSION)) {
            if let Some(stem) = path.file_stem().and_then(OsStr::to_str) {
                 match parse_basename(stem) {
                     Ok((true_base, Some(_))) => {
                        // This is an existing suffixed document
                        if true_base == target_true_base {
                            existing_suffixed_documents.push(stem.to_string());
                        }
                     }
                     Ok((true_base, None)) => {
                        // This is an existing base document
                        if true_base == target_true_base {
                            existing_base_document_found = true;
                        }
                     }
                     Err(_) => {
                         warn!("Could not parse basename for existing file: {}", path.display());
                     } // Ignore parse errors for existing files for now
                 }
             }
        }
        // --- End info gathering ---

    } // End while loop through directory entries

    // --- Ambiguity Checks (Rules 3 & 4) ---

    // Rule 3 Check: Suffix-Base Ambiguity
    // Is the target a suffixed doc (doc.4.markhor) AND does the base (doc.markhor) exist?
    if let Some(hex_suffix) = &target_hex_suffix {
        if existing_base_document_found {
             debug!("Conflict Rule 3 Triggered: Target '{}.{}.markhor' conflicts with existing base '{}.markhor'", target_true_base, hex_suffix, target_true_base);
             return Err(ConflictError::SuffixBaseAmbiguity(target_true_base.clone(), hex_suffix.clone()).into());
        }
    }

    // Rule 4 Check: Base-Suffix Ambiguity
    // Is the target a base doc (doc.markhor) AND does *any* suffixed version (doc.*.markhor) exist?
    if target_hex_suffix.is_none() && !existing_suffixed_documents.is_empty() {
         debug!("Conflict Rule 4 Triggered: Target '{}.markhor' conflicts with existing suffixed documents like '{}.markhor'", target_true_base, existing_suffixed_documents[0]);
         return Err(ConflictError::BaseSuffixAmbiguity(target_true_base.clone(), existing_suffixed_documents[0].clone()).into()); // Report first conflict
    }

    // --- End Ambiguity Checks ---

    debug!("No conflicts found.");
    Ok(())
}


// Example Usage (requires a tokio runtime)
#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    // Helper to create a dummy file
    async fn create_dummy_file(path: &Path) {
        fs::write(path, "").await.expect("Failed to create dummy file");
    }

    // #[tokio::test]
    // async fn test_create_and_open_document() {
    //     let dir = tempdir().unwrap();
    //     let doc_path = dir.path().join("mydoc.markhor");

    //     let doc = Document::create(doc_path.clone(), dir.path().to_path_buf()).await.unwrap();
    //     assert!(doc_path.exists());

    //     let metadata = doc.read_metadata().await.unwrap();
    //     println!("Created doc with UUID: {}", metadata.id);

    //     let opened_doc = Document::open(doc_path.clone(), dir.path().to_path_buf()).await.unwrap();
    //     let opened_metadata = opened_doc.read_metadata().await.unwrap();
    //     assert_eq!(metadata.id, opened_metadata.id);

    //     opened_doc.delete().await.unwrap();
    //     assert!(!doc_path.exists());
    // }

    //  #[tokio::test]
    // async fn test_list_files() {
    //     let dir = tempdir().unwrap();
    //     let doc_path = dir.path().join("testdoc.markhor");
    //     let pdf_path = dir.path().join("testdoc.pdf");
    //     let txt_path = dir.path().join("testdoc.txt");
    //     let hex_txt_path = dir.path().join("testdoc.a1f.txt");
    //     let unrelated_path = dir.path().join("other.txt");
    //     let unrelated_hex_path = dir.path().join("testdoc_extra.txt"); // Doesn't match pattern

    //     let doc = Document::create(doc_path.clone(), dir.path().to_path_buf()).await.unwrap();
    //     create_dummy_file(&pdf_path).await;
    //     create_dummy_file(&txt_path).await;
    //     create_dummy_file(&hex_txt_path).await;
    //     create_dummy_file(&unrelated_path).await;
    //     create_dummy_file(&unrelated_hex_path).await;

    //     let files = doc.files().await.unwrap();
    //     assert_eq!(files.len(), 3);
    //     let paths: Vec<_> = files.iter().map(|f| f.path().to_path_buf()).collect();
    //     assert!(paths.contains(&pdf_path));
    //     assert!(paths.contains(&txt_path));
    //     assert!(paths.contains(&hex_txt_path));

    //     let txt_files = doc.files_by_extension("txt").await.unwrap();
    //     assert_eq!(txt_files.len(), 2);
    //      let txt_paths: Vec<_> = txt_files.iter().map(|f| f.path().to_path_buf()).collect();
    //     assert!(txt_paths.contains(&txt_path));
    //     assert!(txt_paths.contains(&hex_txt_path));

    //     doc.delete().await.unwrap();
    //     assert!(!doc_path.exists());
    //     assert!(!pdf_path.exists());
    //     assert!(!txt_path.exists());
    //     assert!(!hex_txt_path.exists());
    //     // Unrelated files should remain
    //     assert!(unrelated_path.exists());
    //      assert!(unrelated_hex_path.exists());
    // }

    // #[tokio::test]
    // async fn test_conflict_rule1_markhor_exists() {
    //     let dir = tempdir().unwrap();
    //     let doc_path = dir.path().join("conflict1.markhor");
    //     create_dummy_file(&doc_path).await; // Pre-create the file

    //     let result = Document::create(doc_path.clone(), dir.path().to_path_buf()).await;
    //     assert!(matches!(result, Err(Error::Conflict(ConflictError::MarkhorFileExists(_)))));
    // }

    //  #[tokio::test]
    // async fn test_conflict_rule2_file_would_be_adopted() {
    //     let dir = tempdir().unwrap();
    //     let doc_path = dir.path().join("conflict2.markhor");
    //     let existing_file = dir.path().join("conflict2.txt"); // Would be adopted
    //     create_dummy_file(&existing_file).await;

    //     let result = Document::create(doc_path.clone(), dir.path().to_path_buf()).await;
    //     assert!(matches!(result, Err(Error::Conflict(ConflictError::ExistingFileWouldBeAdopted(_)))));

    //     let existing_hex_file = dir.path().join("conflict2.a1.pdf"); // Would also be adopted
    //     create_dummy_file(&existing_hex_file).await;
    //     let result2 = Document::create(doc_path.clone(), dir.path().to_path_buf()).await;
    //      assert!(matches!(result2, Err(Error::Conflict(ConflictError::ExistingFileWouldBeAdopted(_)))));
    // }

    // #[tokio::test]
    // async fn test_conflict_rule3_suffix_base_ambiguity() {
    //     let dir = tempdir().unwrap();
    //     let base_doc_path = dir.path().join("conflict3.markhor");
    //     let suffix_doc_path = dir.path().join("conflict3.4a.markhor");

    //     // Create the base document first
    //     Document::create(base_doc_path.clone(), dir.path().to_path_buf()).await.unwrap();

    //     // Now try to create the suffixed one - should conflict
    //     let result = Document::create(suffix_doc_path.clone(), dir.path().to_path_buf()).await;
    //      println!("{:?}", result); // Debug print
    //     assert!(matches!(result, Err(Error::Conflict(ConflictError::SuffixBaseAmbiguity(b,s))) if b == "conflict3" && s == "4a"));
    // }

    //  #[tokio::test]
    // async fn test_conflict_rule4_base_suffix_ambiguity() {
    //     let dir = tempdir().unwrap();
    //     let base_doc_path = dir.path().join("conflict4.markhor");
    //     let suffix_doc_path = dir.path().join("conflict4.4a.markhor");

    //     // Create the suffixed document first
    //     Document::create(suffix_doc_path.clone(), dir.path().to_path_buf()).await.unwrap();

    //     // Now try to create the base one - should conflict
    //     let result = Document::create(base_doc_path.clone(), dir.path().to_path_buf()).await;
    //      println!("{:?}", result); // Debug print
    //     assert!(matches!(result, Err(Error::Conflict(ConflictError::BaseSuffixAmbiguity(b,s))) if b == "conflict4" && s == "conflict4.4a"));
    // }

    // #[tokio::test]
    // async fn test_move_document() {
    //     let dir = tempdir().unwrap();
    //     let old_doc_path = dir.path().join("move_me.markhor");
    //     let old_file_path = dir.path().join("move_me.data");
    //     let new_doc_path = dir.path().join("subdir/moved_doc.markhor");

    //     // Create target subdir
    //     fs::create_dir(dir.path().join("subdir")).await.unwrap();

    //     let doc = Document::create(old_doc_path.clone(), dir.path().to_path_buf()).await.unwrap();
    //     create_dummy_file(&old_file_path).await;

    //     assert!(old_doc_path.exists());
    //     assert!(old_file_path.exists());

    //     let moved_doc = doc.move_to(new_doc_path.clone()).await.unwrap();

    //     assert!(!old_doc_path.exists());
    //     assert!(!old_file_path.exists());
    //     assert!(new_doc_path.exists());
    //     assert!(dir.path().join("subdir/moved_doc.data").exists()); // Check associated file moved correctly

    //     // Check internal path updated
    //     assert_eq!(moved_doc.absolute_path, new_doc_path);

    //     // Clean up
    //     moved_doc.delete().await.unwrap();
    //      assert!(!new_doc_path.exists());
    //     assert!(!dir.path().join("subdir/moved_doc.data").exists());
    // }

    //  #[tokio::test]
    // async fn test_move_conflict() {
    //     let dir = tempdir().unwrap();
    //     let doc1_path = dir.path().join("doc1.markhor");
    //     let doc2_path = dir.path().join("doc2.markhor");
    //     let conflicting_file = dir.path().join("doc1.txt"); // Will conflict if doc2 moves to doc1

    //     let doc1 = Document::create(doc1_path.clone(), dir.path().to_path_buf()).await.unwrap();
    //     let doc2 = Document::create(doc2_path.clone(), dir.path().to_path_buf()).await.unwrap();
    //     create_dummy_file(&conflicting_file).await; // Create file potentially owned by doc1

    //      // Try moving doc2 to doc1 -> Conflict Rule 1 (MarkhorFileExists) takes precedence here
    //     let move_result = doc2.move_to(doc1_path.clone()).await;
    //     assert!(matches!(move_result, Err(Error::Conflict(ConflictError::MarkhorFileExists(p))) if p == doc1_path));

    //     // Need to reload doc2 as it was consumed by the failed move attempt
    //     let doc2_reloaded = Document::open(doc2_path, dir.path().to_path_buf()).await.unwrap();
    //     doc1.delete().await.unwrap();
    //     doc2_reloaded.delete().await.unwrap();
    // }

    #[tokio::test]
    async fn test_parse_basename_logic() {
        assert_eq!(parse_basename("doc").unwrap(), ("doc".to_string(), None));
        assert_eq!(parse_basename("doc.txt").unwrap(), ("doc.txt".to_string(), None));
        assert_eq!(parse_basename("doc.1a").unwrap(), ("doc".to_string(), Some("1a".to_string())));
        assert_eq!(parse_basename("my.doc.with.dots.f0f").unwrap(), ("my.doc.with.dots".to_string(), Some("f0f".to_string())));
        assert_eq!(parse_basename("nodigits.a").unwrap(), ("nodigits".to_string(), Some("a".to_string()))); // "a" is hex, regex matches
        assert!(parse_basename(".abc").is_err()); // Invalid starting dot
        assert_eq!(parse_basename("doc.1").unwrap(), ("doc".to_string(), Some("1".to_string())));
        assert_eq!(parse_basename("doc.1.2").unwrap(), ("doc.1".to_string(), Some("2".to_string()))); // Regex matches last part
    }

     #[tokio::test]
    async fn test_is_potential_content_file_logic() {
        // Base doc: "mydoc"
        assert!(is_potential_content_file("mydoc.txt", "mydoc"));
        assert!(is_potential_content_file("mydoc.pdf", "mydoc"));
        assert!(is_potential_content_file("mydoc.a1.txt", "mydoc"));
        assert!(is_potential_content_file("mydoc.00ff.dat", "mydoc"));
        assert!(!is_potential_content_file("mydoc_extra.txt", "mydoc"));
        assert!(!is_potential_content_file("otherdoc.txt", "mydoc"));
        assert!(is_potential_content_file("mydoc.a1", "mydoc")); // "a1" is extension, not hex
        assert!(!is_potential_content_file("mydoc", "mydoc")); // No extension
        assert!(!is_potential_content_file("mydoc.markhor", "mydoc")); // Usually handled separately
        assert!(!is_potential_content_file("mydoc.v1.txt", "mydoc")); // "v1" is not hex


        // Suffixed doc: "report.v1.a0"
        let basename = "report.v1.a0";
        assert!(is_potential_content_file("report.v1.a0.csv", basename));
        assert!(is_potential_content_file("report.v1.a0.b1.json", basename)); // report.v1.a0.{hex}.json
        assert!(!is_potential_content_file("report.v1.a0", basename));
        assert!(!is_potential_content_file("report.v1.a0txt", basename)); // Needs dot separator
        assert!(!is_potential_content_file("report.v1.csv", basename)); // Belongs to report.v1 potentially
        assert!(!is_potential_content_file("report.v1.a0.markhor", basename));

    }
}