mod document;
mod folder;
mod retriever;
mod scope;
mod tag;
mod workspace;

use std::path::PathBuf;
use thiserror::Error;

pub use document::{
    Document,
    DocumentMetadata,
    TextLocation,
    MetadataLocation,
    Chunk,
    ChunkMut,
    TextHash,
};
pub(crate) use document::{ChunkIdx};
pub use folder::Folder;
pub use retriever::Retriever;
pub use scope::{PrelimFilter, Scope};
pub use tag::Tag;
pub use workspace::Workspace;


const METADATA_EXTENSION: &str = "mark";
const ATTACHMENTS_DIR: &str = "attachments";
const WORKSPACE_CONFIG_DIR: &str = ".markhor";
const WORKSPACE_METADATA_FILENAME: &str = "workspace.yaml";


#[derive(Debug, Error)]
pub enum AccessStorageError {
    #[error("IO error")]
    Io(#[from] std::io::Error),

    #[error("File not found: {0}")]
    FileNotFound(PathBuf),

    #[error("Directory not found: {0}")]
    DirectoryNotFound(PathBuf),

    #[error("Path is not a directory: {0}")]
    NotADirectory(PathBuf),

    #[error("Path is outside of workspace: {0}")]
    NotInWorkspace(PathBuf),

    #[error("Workspaces may not be nested. Outer workspace: {0}")]
    InWorkspace(PathBuf),

    #[error("Metadata serialization/deserialization error")]
    Metadata(#[from] serde_yaml_ng::Error),

    #[error("Invalid ID: {0}")]
    InvalidId(String),
}

#[cfg(test)]
pub(crate) mod fs_test_utils {
    use std::borrow::Cow;
    use std::future::Future;
    use std::io;
    use std::ops::Deref;
    use std::path::{Path, PathBuf};
    use std::pin::Pin;
    use tempfile::TempDir;
    use tokio::fs;

    /// Represents a node in our virtual filesystem tree.
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub enum FsNode<'a> {
        File {
            name: Cow<'a, str>,
            contents: Cow<'a, [u8]>,
        },
        Folder {
            name: Cow<'a, str>,
            children: Vec<FsNode<'a>>,
        },
    }

    impl<'a> FsNode<'a> {
        /// Convenience constructor for a file
        pub fn file<N, C>(name: N, contents: C) -> Self
        where
            N: Into<Cow<'a, str>>,
            C: IntoNodeBytes<'a>, // <-- Use our new trait here
        {
            FsNode::File {
                name: name.into(),
                contents: contents.into_node_bytes(), // <-- Call our trait method
            }
        }

        /// Convenience constructor for a folder
        pub fn folder<N>(name: N, children: Vec<FsNode<'a>>) -> Self
        where
            N: Into<Cow<'a, str>>,
        {
            FsNode::Folder {
                name: name.into(),
                children,
            }
        }

        /// Returns the name of the file or folder
        pub fn name(&self) -> &str {
            match self {
                FsNode::File { name, .. } => name.as_ref(),
                FsNode::Folder { name, .. } => name.as_ref(),
            }
        }        
    }


    /// Represents a materialized virtual filesystem in a temporary directory.
    pub struct TempTree<'a> {
        pub root: FsNode<'a>,
        pub temp_dir: TempDir,
    }

    impl<'a> TempTree<'a> {
        /// Asynchronously creates a temporary directory and materializes the `FsNode` inside it.
        pub async fn spawn(root: FsNode<'a>) -> io::Result<Self> {
            // Synchronous but fast; perfectly fine for async test setup
            let temp_dir = tempfile::tempdir()?;
            
            Self::materialize(temp_dir.path(), &root).await?;

            Ok(Self { root, temp_dir })
        }

        /// Recursively walks the node and writes files/folders to the disk asynchronously.
        /// Returns a Boxed Future to satisfy Rust's rules around async recursion.
        fn materialize<'b>(
            base_path: &'b Path,
            node: &'b FsNode<'a>,
        ) -> Pin<Box<dyn Future<Output = io::Result<()>> + 'b>> {
            Box::pin(async move {
                match node {
                    FsNode::File { name, contents } => {
                        let file_path = base_path.join(name.as_ref());
                        fs::write(file_path, contents.as_ref()).await?;
                    }
                    FsNode::Folder { name, children } => {
                        let folder_path = base_path.join(name.as_ref());
                        fs::create_dir_all(&folder_path).await?;
                        for child in children {
                            // Await the boxed recursive call
                            Self::materialize(&folder_path, child).await?;
                        }
                    }
                }
                Ok(())
            })
        }

