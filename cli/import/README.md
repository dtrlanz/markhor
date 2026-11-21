# markhor-import

Imports a visual document (PDF, image, or another format Mistral OCR accepts)
into a folder of a Markhor workspace. The text is recognized with Mistral OCR,
and each page becomes a part of the new document. The original file is copied
into the folder.

## Setup

Set the Mistral API key in the environment or in a `.env` file in the directory
you run the binary from:

```
MISTRAL_API_KEY=<your key>
MISTRAL_API_KEY_PERMISSIONS=public
```

## Usage

```
cargo run -p markhor_import -- scan.pdf
cargo run -p markhor_import -- https://example.com/report.pdf --folder notes
cargo run -p markhor_import -- scan.pdf --pages 0,1 --include-images
```

- `<INPUT>` is a local file or an `http(s)` URL. URLs are downloaded once and
  stored with the document.
- `--folder` is the folder to import into. It defaults to the current directory
  and must be inside a workspace.
- `--pages` takes zero-based page numbers. It defaults to all pages.
- `--include-images` also stores the images found in the document.

On success the path of the new document, relative to the workspace, is printed.
An existing document or sidecar with the same name is an error, and nothing is
changed.

## Logging

Logs go to stderr. Set `RUST_LOG` to change the level (default `warn`).
