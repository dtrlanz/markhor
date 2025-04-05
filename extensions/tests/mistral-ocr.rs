use std::{ops::Deref, path::{Path, PathBuf}, sync::atomic::{AtomicU64, Ordering}};

use markhor_extensions::ocr::mistral::{client::MistralClient, helpers::{DocumentInput, OcrRequest}};
use tokio::fs;

mod common;


// Path to the test files are relative to the crate root
const TEST_PDF_PATH: &str = "tests/common/lorem ipsum with mona lisa.pdf";

const USE_TEMP_DIR: bool = true; // Set to true to use a temporary directory for output

enum OutputDir {
    TempDir(tempfile::TempDir),
    FixedDir(PathBuf),
}

impl Deref for OutputDir {
    type Target = Path;

    fn deref(&self) -> &Self::Target {
        match self {
            OutputDir::TempDir(temp_dir) => temp_dir.path(),
            OutputDir::FixedDir(path_buf) => path_buf,
        }
    }
}

fn get_output_dir(file_path: &str) -> OutputDir {
    // Create a temporary directory for potential output saving
    if USE_TEMP_DIR {
        let temp_dir = tempfile::tempdir().expect("Failed to create temporary directory");
        OutputDir::TempDir(temp_dir)
    } else {
        // Append file name to the output directory
        let path_buf = Path::new("tests/output").join(Path::new(file_path).file_name().unwrap()).to_path_buf();
        OutputDir::FixedDir(path_buf)
    }
}

// Adjust the number of tests to run per second to avoid exceeding rate limits
const TESTS_PER_MINUTE: u64 = 20; // Number of tests to run per second
const TEST_DELAY_IN_MILLISECONDS: u64 = 60 * 1000 / TESTS_PER_MINUTE; // Delay in milliseconds
static TEST_COUNT: AtomicU64 = AtomicU64::new(0);

async fn delay_test() {
    let count = TEST_COUNT.fetch_add(1, Ordering::SeqCst);
    tokio::time::sleep(tokio::time::Duration::from_millis(TEST_DELAY_IN_MILLISECONDS * count)).await;
}

// Ignored by default to avoid running costly API calls on every `cargo test`
// Run specifically with: `cargo test -- --ignored`
#[tokio::test]
#[ignore]
async fn pdf_ocr_via_public_url() {
    delay_test().await; // Delay to avoid rate limits
    
    // Setup tracing subscriber
    //tracing_subscriber::fmt::init();

    // --- Setup ---
    let api_key = common::get_api_key("MISTRAL_API_KEY");
    let client = MistralClient::new(api_key);

    // Create a temporary directory for potential output saving
    let output_dir = get_output_dir("public pdf");
    let output_dir_path: &Path = &output_dir;
    println!("Test output will be saved to: {}", output_dir_path.display()); // Info for debugging
    
    let request = OcrRequest {
        model: "mistral-ocr-latest".to_string(),
        document: DocumentInput::DocumentUrl {
            document_url: "https://arxiv.org/pdf/2201.04234".to_string(),
        },
        include_image_base64: Some(true),
        // Set other optional fields if needed
        id: None,
        pages: None,
        image_limit: None,
        image_min_size: None,
    };

    match client.process_document(&request).await {
        Ok(response) => {
            println!("Successfully processed document using model: {}", response.model);
            println!("Processed {} pages.", response.usage_info.pages_processed);

            // Call the new save function
            match response.save_to_files(&*output_dir).await {
                Ok(()) => println!("Successfully saved output to '{}'", output_dir.to_str().unwrap()),
                Err(e) => eprintln!("Error saving OCR output: {}", e),
            }

            for page in response.pages {
                println!("\n--- Page {} ---", page.index + 1); // Display 1-based index
                println!("Dimensions: {}x{} @ {} DPI", page.dimensions.width, page.dimensions.height, page.dimensions.dpi);
                println!("Extracted {} images.", page.images.len());
                // Optionally print markdown or image details
                // println!("Markdown:\n{}", page.markdown);
                for image in page.images {
                    println!("  Image ID: {}, Coords: ({},{}),({},{}) Size: ~{} bytes",
                        image.id,
                        image.top_left_x, image.top_left_y,
                        image.bottom_right_x, image.bottom_right_y,
                        image.image_base64.len() * 3 / 4 // Rough estimate of decoded size
                    );
                    // You could use the `base64` crate here to decode image.image_base64
                }
            }
        }
        Err(e) => {
             eprintln!("Error processing document: {}", e);
        }
    }
}

