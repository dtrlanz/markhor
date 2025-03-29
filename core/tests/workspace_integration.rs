use tempfile::tempdir;
use tokio::fs;
use std::path::Path;

// Replace 'your_library_name' with the actual name of your crate
use markhor_core::storage::{
    Workspace,
    Document,
    Folder,
    Error,
    ConflictError,
    MARKHOR_EXTENSION,
    INTERNAL_DIR_NAME,
};

// Helper to create dummy file/dir - reusing from unit tests basically
async fn create_dummy(path: &Path, is_dir: bool) {
    if is_dir {
        fs::create_dir_all(path).await.expect("Test helper: Failed to create dummy dir");
    } else {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).await.expect("Test helper: Failed to create parent dir");
        }
        fs::write(path, "").await.expect("Test helper: Failed to create dummy file");
    }
}

#[tokio::test]
async fn integration_create_and_open_workspace() {
    let dir = tempdir().unwrap();
    let ws_path = dir.path().join("my_integration_ws");

    // 1. Create workspace
    let created_ws = Workspace::create(ws_path.clone()).await.expect("Failed to create workspace");
    assert_eq!(created_ws.path(), ws_path.as_path());
    assert!(created_ws.path().join(INTERNAL_DIR_NAME).exists(), "Internal .markhor directory should exist after create");
    assert!(created_ws.path().join(INTERNAL_DIR_NAME).is_dir(), "Internal .markhor should be a directory");

    // 2. Open the created workspace
    let opened_ws = Workspace::open(ws_path.clone()).await.expect("Failed to open existing workspace");
    assert_eq!(opened_ws.path(), ws_path.as_path());
    assert!(opened_ws.path().join(INTERNAL_DIR_NAME).exists(), "Internal .markhor directory should exist after open");

    // 3. Try opening a non-existent path
    let non_existent_path = dir.path().join("non_existent_ws");
    let open_err = Workspace::open(non_existent_path).await;
    assert!(matches!(open_err, Err(Error::DirectoryNotFound(_))), "Opening non-existent path should fail");

    // 4. Try opening a path that isn't a workspace
    let not_a_ws_path = dir.path().join("not_a_ws");
    create_dummy(&not_a_ws_path, true).await; // Just a dir, no .markhor subdir
    let open_err_2 = Workspace::open(not_a_ws_path).await;
    assert!(matches!(open_err_2, Err(Error::NotAWorkspace(_))), "Opening dir without .markhor should fail");
}

// todo: Uncomment and implement the test below when DocumentMetadata is public or has getters/setters

// #[tokio::test]
// async fn integration_workspace_document_lifecycle() {
//     let dir = tempdir().unwrap();
//     let ws = Workspace::create(dir.path().to_path_buf()).await.unwrap();
//     let doc_path = ws.path().join("test_doc.markhor");
//     let associated_file_path = ws.path().join("test_doc.txt");

//     // 1. Create document
//     let doc = Document::create(doc_path.clone()).await.expect("Failed to create document");
//     assert!(doc_path.exists(), "Document .markhor file should be created");

//     // 2. Add an associated file manually (simulating external process)
//     create_dummy(&associated_file_path, false).await;
//     assert!(associated_file_path.exists());

//     // 3. List documents in workspace
//     let docs_in_ws = ws.list_documents().await.unwrap();
//     assert_eq!(docs_in_ws.len(), 1, "Workspace should list the created document");
//     assert_eq!(docs_in_ws[0].path(), doc_path, "Listed document path mismatch"); // Assumes pub field or getter

//     // 4. List files within the document
//     // Re-open the document instance from the list (or use the original 'doc')
//     let listed_doc = &docs_in_ws[0];
//     let files_in_doc = listed_doc.files().await.unwrap();
//     assert_eq!(files_in_doc.len(), 1, "Document should list the associated file");
//     assert_eq!(files_in_doc[0].path(), associated_file_path, "Listed file path mismatch");

//     // 5. Read/Write Metadata (requires pub DocumentMetadata or getter/setter)
//     let mut metadata = listed_doc.read_metadata().await.unwrap();
//     let original_id = metadata.id; // Assuming pub field 'id' on DocumentMetadata
//     metadata.id = uuid::Uuid::new_v4(); // Change something (requires pub field or method)
//     listed_doc.save_metadata(&metadata).await.unwrap();

//     // Re-open and verify metadata change
//     let reopened_doc = Document::open(doc_path.clone()).await.unwrap();
//     let updated_metadata = reopened_doc.read_metadata().await.unwrap();
//     assert_ne!(original_id, updated_metadata.id, "Metadata ID should have been updated");
//     assert_eq!(metadata.id, updated_metadata.id, "Saved metadata ID should match re-read ID");


//     // 6. Delete document
//     reopened_doc.delete().await.expect("Failed to delete document");
//     assert!(!doc_path.exists(), "Document .markhor file should be deleted");
//     assert!(!associated_file_path.exists(), "Associated .txt file should be deleted");

