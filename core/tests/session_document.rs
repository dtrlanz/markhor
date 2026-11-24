use std::fs;

use markhor_core::dependencies::{AccessDocumentError, AddDocumentError, Session};
use markhor_core::library::{AccessLibraryError, Folder, NewSource};
use markhor_core::permissions::{PUBLIC, Restricted};

/// Creates a collection containing an unrestricted `child/doc.md` and a restricted
/// `restricted.md`. Documents without metadata get the collection default (`ON_DEVICE`).
async fn setup() -> (tempfile::TempDir, Folder) {
    let dir = tempfile::tempdir().unwrap();
    fs::create_dir(dir.path().join("child")).unwrap();
    // Inherits the collection default (ON_DEVICE) because it has no metadata.
    fs::write(dir.path().join("restricted.md"), "secret").unwrap();

    let root = Folder::open(dir.path()).await.unwrap();
    let child = root.folder("child").await.unwrap();
    child
        .create_document("doc.md", NewSource::Bytes(b"nested"), Some(&[]))
        .await
        .unwrap();
    (dir, root)
}

#[tokio::test]
async fn accepts_absolute_path_inside_collection() {
    let (dir, root) = setup().await;
    let mut session = Session::with_folder(root);

    let doc = session
        .document(dir.path().join("child").join("doc.md"))
        .await
        .unwrap();
    assert_eq!(doc.path(), "child/doc.md");
}

#[tokio::test]
async fn returns_cached_document_for_equivalent_paths() {
    let (_dir, root) = setup().await;
    let mut session = Session::with_folder(root);

    let first = session.document("child/doc.md").await.unwrap() as *const _;
    let second = session.document("./child/../child/doc.md").await.unwrap() as *const _;
    assert!(std::ptr::eq(first, second));
}

#[tokio::test]
async fn reports_missing_file() {
    let (_dir, root) = setup().await;
    let mut session = Session::with_folder(root);

    let err = session.document("missing.md").await.unwrap_err();
    assert!(matches!(
        err,
        AccessDocumentError::Library(AccessLibraryError::FileNotFound(_))
    ));
}

// Intended usage (see the comment on `Session::restrict_access`): elevate the session's
// requirements so that a document with matching permissions can be added.
#[tokio::test]
async fn restrict_access_allows_adding_restricted_document() {
    let (_dir, root) = setup().await;
    let mut session = Session::with_folder(root);
    session
        .restrict_access(vec![markhor_core::permissions::ON_DEVICE])
        .unwrap();

    let doc = session.document("restricted.md").await.unwrap();
    assert_eq!(doc.path(), "restricted.md");
    assert!(!doc.permissions_required().is_empty());
}

// Requirements that don't cover the document's permissions still reject it.
#[tokio::test]
async fn restrict_access_rejects_document_beyond_session_requirements() {
    let (_dir, root) = setup().await;
    let mut session = Session::with_folder(root);
    session.restrict_access(vec![PUBLIC]).unwrap();

    let err = session.document("restricted.md").await.unwrap_err();
    assert!(matches!(
        err,
        AccessDocumentError::Permission(AddDocumentError::ElevationRequired(_))
    ));
}
