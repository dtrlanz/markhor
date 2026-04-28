mod document;
mod folder;
mod retriever;
mod scope;
mod tag;
mod workspace;

use std::{borrow::Borrow, ops::BitXor, path::PathBuf};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256, digest::{OutputSizeUser, generic_array::GenericArray}};
use thiserror::Error;

pub use document::{
    Document,
    DocumentMetadata,
    // TextLocation,
    MetadataLocation,
    chunks::{Chunk, ChunkMut},
};
pub(crate) use document::chunks::ChunkIdx;
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
    #[error("IO error: {0}")]
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
    MetadataFormat(#[from] serde_yaml_ng::Error),

    #[error("Invalid metadata location: {0:?} ({1})")]
    MetadataLocation(MetadataLocation, String),

    #[error("Invalid ID: {0}")]
    InvalidId(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct HashValue {
    value: GenericArray<u8, <Sha256 as OutputSizeUser>::OutputSize>,
}

impl BitXor for HashValue {
    type Output = Self;

    fn bitxor(self, rhs: Self) -> Self::Output {
        let value = self.value.iter().zip(rhs.value.iter())
            .map(|(a, b)| a ^ b)
            .collect::<Vec<u8>>();
        HashValue { value: GenericArray::from_slice(&value).clone() }
    }
}

impl<T: Borrow<str> + ?Sized> From<&T> for HashValue {
    fn from(value: &T) -> Self {
        let mut hasher = sha2::Sha256::new();
        hasher.update(value.borrow().as_bytes());
        HashValue { value: hasher.finalize() }
    }
}

// impl<'a> FromIterator<&'a str> for HashValue {
//     fn from_iter<I: IntoIterator<Item = &'a str>>(iter: I) -> Self {
//         let mut hasher = sha2::Sha256::new();
//         for value in iter {
//             hasher.update(value.as_bytes());
//         }
//         HashValue { value: hasher.finalize() }
//     }
// }

impl Serialize for HashValue {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error> where S: serde::Serializer {
        let hex_string = self.value.iter().map(|byte| format!("{:02x}", byte)).collect::<String>();
        serializer.serialize_str(&hex_string)
    }
}

impl<'de> Deserialize<'de> for HashValue {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error> where D: serde::Deserializer<'de> {
        let hex_string = String::deserialize(deserializer)?;
        let bytes = hex::decode(hex_string).map_err(serde::de::Error::custom)?;
        let value = GenericArray::from_slice(&bytes).clone();
        Ok(HashValue { value })
    }
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
        pub nodes: Vec<FsNode<'a>>,
        pub temp_dir: TempDir,
    }

    impl<'a> TempTree<'a> {
        /// Asynchronously creates a temp dir and materializes multiple top-level nodes.
        pub async fn new(nodes: Vec<FsNode<'a>>) -> io::Result<Self> {
            let temp_dir = tempfile::tempdir()?;
            
            // Materialize each top-level node directly into the temp_dir
            for node in &nodes {
                Self::materialize(temp_dir.path(), node).await?;
            }

            Ok(Self { nodes, temp_dir })
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

        /// Asynchronously merges a new virtual tree into the existing directory structure.
        /// Existing files will be overwritten, and new files will be added to existing directories.
        pub async fn add(&mut self, nodes: Vec<FsNode<'a>>) -> io::Result<()> {
            // 1. Materialize the new nodes to the disk
            for node in &nodes {
                Self::materialize(self.temp_dir.path(), node).await?;
            }

            // 2. Merge the new nodes into our in-memory structure to keep `iter()` accurate
            Self::merge_nodes(&mut self.nodes, nodes);

            Ok(())
        }

        /// Recursively deep-merges a list of new nodes into an existing list of nodes.
        fn merge_nodes(existing: &mut Vec<FsNode<'a>>, new_nodes: Vec<FsNode<'a>>) {
            for new_node in new_nodes {
                // Check if a node with the exact same name already exists at this level
                if let Some(existing_node) = existing.iter_mut().find(|n| n.name() == new_node.name()) {
                    match (existing_node, new_node) {
                        // If both are folders, recursively merge their children
                        (
                            FsNode::Folder { children: existing_children, .. },
                            FsNode::Folder { children: new_children, .. }
                        ) => {
                            Self::merge_nodes(existing_children, new_children);
                        }
                        // Otherwise, overwrite the existing node 
                        // (e.g., file overwrites file, or a file replaces a folder)
                        (e, n) => *e = n,
                    }
                } else {
                    // Node doesn't exist at this level, simply append it
                    existing.push(new_node);
                }
            }
        }        

        /// Returns an iterator that yields the absolute path and reference to every node in the tree.
        pub fn iter(&self) -> TempTreeIter<'a, '_> {
            let mut stack = Vec::new();
            
            // Push in reverse order so the first item declared in the macro is yielded first
            for node in self.nodes.iter().rev() {
                stack.push((self.temp_dir.path().join(node.name()), node));
            }
            
            TempTreeIter { stack }
        }

        /// Returns an iterator that yields the absolute path and reference to every node with the specified name.
        pub fn iter_by_name(&self, name: &str) -> impl Iterator<Item = (PathBuf, &FsNode<'a>)> {
            self.iter().filter(move |(_, node)| node.name() == name)
        }
    }

    impl<'a> Deref for TempTree<'a> {
        type Target = Path;

        /// Derefs to the root of the temporary directory.
        fn deref(&self) -> &Self::Target {
            self.temp_dir.path()
        }
    }

    impl<'a> AsRef<Path> for TempTree<'a> {
        fn as_ref(&self) -> &Path {
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
        // 1. Top-level rule: Matches a list of files/folders and returns a Vec<FsNode>
        ( $($name:expr => $content:tt),* $(,)? ) => {
            vec![
                $( fs_tree!(@node $name => $content) ),*
            ]
        };

        // --- Internal rules for recursion ---

        // 2. Match a nested folder (returns FsNode)
        (@node $name:expr => { $($child_name:expr => $child_content:tt),* $(,)? }) => {
            $crate::storage2::fs_test_utils::FsNode::folder(
                $name,
                vec![
                    $( fs_tree!(@node $child_name => $child_content) ),*
                ]
            )
        };

        // 3. Match a nested file (returns FsNode)
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

        let tree = TempTree::new(fs_tree! {
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

    #[tokio::test]
    async fn test_multiple_top_level_nodes() {
        // Some dynamic data to prove our `IntoNodeBytes` trait works beautifully
        let magic_bytes = vec![0xDE, 0xAD, 0xBE, 0xEF];

        // Spawn a tree with multiple root-level files and folders
        let tree = TempTree::new(fs_tree! {
            "README.md" => "# Top Level Readme\n",
            "src" => {
                "main.rs" => "fn main() {}",
                "utils.rs" => "pub fn do_stuff() {}",
            },
            "tests" => {
                "integration_test.rs" => "#[tokio::test]\nasync fn test_all() {}",
            },
            ".gitignore" => "/target\n.env\n",
            "data.bin" => { magic_bytes },
        })
        .await
        .unwrap();

        // 1. Verify top-level files exist directly under the temp dir
        assert!(tree.join("README.md").is_file());
        assert!(tree.join(".gitignore").is_file());
        assert!(tree.join("data.bin").is_file());

        // 2. Verify top-level folders exist
        assert!(tree.join("src").is_dir());
        assert!(tree.join("tests").is_dir());

        // 3. Verify nested files exist inside those folders
        assert!(tree.join("src/main.rs").is_file());
        assert!(tree.join("src/utils.rs").is_file());
        assert!(tree.join("tests/integration_test.rs").is_file());

        // 4. Read contents back asynchronously to ensure data was written correctly
        let readme_content = tokio::fs::read_to_string(tree.join("README.md")).await.unwrap();
        assert_eq!(readme_content, "# Top Level Readme\n");

        let bin_content = tokio::fs::read(tree.join("data.bin")).await.unwrap();
        assert_eq!(bin_content, vec![0xDE, 0xAD, 0xBE, 0xEF]);

        // 5. Verify the Iterator traverses all root nodes and their children
        // Nodes: README, src, main.rs, utils.rs, tests, integration_test.rs, .gitignore, data.bin (8 total)
        let node_count = tree.iter().count();
        assert_eq!(node_count, 8);

        // Ensure our iterator yields the actual absolute paths correctly
        let (utils_path, utils_node) = tree
            .iter()
            .find(|(_, node)| node.name() == "utils.rs")
            .expect("utils.rs should be in the iterator");
        
        assert_eq!(utils_node.name(), "utils.rs");
        assert!(utils_path.is_absolute());
        assert_eq!(
            tokio::fs::read_to_string(utils_path).await.unwrap(), 
            "pub fn do_stuff() {}"
        );
    }   

    #[tokio::test]
    async fn test_tree_merging() {
        // 1. Initial setup
        let mut tree = TempTree::new(fs_tree! {
            "src" => {
                "main.rs" => "fn main() { println!(\"v1\"); }",
            }
        }).await.unwrap();

        // Verify initial state
        let v1 = tokio::fs::read_to_string(tree.join("src/main.rs")).await.unwrap();
        assert_eq!(v1, "fn main() { println!(\"v1\"); }");

        // 2. Merge new files and modify existing ones
        tree.add(fs_tree! {
            "src" => {
                // This will overwrite the existing main.rs
                "main.rs" => "fn main() { println!(\"v2\"); }",
                // This will be added to the existing "src" folder
                "lib.rs" => "pub fn run() {}",
            },
            // This will be created at the root
            "README.md" => "# Updated App",
        }).await.unwrap();

        // 3. Verify the disk was updated properly
        let v2 = tokio::fs::read_to_string(tree.join("src/main.rs")).await.unwrap();
        assert_eq!(v2, "fn main() { println!(\"v2\"); }");
        assert!(tree.join("src/lib.rs").is_file());
        assert!(tree.join("README.md").is_file());

        // 4. Verify our in-memory structure was merged cleanly (no duplicate folders)
        // Nodes should be exactly: README.md, src, main.rs, lib.rs (4 items)
        assert_eq!(tree.iter().count(), 4);
    }    


}