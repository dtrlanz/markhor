# Markhor Extensions Library

This library provides the extensions for the Markhor platform, including model clients.

Each extension implements the `Extension` trait from `markhor_core` and returns boxed components, such as chat models, embedding models, chunkers, and OCR models.

## Modules

- `google`: Gemini API client.
- `anthropic`: Anthropic API client and chat model.
- `mistral`: Mistral API client for OCR and file handling.
- `chunking::plaintext`: Plain-text chunker.

## Test

```
cargo test -p markhor_extensions
```

Some tests call live APIs and are marked `#[ignore]`. For example, the Google chat tests need `GOOGLE_API_KEY`. Run them with:

```
cargo test -p markhor_extensions -- --ignored
```
