# markhor-chat

A terminal chat with a Gemma model served by the Gemini API. It uses the
console UI from `markhor_cli` and the chat loop from `markhor`.

## Setup

Set the Google API key in the environment or in a `.env` file in the directory
you run the binary from:

```
GOOGLE_API_KEY=<your key>
GOOGLE_API_KEY_PERMISSIONS=public
```

## Usage

```
cargo run -p markhor_chat
cargo run -p markhor_chat -- --model gemma-4-31b-it
```

Without `--model`, the first Gemma 4 model the API lists is used. With
`--model`, that exact model id is used.

## Commands

- `/help` lists the available commands.
- `/exit` ends the chat.

Ctrl-C or end of input also ends the chat. Any other input, including unknown
`/names`, is sent to the model as a message.

## Logging

Logs go to stderr. Set `RUST_LOG` to change the level (default `warn`).
