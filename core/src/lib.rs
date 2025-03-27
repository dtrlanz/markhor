
mod extensions;


pub mod document;


// pub mod error {
//     use thiserror::Error;
    

//     #[derive(Error, Debug)]
//     pub enum MarkhorError {
//         #[error("Document Error: {0}")]
//         DocumentError(String),
//         #[error("Model Error: {0}")]
//         ModelError(String),
//         #[error("Plugin Error: {0}")]
//         PluginError(String),
//         #[error("Workspace Error: {0}")]
//         WorkspaceError(String),
//         #[error("Event Error: {0}")]
//         EventError(String),
//         #[error("IO Error: {0}")]
//         IoError(#[from] std::io::Error),
//         #[error("Other Error: {0}")]
//         OtherError(String),
//     }

//     pub type Result<T> = std::result::Result<T, MarkhorError>;
// }


pub fn add(left: u64, right: u64) -> u64 {
    left + right
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn it_works() {
        let result = add(2, 2);
        assert_eq!(result, 4);
    }
}
