use std::error::Error;

use markhor_extensions::embedding::gemini::{GeminiEmbedder, GeminiEmbedderOptions};
use markhor_core::embedding::{Embedder, EmbeddingUseCase, EmbeddingError};



#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Embedder, EmbeddingUseCase};
    use std::env;

    // Helper function to get API key or skip test
    fn get_api_key(test_name: &str) -> Option<String> {
        dotenv::dotenv().ok(); // Load .env file if present

        // Check for GOOGLE_API_KEY in environment variables
        match env::var("GOOGLE_API_KEY") {
            Ok(key) if !key.is_empty() => Some(key),
            _ => {
                println!("Skipping integration test {} - GOOGLE_API_KEY environment variable not set.", test_name);
                None // Signal to skip
            }
        }
    }

    #[tokio::test]
    async fn integration_test_gemini_embed_success() {
        let test_name = "integration_test_gemini_embed_success";
        let api_key = match get_api_key(test_name) {
            Some(key) => key,
            None => return, // Skip test
        };

        // Use a known, generally available model
        let model_name = "embedding-001";
        let options = GeminiEmbedderOptions::default();

        let embedder = match GeminiEmbedder::new(api_key, model_name, options) {
             Ok(e) => e,
             Err(e) => panic!("{}: Failed to create GeminiEmbedder: {}", test_name, e),
        };

        let texts = ["Hello Gemini!", "This is an integration test."];

        let result = embedder.embed(&texts).await;

        match result {
            Ok(embeddings) => {
                assert_eq!(embeddings.len(), texts.len(), "{}: Should return same number of embeddings as inputs", test_name);
                let expected_dims = embedder.dimensions();
                assert!(expected_dims.is_some(), "{}: Dimensions should be known for {}", test_name, model_name);
                let expected_dims = expected_dims.unwrap(); // Safe after check

                for (i, embedding) in embeddings.iter().enumerate() {
                    assert_eq!(embedding.0.len(), expected_dims, "{}: Embedding {} should have correct dimensions", test_name, i);
                    // Basic sanity check on values (not all zero)
                    assert!(embedding.0.iter().any(|&v| v != 0.0), "{}: Embedding {} should not be all zeros", test_name, i);
                }
                assert_eq!(embedder.intended_use_case(), EmbeddingUseCase::General, "{}: Default use case should be General", test_name);
            }
            Err(e) => {
                panic!("{}: Embedding failed unexpectedly: {}", test_name, e);
            }
        }
    }

    #[tokio::test]
    async fn integration_test_gemini_embed_with_task_type() {
        let test_name = "integration_test_gemini_embed_with_task_type";
         let api_key = match get_api_key(test_name) {
            Some(key) => key,
            None => return, // Skip test
        };

        let model_name = "embedding-001";
        let task_type = "RETRIEVAL_DOCUMENT".to_string();
        let options = GeminiEmbedderOptions {
            task_type: Some(task_type.clone()),
            ..Default::default()
        };

        let embedder = match GeminiEmbedder::new(api_key, model_name, options) {
             Ok(e) => e,
             Err(e) => panic!("{}: Failed to create GeminiEmbedder: {}", test_name, e),
        };

        assert_eq!(embedder.intended_use_case(), EmbeddingUseCase::RetrievalDocument, "{}: Use case should be mapped correctly", test_name);

        let texts = ["This document is intended for retrieval."];
        let result = embedder.embed(&texts).await;

         match result {
            Ok(embeddings) => {
                assert_eq!(embeddings.len(), texts.len(), "{}: Should return embedding for the input", test_name);
                let expected_dims = embedder.dimensions().unwrap_or(0);
                assert_ne!(expected_dims, 0, "{}: Dimensions should be > 0", test_name);
                assert_eq!(embeddings[0].0.len(), expected_dims, "{}: Embedding should have correct dimensions", test_name);
            }
            Err(e) => {
                // Note: Some task_types might eventually be invalid for certain models,
                // so this could potentially error, but RETRIEVAL_DOCUMENT is generally safe.
                panic!("{}: Embedding failed unexpectedly with task_type '{}': {}", test_name, task_type, e);
            }
        }
    }

    #[tokio::test]
    async fn integration_test_gemini_embed_invalid_api_key() {
        let test_name = "integration_test_gemini_embed_invalid_api_key";
        // Not needed for the test, we are testing *with* an invalid key. But we're using the 
        // environment variable to toggle integration tests that make API calls.
        let api_key = match get_api_key(test_name) {
            Some(key) => key,
            None => return, // Skip test
        };

        let invalid_api_key = "THIS_IS_NOT_A_VALID_API_KEY";
        let model_name = "embedding-001"; // Use a real model name
        let options = GeminiEmbedderOptions::default();

        // Creation should succeed, the key isn't validated until the API call
        let embedder = match GeminiEmbedder::new(invalid_api_key, model_name, options) {
            Ok(e) => e,
            Err(e) => panic!("{}: Failed to create GeminiEmbedder (should succeed): {}", test_name, e),
        };

        let texts = ["Testing with an invalid key."];
        let result = embedder.embed(&texts).await;

        assert!(result.is_err(), "{}: Embedding should fail with invalid API key", test_name);

        // Check if the error is an ApiError (most likely)
        // The exact status code might vary (e.g., 400, 403, 401), but it should be an API error.
        assert!(
            matches!(result.err().unwrap(), EmbeddingError::ApiError { .. }),
            "{}: Error should be an ApiError variant", test_name
        );
    }

     #[tokio::test]
    async fn integration_test_gemini_embed_invalid_model_name() {
        let test_name = "integration_test_gemini_embed_invalid_model_name";
        let api_key = match get_api_key(test_name) {
            Some(key) => key,
            None => return, // Skip test
        };

        let invalid_model_name = "non-existent-embedding-model-foobar";
        let options = GeminiEmbedderOptions::default();

        let embedder = match GeminiEmbedder::new(api_key, invalid_model_name, options) {
             Ok(e) => e,
             Err(e) => panic!("{}: Failed to create GeminiEmbedder: {}", test_name, e),
        };

        let texts = ["Testing with an invalid model."];
        let result = embedder.embed(&texts).await;

        assert!(result.is_err(), "{}: Embedding should fail with invalid model name", test_name);

        // Expect an API error, likely a 404 or similar invalid argument status.
        let err = result.err().unwrap();
        assert!(
            matches!(err, EmbeddingError::ApiError { .. }),
            "{}: Error should be an ApiError variant, got {:?}", test_name, err
        );

        // Optional: Check if status code seems like 'not found' or 'invalid argument' if needed
        if let EmbeddingError::ApiError { status_code, message, .. } = err {
             println!("{}: Received ApiError status: {:?}, message: {}", test_name, status_code, message);
             // Example check: assert!(status_code == Some(404) || status_code == Some(400));
        }
    }

     #[tokio::test]
     async fn integration_test_gemini_batch_limit_error() {
        let test_name = "integration_test_gemini_batch_limit_error";
        let api_key = match get_api_key(test_name) {
            Some(key) => key,
            None => return, // Skip test
        };

        let model_name = "embedding-001";
        let options = GeminiEmbedderOptions::default();

        let embedder = match GeminiEmbedder::new(api_key, model_name, options) {
             Ok(e) => e,
             Err(e) => panic!("{}: Failed to create GeminiEmbedder: {}", test_name, e),
        };

        // Create a batch larger than the expected limit (100 for embedding-001)
        let large_batch: Vec<&str> = (0..101).map(|_| "text").collect();

        let result = embedder.embed(&large_batch).await;

        assert!(result.is_err(), "{}: Embedding should fail when batch size exceeds limit", test_name);

        let err = result.err().unwrap();
        assert!(
            matches!(err, EmbeddingError::BatchTooLarge { limit: Some(100), actual: 101 }),
            "{}: Error should be BatchTooLarge with correct details, got {:?}", test_name, err
        );
     }

    // Add more tests for edge cases if needed (e.g., empty input text strings,
    // very long strings - though the API handles truncation/errors for those)
}