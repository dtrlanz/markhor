# markhor (CLI)

The `markhor` binary is the user-facing command-line interface. It lives in the
`markhor_cli` package, which also holds the terminal UI library.

## Setup

Set the API keys in the environment or in a `.env` file in the directory you run
the binary from. Only the keys for the commands you use are needed.

```
GOOGLE_API_KEY=<your key>
GOOGLE_API_KEY_PERMISSIONS=public     # for `chat`
MISTRAL_API_KEY=<your key>
MISTRAL_API_KEY_PERMISSIONS=public    # for `import`
```

## Usage

```
cargo run -p markhor_cli -- --help
cargo run -p markhor_cli -- chat
cargo run -p markhor_cli -- chat --model gemma-4-31b-it
cargo run -p markhor_cli -- import scan.pdf
cargo run -p markhor_cli -- import https://example.com/report.pdf --folder notes
cargo run -p markhor_cli -- import scan.pdf --pages 0,1 --no-images
```

### `chat`

A terminal chat with a model served by the Gemini API. Without `--model`,
`gemini-flash-lite-latest` is used. With `--model`, that exact model id is used.

- `/help` lists the available commands.
- `/exit` ends the chat.

Ctrl-C or end of input also ends the chat. Any other input, including unknown
`/names`, is sent to the model as a message.

### `import`

Imports a visual document (PDF, image, or another format Mistral OCR accepts)
into a folder of a Markhor workspace. The text is recognized with Mistral OCR and
stored as one part of the new document. The original file is copied into the
folder.

- `<INPUT>` is a local file or an `http(s)` URL. URLs are downloaded once and
  stored with the document.
- `--folder` is the folder to import into. It defaults to the current directory
  and must be inside a workspace.
- `--pages` takes zero-based page numbers. It defaults to all pages.
- `--no-images` does not store the images found in the document. By default they
  are stored as attachments. Without them, image links in the text are left as
  the OCR output wrote them and point at nothing.
- `--page-mode` chooses how pages are laid out in the text:
  - `milestones` (default): each page starts with a
    `<milestone unit="page" n="{index}" />` marker, where `n` is the zero-based
    page index. The markers stay in the text, including what chunkers and
    embedding models see.
  - `parts`: each page becomes its own part.
  - `merged`: pages are joined by blank lines, with no markers. Page boundaries
    are lost.

  In every mode, a page break inside a paragraph, list, table, or code block is
  kept, so that block may render differently from the original page.

On success the path of the new document, relative to the workspace, is printed.
An existing document or sidecar with the same name is an error, and nothing is
changed.

## Logging

Logs go to stderr. Set `RUST_LOG` to change the level (default `warn`).

## Simulator

`markhor_chat_sim` (in `cli/sim`) is an unpublished development tool that runs
the chat UI against a scripted model, with injected latency and failures:

```
cargo run -p markhor_chat_sim -- --seed 1
```
