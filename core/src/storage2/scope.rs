use crate::storage2::Document;



pub struct Scope {

}

/// A preliminary filter that can be applied to a scope to quickly determine if it may include 
/// a specific document.
/// 
/// This is used to avoid expensive operations when we can determine that a document is 
/// definitely not included in the scope. The filter may return false positives (indicating that 
/// a document may be included when it is not), but is guaranteed not to return false negatives 
/// (indicating that a document is not included when it is).
/// 
/// # To do
/// 
/// This is a placeholder implementation that always returns true. Actual implementation should be
/// fairly simple, probably involving pigeonholes, bitwise operations, and nice things like that.
#[derive(Debug, Clone)]
pub struct PrelimFilter {
    // TODO
}

impl PrelimFilter {
    /// Returns true if the document from which this filter was created may be included in the 
    /// given scope, false if it is definitely not.
    pub fn maybe_matches(&self, scope: &Scope) -> bool {
        // TODO
        true
    }
}

impl From<&Document> for PrelimFilter {
    fn from(document: &Document) -> Self {
        // TODO
        PrelimFilter {}
    }
}