//     // 7. List documents again
//     let docs_after_delete = ws.list_documents().await.unwrap();
//     assert!(docs_after_delete.is_empty(), "Workspace should be empty after delete");
// }

#[tokio::test]
async fn integration_folders_and_nested_docs() {
    let dir = tempdir().unwrap();
    let ws = Workspace::create(dir.path().to_path_buf()).await.unwrap();
    let folder1_path = ws.path().join("FolderA");
    let folder2_path = folder1_path.join("FolderB");
    let doc1_path = folder1_path.join("doc_in_a.markhor");
    let doc2_path = folder2_path.join("doc_in_b.markhor");

    // 1. Create folder structure
    create_dummy(&folder1_path, true).await;
    create_dummy(&folder2_path, true).await;

    // 2. List folders in workspace
    let root_folders = ws.list_folders().await.unwrap();
    assert_eq!(root_folders.len(), 1);
    assert_eq!(root_folders[0].path(), folder1_path);

    // 3. List contents of FolderA
    // Get the Folder struct instance first
    let folder_a = ws.list_folders().await.unwrap().into_iter()
        .find(|f| f.path() == folder1_path)
        .expect("Could not find FolderA");

    let docs_in_a = folder_a.list_documents().await.unwrap();
    assert_eq!(docs_in_a.len(), 0);
            
    // Create document in folder
    let _doc1 = folder_a.create_document("doc_in_a").await.unwrap();

    let docs_in_a = folder_a.list_documents().await.unwrap();
    assert_eq!(docs_in_a.len(), 1);
    assert_eq!(docs_in_a[0].path(), doc1_path);

    let folders_in_a = folder_a.list_folders().await.unwrap();
    assert_eq!(folders_in_a.len(), 1);
    assert_eq!(folders_in_a[0].path(), folder2_path);

    // 4. List contents of FolderB
    let folder_b = folders_in_a.into_iter().next().expect("Could not find FolderB");

    let docs_in_b = folder_b.list_documents().await.unwrap();
    assert_eq!(docs_in_b.len(), 0);

    // Create document in folder
    let _doc2 = folder_b.create_document("doc_in_b").await.unwrap();

    let docs_in_b = folder_b.list_documents().await.unwrap();
    assert_eq!(docs_in_b.len(), 1);
    assert_eq!(docs_in_b[0].path(), doc2_path);

    let folders_in_b = folder_b.list_folders().await.unwrap();
    assert!(folders_in_b.is_empty());
}


#[tokio::test]
async fn integration_move_document_between_folders() {
    let dir = tempdir().unwrap();
    let ws = Workspace::create(dir.path().to_path_buf()).await.unwrap();
    let folder_a = ws.create_subfolder("DirA").await.unwrap();
    let folder_b = ws.create_subfolder("DirB").await.unwrap();
    let original_doc_path = folder_a.path().join("movable.markhor");
    let original_file_path = folder_a.path().join("movable.data");
    let target_doc_path = folder_b.path().join("moved_doc.markhor"); // Moving and renaming

    // 1. Setup initial state
    create_dummy(&folder_a.path(), true).await;
    create_dummy(&folder_b.path(), true).await;
    let doc = folder_a.create_document("movable").await.unwrap();
    create_dummy(&original_file_path, false).await; // Associated file

    assert!(original_doc_path.exists());
    assert!(original_file_path.exists());
    assert!(!target_doc_path.exists());
    assert!(!folder_b.path().join("moved_doc.data").exists());


    // 2. Perform the move
    let moved_doc = doc.move_to(target_doc_path.clone()).await.unwrap();

    // 3. Verify final state
    assert!(!original_doc_path.exists(), "Original .markhor should be gone");
    assert!(!original_file_path.exists(), "Original .data file should be gone");
    assert!(target_doc_path.exists(), "Target .markhor should exist");
    assert!(folder_b.path().join("moved_doc.data").exists(), "Target .data file should exist");
    assert_eq!(moved_doc.path(), target_doc_path, "Moved document internal path should be updated");

    // 4. Verify listing in new location
    let folders = ws.list_folders().await.unwrap();
    let folder_b = folders.iter().find(|f| f.path() == folder_b.path()).unwrap();
    let docs_in_b = folder_b.list_documents().await.unwrap();
    assert_eq!(docs_in_b.len(), 1);
    assert_eq!(docs_in_b[0].path(), target_doc_path);

    // 5. Verify listing in old location
    let folder_a = folders.iter().find(|f| f.path() == folder_a.path()).unwrap();
    let docs_in_a = folder_a.list_documents().await.unwrap();
    assert!(docs_in_a.is_empty());
}

