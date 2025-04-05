use std::env;

// Helper function to get API key
pub fn get_api_key(var_name: &str) -> String {
    dotenv::dotenv().ok(); // Load .env file if present

    // Check for api key in environment variables
    match env::var(var_name) {
        Ok(key) if !key.is_empty() => key,
        _ => {
            panic!("Failed to load {} environment variable from '.env'.", var_name);
        }
    }
}


// Helper function to get API key or skip test
pub fn get_api_key_or_skip(var_name: &str, test_name: &str) -> Option<String> {
    dotenv::dotenv().ok(); // Load .env file if present

    // Check for api key in environment variables
    match env::var(var_name) {
        Ok(key) if !key.is_empty() => Some(key),
        _ => {
            println!("Skipping integration test {} - {} environment variable not set.", test_name, var_name);
            None // Signal to skip
        }
    }
}


