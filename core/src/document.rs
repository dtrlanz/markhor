// Storage System Design:
//     Data Structures:
//         Folder: Represents a directory within the workspace.
//             Fields: path: PathBuf.
//         Document: Represents a document.
//             Fields: id: UUID, metadata: DocumentMetadata, files: Vec<File>.
//         DocumentMetadata: Represents document metadata.
//             Fields: folder: PathBuf, labels: Vec<String>, related_documents: Vec<UUID>, extension_metadata: HashMap<String, String>.
//         File: Represents a file within a document.
//             Fields: path: PathBuf, file_type: FileType.
//         FileType: Enum representing file types (e.g., Markdown, PDF, JSON).
//     Metadata Storage:
//         Store metadata in YAML files (e.g., document_id.yaml).
//         Use a consistent schema for metadata, with versioning to handle schema evolution.
//         Use unique URIs as keys within the extension_metadata field to prevent conflicts between extensions.
//     File Organization:
//         Store all files related to a document in a dedicated directory within the workspace folder, mirroring the folder field of the document's metadata.
//         Use a naming convention to avoid file naming conflicts (e.g., appending numeric suffixes).
//     Embedding Storage:
//         Store embeddings in a separate, dedicated file within the .markhor directory in the workspace root.
//         Cache embeddings in memory for performance.
//     Workspace Root:
//         The CLI application will search for a workspace in the current directory and its parent directories, until it finds a .markhor directory.
//         The GUI application will remember previously opened workspaces.
//     Concurrency:
//         Use file locking or other concurrency control mechanisms to prevent data corruption. (Remaining issue to think about)
//     Error Handling:
//         Use Rust's Result type to handle file I/O errors and other storage-related issues.
//         Define a custom error type for storage operations. (Remaining issue to think about)
//     Operations:
//         create document (folder: PathBuf, files: Vec<PathBuf>, metadata: DocumentMetadata) -> Result<Document>.
//         add file to document (document_id: UUID, file: PathBuf) -> Result<()>.
//         remove file from document (document_id: UUID, file: PathBuf) -> Result<()>.
//         rename document (document_id: UUID, new_name: String) -> Result<()>.
//         move document (document_id: UUID, new_folder: PathBuf) -> Result<()>.
//         edit metadata (document_id: UUID, metadata: DocumentMetadata) -> Result<()>.
//         delete document (document_id: UUID) -> Result<()>.


use std::path::PathBuf;

pub struct Document {
    pub path: PathBuf,
    pub metadata: DocumentMetadata,
    // Add other document-related fields as needed
}

pub struct DocumentMetadata {
    pub title: String,
    pub tags: Vec<String>,
    // Add other metadata fields as needed
}