#[tokio::test]
async fn integration_move_document_causes_conflict() {
     let dir = tempdir().unwrap();
    let ws = Workspace::create(dir.path().to_path_buf()).await.unwrap();

    let doc1_path = ws.path().join("doc1.markhor");
    let doc2_path = ws.path().join("doc2.markhor");
    let conflicting_file_for_doc1 = ws.path().join("doc1.txt");

    // 1. Setup: doc1, doc2, and a file potentially belonging to doc1
    let doc1 = ws.create_document("doc1").await.unwrap();
    let doc2 = ws.create_document("doc2").await.unwrap();
    create_dummy(&conflicting_file_for_doc1, false).await;

    // 2. Attempt to move doc2 to doc1's name - should conflict (MarkhorFileExists)
    let move_result = doc2.move_to(doc1_path.clone()).await;
    assert!(matches!(move_result, Err(Error::Conflict(ConflictError::MarkhorFileExists(p))) if p == doc1_path));

    // 3. Attempt to move doc1 to doc2's name - should conflict (MarkhorFileExists)
    // Setup: doc1, file named doc2.txt. Try moving doc1 -> doc2.markhor
    let conflicting_file_for_doc2 = ws.path().join("doc2.txt");
    create_dummy(&conflicting_file_for_doc2, false).await;

    let move_result_2 = doc1.move_to(doc2_path.clone()).await;
    assert!(matches!(move_result_2, Err(Error::Conflict(ConflictError::MarkhorFileExists(p))) if p == doc2_path));


}

#[tokio::test]
async fn integration_move_adopts_orphan_conflict() {
    let dir = tempdir().unwrap();
    let ws = Workspace::create(dir.path().to_path_buf()).await.unwrap();

    let source_doc_path = ws.path().join("source.markhor");
    let target_doc_path = ws.path().join("target.markhor"); // Target does NOT exist initially
    let orphan_file_path = ws.path().join("target.txt");   // File that WOULD belong to target

    // 1. Setup: Create source doc and the "orphan" file
    let source_doc = ws.create_document("source").await.unwrap();
    create_dummy(&orphan_file_path, false).await;

    assert!(source_doc_path.exists());
    assert!(!target_doc_path.exists()); // Ensure target .markhor does not exist
    assert!(orphan_file_path.exists());

    // 2. Attempt to move source.markhor -> target.markhor
    // This should fail because target.txt exists and would be adopted.
    let move_result = source_doc.move_to(target_doc_path.clone()).await;

    // 3. Verify the specific conflict
    let err_msg = format!(
        "Move should fail with ExistingFileWouldBeAdopted, but got: {:?}",
        move_result
    );
    assert!(
        matches!(move_result, Err(Error::Conflict(ConflictError::ExistingFileWouldBeAdopted(p))) if p == orphan_file_path),
        "{}", err_msg
    );

    // 4. Verify filesystem state hasn't changed (move didn't proceed)
    assert!(source_doc_path.exists(), "Source document should still exist after failed move");
    assert!(!target_doc_path.exists(), "Target document should not have been created");
    assert!(orphan_file_path.exists(), "Orphan file should still exist");
}

#[tokio::test]
async fn integration_create_document_causes_conflict() {
    let dir = tempdir().unwrap();
    let ws = Workspace::create(dir.path().to_path_buf()).await.unwrap();

    let base_doc_name = "base";
    let suffix_doc_name = "base.a1";
    let conflicting_file_path = ws.path().join("base.txt");

    // 1. Create base.txt - this conflicts with creating base.markhor (Rule 2)
    create_dummy(&conflicting_file_path, false).await;
    let create_result_1 = ws.create_document(base_doc_name).await;
    assert!(matches!(create_result_1, Err(Error::Conflict(ConflictError::ExistingFileWouldBeAdopted(p))) if p == conflicting_file_path));
    fs::remove_file(&conflicting_file_path).await.unwrap(); // Clean up for next test


    // 2. Create base.markhor successfully now
    let base_doc = ws.create_document(base_doc_name).await.unwrap();

    // 3. Attempt to create base.a1.markhor - conflicts with existing base.markhor (Rule 3)
    let create_result_2 = ws.create_document(suffix_doc_name).await;
     assert!(matches!(create_result_2, Err(Error::Conflict(ConflictError::SuffixBaseAmbiguity(b, s))) if b == "base" && s == "a1"));

    // 4. Delete base, create suffix, try creating base (Rule 4)
     base_doc.delete().await.unwrap();
     let _suffix_doc = ws.create_document(suffix_doc_name).await;
     let create_result_3 = ws.create_document(base_doc_name).await;
     assert!(matches!(create_result_3, Err(Error::Conflict(ConflictError::BaseSuffixAmbiguity(b, s))) if b == "base" && s == "base.a1"));

}

// Add more tests as needed:
// - Deleting folders (if implemented)
// - Moving folders (if implemented)
// - Specific edge cases for naming and hex suffixes
// - Interactions with workspace configuration (when added)