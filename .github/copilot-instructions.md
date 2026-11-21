# Copilot Instructions for Markhor

Markhor is a pre-alpha Rust workspace (edition 2024, resolver 3) for Markdown-based knowledge management with AI models. Assume all APIs are experimental and may change without notice.

## Build, test, lint

- Build: `cargo build` (whole workspace)
- Run binaries. The `markhor` package is a library with no binary, so `cargo run -p markhor` fails. The user-facing CLI is the `markhor` binary in package `markhor_cli`:
  - Chat: `cargo run -p markhor_cli -- chat --model <id>`
  - Import with OCR: `cargo run -p markhor_cli -- import <file-or-url>`
  - Chat simulator (dev only): `cargo run -p markhor_chat_sim` (binary `markhor-chat-sim`)
- Test all: `cargo test --workspace`
- Test one crate: `cargo test -p markhor_core` (package names: `markhor_core`, `markhor_extensions`, `markhor`, `derive_provide`, `markhor_cli`, `markhor_chat_sim`, `markhor_gui`, `markhor_util`)
- Test one test by name: `cargo test -p markhor_core dependencies::provide::tests::derive_provide -- --exact`
  - Without `--exact`, a plain substring filter works: `cargo test -p markhor_core provide`
- Include ignored tests: append `-- --ignored`. Ignored tests in `markhor_extensions` and `markhor/tests` call live APIs and need `GOOGLE_API_KEY`, `MISTRAL_API_KEY`, or `ANTHROPIC_API_KEY`; don't run them without keys.
- There is no CI, Makefile, or lint config. Use `cargo clippy --workspace` and `cargo fmt` with defaults if needed.

## Workspace layout

Package name differs from directory name:

| Directory | Package | Role |
|---|---|---|
| `core/` | `markhor_core` | Domain logic: library (workspaces, documents, folders), chat/embedding traits, chunking, markdown, permissions, dependency injection, extension trait |
| `core_derive_provide/` | `derive_provide` | Proc macro `#[derive(Provide)]` used by `markhor_core` |
| `extensions/` | `markhor_extensions` | Concrete extensions: `google` (Gemini API, serves Gemini/Gemma models), `anthropic`, `mistral` (OCR and files), `chunking::plaintext`, `plugin` |
| `markhor/` | `markhor` | Library (no binary): `agent` (agent builder/config), `chat` (`Simple` chat orchestration), `user`. `cli.rs` holds clap definitions that no binary uses yet |
| `cli/` | `markhor_cli` | The `markhor` binary (subcommands `chat`, `import`; sources in `cli/src/commands/`) and the terminal UI library (`convert`, `html`, `ui`) |
| `cli/sim` | `markhor_chat_sim` | Unpublished dev simulator binary `markhor-chat-sim` |
| `gui/`, `util/` | `markhor_gui`, `markhor_util` | Placeholders (`gui` is still hello-world) |

Dependency direction: `cli` and `cli/sim` and `markhor` → `markhor_extensions` → `markhor_core` → `derive_provide`. Core must never depend on extensions.

## Architecture (big picture)

**Extensions provide components.** `markhor_core::extension::Extension` is an async trait with `uri()`, `name()`, `description()`, `initialize()`, and factory methods (`chat_models`, `chat_uis`, `embedding_models`, `chunkers`, `ocr_models`, `tools`), each defaulting to empty. Extensions in `markhor_extensions` implement it and return boxed trait objects.

**Dependency injection via `Session`.** `core/src/dependencies/` implements an IoC container:
- `Session` (`session.rs`) holds registered extensions and API keys, enforces permissions, and tracks which components are used.
- `Provide` (`provide.rs`) is implemented by types that can be resolved from a session. `Session::resolve::<T>()` and `Provide::first(&assets)` do the lookup. Blanket impls exist for `Vec`, `Option`, `Box`, `Arc`, `Mutex`, `HashMap`, and tuples.
- `#[derive(Provide)]` (from `core_derive_provide`) generates impls from field attributes: `#[provide(each)]`, `filter = ...`, `map = ...`, `key = ...`, `sort_by = ...`. `sort_by_key` and `sort_by_field` are unimplemented. Always write `#[provide(crate = "crate")]` inside `markhor_core`.
- `Comp<T>` (`extension/comp.rs`) derefs to the component and wraps it with its owning extension (`extension()`, an `Arc<dyn Extension>`) and a `TrackingGuard`. Use `Comp<dyn ChatModel>` etc. as the resolved type. `metadata_id()` gives a stable identifier string.