// Ignored by default to avoid running costly API calls on every `cargo test`
// Run specifically with: `cargo test -- --ignored`
#[tokio::test]
#[ignore]
async fn png_ocr_via_public_url() {
    delay_test().await; // Delay to avoid rate limits

    // Setup tracing subscriber
    //tracing_subscriber::fmt::init();

    // --- Setup ---
    let api_key = common::get_api_key("MISTRAL_API_KEY");
    let client = MistralClient::new(api_key);

    // Create a temporary directory for potential output saving
    let output_dir = get_output_dir("public png");
    let output_dir_path: &Path = &output_dir;
    println!("Test output will be saved to: {}", output_dir_path.display()); // Info for debugging
    
    let request = OcrRequest {
        model: "mistral-ocr-latest".to_string(),
        document: DocumentInput::ImageUrl { 
            image_url: "https://raw.githubusercontent.com/mistralai/cookbook/refs/heads/main/mistral/ocr/receipt.png".into(),
        },
        include_image_base64: Some(true),
        // Set other optional fields if needed
        id: None,
        pages: None,
        image_limit: None,
        image_min_size: None,
    };

    match client.process_document(&request).await {
        Ok(response) => {
            println!("Successfully processed document using model: {}", response.model);
            println!("Processed {} pages.", response.usage_info.pages_processed);

            // Call the new save function
            match response.save_to_files(&*output_dir).await {
                Ok(()) => println!("Successfully saved output to '{}'", output_dir.to_str().unwrap()),
                Err(e) => eprintln!("Error saving OCR output: {}", e),
            }

            for page in response.pages {
                println!("\n--- Page {} ---", page.index + 1); // Display 1-based index
                println!("Dimensions: {}x{} @ {} DPI", page.dimensions.width, page.dimensions.height, page.dimensions.dpi);
                println!("Extracted {} images.", page.images.len());
                // Optionally print markdown or image details
                // println!("Markdown:\n{}", page.markdown);
                for image in page.images {
                    println!("  Image ID: {}, Coords: ({},{}),({},{}) Size: ~{} bytes",
                        image.id,
                        image.top_left_x, image.top_left_y,
                        image.bottom_right_x, image.bottom_right_y,
                        image.image_base64.len() * 3 / 4 // Rough estimate of decoded size
                    );
                    // You could use the `base64` crate here to decode image.image_base64
                }
            }
        }
        Err(e) => {
             eprintln!("Error processing document: {}", e);
        }
    }
}

