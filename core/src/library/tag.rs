use std::sync::Arc;


#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Tag {
    name: Arc<str>,
}