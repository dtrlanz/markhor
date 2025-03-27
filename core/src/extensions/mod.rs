
pub mod models {
    pub trait ChatModel {
        fn generate(&self, prompt: &str) -> Result<String, String>;
        // Add other model-related methods as needed
    }

    pub trait EmbeddingModel {
        fn embed(&self, text: &str) -> Result<Vec<f32>, String>;
        // Add other embedding-related methods as needed
    }
}

pub mod plugins {
    // Define plugin-related structures and traits here later
}