#[tokio::test]
#[ignore]
async fn pdf_ocr_via_file_upload_workflow() {
    delay_test().await; // Delay to avoid rate limits

    // --- Setup ---
    let api_key = common::get_api_key("MISTRAL_API_KEY");
    let client = MistralClient::new(api_key);
    let file_path = Path::new(TEST_PDF_PATH);

    // Basic check: ensure the test file exists before proceeding
    assert!(
        file_path.exists(),
        "Test file not found at '{}'. Please place a sample PDF there.",
        TEST_PDF_PATH
    );

    // Create a temporary directory for potential output saving
    let output_dir = get_output_dir(TEST_PDF_PATH);
    let output_dir_path: &Path = &output_dir;
    println!("Test output will be saved to: {}", output_dir_path.display()); // Info for debugging

    // --- Step 1: Upload File ---
    println!("Step 1: Uploading file '{}'...", file_path.display());
    let upload_response = client
        .upload_file(file_path, "ocr") // Specify "ocr" purpose
        .await
        .expect("1. File upload failed");

    println!(
        " -> Upload successful. File ID: {}, Filename: {}",
        upload_response.id, upload_response.filename
    );
    assert_eq!(upload_response.purpose, "ocr");
    assert!(upload_response.bytes > 0);
    assert_eq!(upload_response.filename, "sample.pdf"); // Assuming this filename
    assert!(!upload_response.id.is_empty());
    let file_id = upload_response.id; // Keep the ID for the next step

    // --- Step 2: Get Signed URL ---
    // Add a small delay? Sometimes eventual consistency might apply, though unlikely needed here.
    // tokio::time::sleep(Duration::from_secs(1)).await;
    println!("Step 2: Getting signed URL for file ID '{}'...", file_id);
    let expiry_hours = Some(1u32); // Request a 1-hour expiry URL
    let signed_url_response = client
        .get_signed_url(&file_id, expiry_hours)
        .await
        .expect("2. Getting signed URL failed");

    println!(" -> Signed URL obtained: {}...", &signed_url_response.url[..50]); // Print prefix
    assert!(!signed_url_response.url.is_empty());
    assert!(signed_url_response.url.starts_with("https://")); // Basic check
    let usable_url = signed_url_response.url; // Keep the URL for the next step

    // --- Step 3: Process Document (OCR) ---
    println!("Step 3: Sending document URL to OCR endpoint...");
    let ocr_request = OcrRequest {
        model: "mistral-ocr-latest".to_string(), // Use the appropriate model
        document: DocumentInput::DocumentUrl {
            document_url: usable_url, // Use the signed URL obtained previously
        },
        include_image_base64: Some(true), // Request images if sample.pdf has them
        // Set other optional fields if needed for the test
        id: None,
        pages: None,
        image_limit: None,
        image_min_size: None,
    };

    let ocr_response = client
        .process_document(&ocr_request)
        .await
        .expect("3. OCR processing failed");

    println!(
        " -> OCR successful. Model: {}, Pages processed: {}",
        ocr_response.model, ocr_response.usage_info.pages_processed
    );

    // Core assertions for OCR success
    assert!(ocr_response.usage_info.pages_processed > 0, "Expected at least one page processed");
    assert_eq!(
        ocr_response.pages.len() as u32,
        ocr_response.usage_info.pages_processed,
        "Number of page details should match pages processed count"
    );

    // Check details of the first page (basic checks)
    if let Some(first_page) = ocr_response.pages.first() {
        println!(
            " -> First page index: {}, Markdown length: {}, Images found: {}",
            first_page.index,
            first_page.markdown.len(),
            first_page.images.len()
        );
        assert!(!first_page.markdown.is_empty(), "First page markdown should not be empty");

        // Specific checks based on sample pdf
        assert!(first_page.markdown.contains("Lorem ipsum"), "Expected 'Lorem ipsum' in the first page markdown");

        // Assuming images in sample pdf
        assert!(!first_page.images.is_empty(), "Expected images on the first page");
    } else {
        panic!("OCR response had 0 pages in the details array, but processed > 0");
    }

    // --- Step 4 (Optional): Test Saving Output ---
    println!("Step 4: Saving OCR output to '{}'...", output_dir_path.display());
    ocr_response
        .save_to_files(output_dir_path)
        .await
        .expect("4. Saving OCR output failed");

    // Verify that expected files were created
    let expected_md_path = output_dir_path.join("output.md");
    let expected_images_dir = output_dir_path.join("images");

    assert!(
        expected_md_path.exists(),
        "Expected output file '{}' was not created",
        expected_md_path.display()
    );
    assert!(
        fs::read_to_string(&expected_md_path).await.expect("Failed to read output.md").len() > 0,
        "output.md should not be empty"
    );


    // Check images directory *if* images were expected and found
    let images_were_found = ocr_response.pages.iter().any(|p| !p.images.is_empty());
    if images_were_found {
        assert!(
            expected_images_dir.exists() && expected_images_dir.is_dir(),
            "Expected images directory '{}' was not created or is not a directory",
            expected_images_dir.display()
        );
        // Optionally, count files in the images dir or check specific names if predictable
        //let image_files = fs::read_dir(&expected_images_dir).expect("Could not read images dir").count();
        //assert!(image_files > 0, "Images directory is empty but images were expected");
    } else {
        println!(" -> No images were extracted by OCR (or include_image_base64 was false), skipping image directory check.");
        assert!(!expected_images_dir.exists(), "Images directory should not exist if no images were extracted.");
    }

    println!("Integration test completed successfully!");
}

#[tokio::test]
#[ignore]
async fn lorem_ipsum_png_ocr() {
    delay_test().await; // Delay to avoid rate limits

    test_image_ocr_helper("tests/common/Lorem ipsum.png", "Lorem ipsum").await;
}

#[tokio::test]
#[ignore] 
async fn lorem_ipsum_jpg_ocr() {
    delay_test().await; // Delay to avoid rate limits

    test_image_ocr_helper("tests/common/Lorem ipsum.jpg", "Lorem ipsum").await;
}

#[tokio::test]
#[ignore] 
async fn lorem_ipsum_with_mona_lisa_png_ocr() {
    delay_test().await; // Delay to avoid rate limits

    test_image_ocr_helper("tests/common/Lorem ipsum with Mona Lisa.png", "Lorem ipsum").await;
}

#[tokio::test]
#[ignore]
async fn mona_lisa_jpg_ocr() {
    delay_test().await; // Delay to avoid rate limits

    test_image_ocr_helper("tests/common/Mona Lisa.jpg", "").await;
}

