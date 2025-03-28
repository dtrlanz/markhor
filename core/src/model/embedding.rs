pub trait EmbeddingModel {
    fn embed(&self, text: &str) -> Result<Vec<f32>, String>;
    // Add other embedding-related methods as needed
}