        /// Returns an iterator that yields the absolute path and reference to every node in the tree.
        pub fn iter(&self) -> TempTreeIter<'a, '_> {
            let root_path = self.temp_dir.path().join(self.root.name());
            TempTreeIter {
                stack: vec![(root_path, &self.root)],
            }
        }        
    }

    impl<'a> Deref for TempTree<'a> {
        type Target = Path;

        /// Derefs to the root of the temporary directory.
        fn deref(&self) -> &Self::Target {
            self.temp_dir.path()
        }
    }

    pub struct TempTreeIter<'a, 'b> {
        // A stack holding the absolute path and the node reference
        stack: Vec<(PathBuf, &'b FsNode<'a>)>,
    }

    impl<'a, 'b> Iterator for TempTreeIter<'a, 'b> {
        type Item = (PathBuf, &'b FsNode<'a>);

        fn next(&mut self) -> Option<Self::Item> {
            let (current_path, node) = self.stack.pop()?;

            if let FsNode::Folder { children, .. } = node {
                // Push children in reverse order so they are yielded in the order they were defined
                for child in children.iter().rev() {
                    self.stack.push((current_path.join(child.name()), child));
                }
            }

            Some((current_path, node))
        }
    }

    /// Helper trait to allow passing both Strings and Bytes seamlessly into FsNode::file
    pub trait IntoNodeBytes<'a> {
        fn into_node_bytes(self) -> Cow<'a, [u8]>;
    }

    impl<'a> IntoNodeBytes<'a> for &'a str {
        fn into_node_bytes(self) -> Cow<'a, [u8]> {
            Cow::Borrowed(self.as_bytes())
        }
    }

    impl<'a> IntoNodeBytes<'a> for String {
        fn into_node_bytes(self) -> Cow<'a, [u8]> {
            Cow::Owned(self.into_bytes())
        }
    }

    impl<'a> IntoNodeBytes<'a> for &'a [u8] {
        fn into_node_bytes(self) -> Cow<'a, [u8]> {
            Cow::Borrowed(self)
        }
    }

    impl<'a> IntoNodeBytes<'a> for Vec<u8> {
        fn into_node_bytes(self) -> Cow<'a, [u8]> {
            Cow::Owned(self)
        }
    }

    impl<'a, const N: usize> IntoNodeBytes<'a> for &'a [u8; N] {
        fn into_node_bytes(self) -> Cow<'a, [u8]> {
            Cow::Borrowed(self.as_slice())
        }
    }    

    #[macro_export]
    macro_rules! fs_tree {
        // 1. Match a folder (a block with key-value pairs separated by commas)
        ( $name:expr => { $($child_name:expr => $child_content:tt),* $(,)? } ) => {
            $crate::storage2::fs_test_utils::FsNode::folder(
                $name,
                vec![
                    $( fs_tree!(@node $child_name => $child_content) ),*
                ]
            )
        };

        // 2. Match a single file (root level)
        ( $name:expr => $contents:expr ) => {
            $crate::storage2::fs_test_utils::FsNode::file($name, $contents)
        };

        // --- Internal rules for recursion ---

        // 3. Match a nested folder
        (@node $name:expr => { $($child_name:expr => $child_content:tt),* $(,)? }) => {
            $crate::storage2::fs_test_utils::FsNode::folder(
                $name,
                vec![
                    $( fs_tree!(@node $child_name => $child_content) ),*
                ]
            )
        };

        // 4. Match a nested file
        (@node $name:expr => $contents:expr) => {
            $crate::storage2::fs_test_utils::FsNode::file($name, $contents)
        };
    }
    pub use fs_tree;
}

#[cfg(test)]
mod tests {
    use super::fs_test_utils::*;

    #[tokio::test]
    async fn test_macro_structure() {
        // Some dynamically generated content
        let dynamic_config = format!(r#"{{ "port": {} }}"#, 8080);

        let tree = TempTree::spawn(fs_tree! {
            "my_app" => {
                "Cargo.toml" => "[package]\nname=\"my_app\"",
                
                // Nested folder
                "src" => {
                    "main.rs" => "fn main() { println!(\"Hello\"); }",
                    "lib.rs" => "pub fn run() {}",
                },
                
                // Empty folder
                "assets" => {},
                
                // For complex expressions (function calls, macros, etc),
                // wrap the contents in `{ }`
                "config.json" => { dynamic_config },
                "data.bin" => { vec![0, 1, 2, 3] },
            }
        }).await.unwrap();

        // Verify it works!
        assert!(tree.join("my_app/src/main.rs").exists());
        assert!(tree.join("my_app/assets").is_dir());
        
        let config = tokio::fs::read_to_string(tree.join("my_app/config.json")).await.unwrap();
        assert_eq!(config, r#"{ "port": 8080 }"#);
    }

}