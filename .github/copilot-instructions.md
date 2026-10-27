# Copilot Instructions for Markhor

Markhor is a pre-alpha Rust workspace (edition 2024, resolver 3) for Markdown-based knowledge management with AI models. Assume all APIs are experimental and may change without notice.

## Build, test, lint

- Build: `cargo build` (or `cargo build -p markhor` for the CLI)
- Run CLI: `cargo run --package markhor -- <args>`
- Test all: `cargo test --workspace`
- Test one crate: `cargo test -p markhor_core` (crate names are `markhor_core`, `markhor_extensions`, `markhor`, `derive_provide`, `markhor_util`)
- Test one test by name: `cargo test -p markhor_core dependencies::provide::tests::derive_provide -- --exact`
  - Without `--exact`, a plain substring filter works: `cargo test -p markhor_core provide`
- Include ignored tests: append `-- --ignored`
- There is no CI, Makefile, or lint config. Use `cargo clippy --workspace` and `cargo fmt` with defaults if needed.
- Expect many `dead_code`/unused warnings in `markhor_extensions` (Gemini) and `markhor_core`; they are pre-existing, not regressions.

## Workspace layout

Package name differs from directory name:

| Directory | Package | Role |
|---|---|---|
| `core/` | `markhor_core` | Domain logic: library (workspaces, documents, folders), chat/embedding traits, chunking, markdown, permissions, dependency injection, extension trait |
| `core_derive_provide/` | `derive_provide` | Proc macro `#[derive(Provide)]` used by `markhor_core` |
| `extensions/` | `markhor_extensions` | Concrete extensions: Gemini client, Mistral OCR, plaintext chunker, CLI extension, plugin loader |
| `markhor/` | `markhor` | The CLI binary (`src/main.rs`), command handlers (`src/commands`), app context (`src/app`) |
| `cli/`, `gui/`, `util/` | `markhor_cli`, `markhor_gui`, `markhor_util` | Stubs/placeholders with no real code yet |

Dependency direction: `markhor` → `markhor_extensions` → `markhor_core` → `derive_provide`. Core must never depend on extensions.

## Architecture (big picture)

**Extensions provide components.** `markhor_core::extension::Extension` is an async trait with `uri()`, `name()`, `initialize()`, and factory methods (`chat_models`, `embedding_models`, `chunkers`, `converters`, `prompters`, `tools`), each defaulting to empty. Extensions in `markhor_extensions` implement it and return boxed trait objects.

**Dependency injection via `Session`.** `core/src/dependencies/` implements an IoC container:
- `Session` (`session.rs`) holds registered extensions and API keys, enforces permissions, and tracks which components are used.
- `Provide` (`provide.rs`) is implemented by types that can be resolved from a session. `Session::resolve::<T>()` and `Provide::first(&assets)` do the lookup. Blanket impls exist for `Vec`, `Option`, `Box`, `Arc`, `Mutex`, `HashMap`, and tuples.
- `#[derive(Provide)]` (from `core_derive_provide`) generates impls from field attributes: `#[provide(each)]`, `filter = ...`, `map = ...`, `key = ...`, `sort_by = ...`. Always write `#[provide(crate = "crate")]` inside `markhor_core`.
- `Comp<T>` (`extension/comp.rs`) wraps a component with its owning extension (`Arc<dyn Extension>`) and a `TrackingGuard`. Use `Comp<dyn ChatModel>` etc. as the resolved type. `metadata_id()` gives a stable identifier string.

**Permissions.** `core/src/permissions/` uses `Restricted` (resources declare `permissions_required()`) and `Authorized` (actors declare `permissions_granted()`). An actor may access a resource only if it holds all required permissions. Built-in permissions: `ON_DEVICE`, `NOT_USED_FOR_TRAINING`, `PUBLIC`. `Session::restrict_access` narrows what a session may do.

**Library model.** `core/src/library/` is the on-disk knowledge base:
- A workspace is a directory with a `.markhor/` config dir (`WORKSPACE_CONFIG_DIR` in `library/mod.rs`). `Workspace::open` walks up parents to find it.
- Documents are Markdown files with YAML metadata. Derived data goes to `attachments/<doc>/` (`ATTACHMENTS_DIR`), with `.cache.yaml` and `.chunks_<chunker-id>.yaml` files.
- Chunks are cached per chunker ID, so changing a chunker invalidates only its own cache.

**Error handling.** Core uses `thiserror` enums (`MarkhorError` in `core/src/lib.rs`, plus per-module errors such as `ResolveDependencyError`, `InitExtensionError`, `AccessLibraryError`). The CLI uses `anyhow` at the top level.

**Async.** Tokio everywhere (`#[tokio::main]`, `#[tokio::test]`). Traits use `async_trait`; some use `trait_variant`/`dynosaur`.

## Work-in-progress areas (important when changing code)

- `markhor/src/app/mod.rs`: `Markhor::import`, `Markhor::chat`, and `use_extension` are `unimplemented!()`, with the old job-based implementation left commented out. Commands that call them will panic.
- `markhor/src/main.rs`: extension registration (Gemini, Mistral, CLI, chunkers) is commented out, so `Markhor` runs with no extensions.
- Ignored tests in `core/src/dependencies/session.rs` (`#[ignore = "waiting for dependency tracking redesign"]`) are known-broken; don't treat them as regressions.
- Placeholder crates `cli`, `gui`, `util` have no real code.

When implementing a feature that touches these areas, finish the wiring (e.g., re-enable the extension list in `main.rs`) rather than adding a parallel path.

## Conventions

- Tests are inline `#[cfg(test)] mod tests` modules next to the code, plus a few integration tests in `core/tests/`. Follow the same pattern. Test helpers live in `pub(crate)` modules such as `chunking::test_chunker` and `embedding::test_utils`.
- Test extensions implement `Extension` inline with a `markhor://test-...` URI. Reuse `MockEmbedderExtension` and `FixedSizeChunkerExtension` rather than writing new mocks.
- Filesystem tests use `tempfile` (a dev-dependency of `markhor_core`). Document tests open files via `Document::open(ws.clone(), &dir.join("doc.md"))`.
- Logging uses `tracing` macros. The CLI sends logs to stderr and reads `RUST_LOG` first, then `-v`/`-q` flags.
- Secrets: `dotenv::dotenv()` is loaded in `main.rs`, and `**/.env` is git-ignored. Do not read, print, or commit `.env` files (e.g., `extensions/.env`). Never hardcode API keys.
- `target/` is a large build directory. Exclude it from searches.
- Crate versions are shared via `version.workspace = true`; the version lives in the root `Cargo.toml` under `[workspace.package]`.