async fn test_image_ocr_helper(test_file: &str, expected_text: &str) {
    // --- Setup ---
    let api_key = common::get_api_key("MISTRAL_API_KEY");
    let client = MistralClient::new(api_key);
    let file_path = Path::new(test_file);

    // Basic check: ensure the test file exists before proceeding
    assert!(
        file_path.exists(),
        "Test file not found at '{}'. Please place a sample file there.",
        test_file
    );

    // Create a temporary directory for potential output saving
    let output_dir = get_output_dir(test_file);
    let output_dir_path: &Path = &output_dir;
    println!("Test output will be saved to: {}", output_dir_path.display()); // Info for debugging

    // --- Step 1: Upload File ---
    println!("Step 1: Uploading file '{}'...", file_path.display());
    let upload_response = client
        .upload_file(file_path, "ocr") // Specify "ocr" purpose
        .await
        .expect("1. File upload failed");

    println!(
        " -> Upload successful. File ID: {}, Filename: {}",
        upload_response.id, upload_response.filename
    );
    assert_eq!(upload_response.purpose, "ocr");
    assert!(upload_response.bytes > 0);
    assert_eq!(upload_response.filename, file_path.file_name().unwrap().to_str().unwrap()); // Assuming this filename
    assert!(!upload_response.id.is_empty());
    let file_id = upload_response.id; // Keep the ID for the next step

    // --- Step 2: Get Signed URL ---
    // Add a small delay? Sometimes eventual consistency might apply, though unlikely needed here.
    // tokio::time::sleep(Duration::from_secs(1)).await;
    println!("Step 2: Getting signed URL for file ID '{}'...", file_id);
    let expiry_hours = Some(1u32); // Request a 1-hour expiry URL
    let signed_url_response = client
        .get_signed_url(&file_id, expiry_hours)
        .await
        .expect("2. Getting signed URL failed");

    println!(" -> Signed URL obtained: {}...", &signed_url_response.url);
    assert!(!signed_url_response.url.is_empty());
    assert!(signed_url_response.url.starts_with("https://")); // Basic check
    let usable_url = signed_url_response.url; // Keep the URL for the next step

    // --- Step 3: Process Document (OCR) ---
    println!("Step 3: Sending document URL to OCR endpoint...");
    let ocr_request = OcrRequest {
        model: "mistral-ocr-latest".to_string(), // Use the appropriate model
        document: DocumentInput::ImageUrl {
            image_url: usable_url, // Use the signed URL obtained previously
        },
        include_image_base64: Some(true), // Request images if sample.pdf has them
        // Set other optional fields if needed for the test
        id: None,
        pages: None,
        image_limit: None,
        image_min_size: None,
    };

    let ocr_response = client
        .process_document(&ocr_request)
        .await
        .expect("3. OCR processing failed");

    println!(
        " -> OCR successful. Model: {}, Pages processed: {}",
        ocr_response.model, ocr_response.usage_info.pages_processed
    );

    // Core assertions for OCR success
    assert!(ocr_response.usage_info.pages_processed > 0, "Expected at least one page processed");
    assert_eq!(
        ocr_response.pages.len() as u32,
        ocr_response.usage_info.pages_processed,
        "Number of page details should match pages processed count"
    );

    // Check details of the first page (basic checks)
    let content_checks = if let Some(first_page) = ocr_response.pages.first() {
        println!(
            " -> First page index: {}, Markdown length: {}, Images found: {}",
            first_page.index,
            first_page.markdown.len(),
            first_page.images.len()
        );
        assert!(!first_page.markdown.is_empty(), "First page markdown should not be empty");


        // Specific checks based on sample pdf
        // Defer until after file is saved (to allow for manual inspection if needed)
        || {
            assert!(first_page.markdown.contains(expected_text), 
                "Expected '{}' in the first page markdown", expected_text);

            // Uncomment if images in sample pdf
            //assert!(!first_page.images.is_empty(), "Expected images on the first page");
        }
    } else {
        panic!("OCR response had 0 pages in the details array, but processed > 0");
    };

    // --- Step 4 (Optional): Test Saving Output ---
    println!("Step 4: Saving OCR output to '{}'...", output_dir.display());
    ocr_response
        .save_to_files(&*output_dir)
        .await
        .expect("4. Saving OCR output failed");

    // Check content
    content_checks();

    // Verify that expected files were created
    let expected_md_path = output_dir.join("output.md");
    let expected_images_dir = output_dir.join("images");

    assert!(
        expected_md_path.exists(),
        "Expected output file '{}' was not created",
        expected_md_path.display()
    );
    assert!(
        fs::read_to_string(&expected_md_path).await.expect("Failed to read output.md").len() > 0,
        "output.md should not be empty"
    );


    // Check images directory *if* images were expected and found
    let images_were_found = ocr_response.pages.iter().any(|p| !p.images.is_empty());
    if images_were_found {
        assert!(
            expected_images_dir.exists() && expected_images_dir.is_dir(),
            "Expected images directory '{}' was not created or is not a directory",
            expected_images_dir.display()
        );
        // Optionally, count files in the images dir or check specific names if predictable
        //let image_files = fs::read_dir(&expected_images_dir).expect("Could not read images dir").count();
        //assert!(image_files > 0, "Images directory is empty but images were expected");
    } else {
        println!(" -> No images were extracted by OCR (or include_image_base64 was false), skipping image directory check.");
        assert!(!expected_images_dir.exists(), "Images directory should not exist if no images were extracted.");
    }

    println!("Integration test completed successfully!");
}