**Permissions.** `core/src/permissions/` uses `Restricted` (resources declare `permissions_required()`) and `Authorized` (actors declare `permissions_granted()`). An actor may access a resource only if it holds all required permissions. Built-in permissions: `ON_DEVICE`, `NOT_USED_FOR_TRAINING`, `PUBLIC`. `Session::restrict_access` narrows what a session may do.

**Library model.** `core/src/library/` is the on-disk knowledge base:
- A workspace is a directory with a `.markhor/` config dir (`WORKSPACE_CONFIG_DIR` in `library/mod.rs`). `Workspace::open(path)` opens exactly that directory and returns `InWorkspace` if it sits inside one. `Folder::open(path)` walks up to the containing workspace. Nested workspaces are rejected (`find_workspace_descendant`, `library/workspace.rs`).
- Documents are Markdown files with YAML metadata. Derived data goes to `attachments/<document file name>/` (`ATTACHMENTS_DIR`), with `.cache.yaml` and `.chunks_<chunker-id>.yaml` files.
- Chunks are cached per chunker ID, so changing a chunker invalidates only its own cache.

**Error handling.** Core uses `thiserror` enums (`MarkhorError` in `core/src/lib.rs`, plus per-module errors such as `ResolveDependencyError`, `InitExtensionError`, `AccessLibraryError`). The CLI uses `anyhow` at the top level.

**Async.** Tokio everywhere (`#[tokio::main]`, `#[tokio::test]`). Traits use `async_trait`; some use `trait_variant`/`dynosaur`.

## Work-in-progress areas (important when changing code)

- `todo!()` placeholders in `core/src/dependencies/session.rs` (permission elevation failures) and `core/src/library/document/mod.rs` (Document permissions, text-load errors for chunks). `core/src/library/folder.rs` has one as well.
- `markhor/src/agent/config.rs` has an `unimplemented!()`.
- Ignored tests in `core/src/dependencies/session.rs` (`#[ignore = "waiting for dependency tracking redesign/alternative"]`) are known-broken; don't treat them as regressions.
- `markhor::cli` (clap definitions) is not wired to a binary. `gui` and `util` are placeholders.

When implementing a feature that touches these areas, finish the existing `todo!()`/`unimplemented!()` paths and wiring rather than adding a parallel path.

## Conventions

- Tests are inline `#[cfg(test)] mod tests` modules next to the code, plus a few integration tests in `core/tests/`. Follow the same pattern. Test helpers live in `pub(crate)` modules such as `chunking::test_chunker` and `embedding::test_utils`.
- Test extensions implement `Extension` inline with a `markhor://test-...` URI. Reuse `MockEmbedderExtension` and `FixedSizeChunkerExtension` rather than writing new mocks.
- Filesystem tests use `tempfile` (a dev-dependency of `markhor_core`) and open a workspace with `Workspace::open(&dir)`, as in `core/src/library/document/mod.rs` tests.
- Logging uses `tracing` macros. The binaries log to stderr, filtered by `RUST_LOG` (default `warn`), as in `cli/src/main.rs`.
- Secrets: each binary calls `dotenv::dotenv()` at the start of `main` (e.g., `cli/src/main.rs`), and `**/.env` is git-ignored. Do not read, print, or commit `.env` files. Never hardcode API keys.
- The root `tests/integration_test.rs` is never compiled, because the root manifest is a virtual workspace. Put new integration tests in a crate's `tests/` directory.
- `target/` is a large build directory. Exclude it from searches.
- Crate versions are shared via `version.workspace = true`; the version lives in the root `Cargo.toml` under `[workspace.package]`.
