# Markhor Core Library

This library provides the core AI functionalities for the Markhor platform, including model interfaces, embedding management, and data structures.

`markhor_core` is the domain layer; concrete providers live in extensions and are injected at runtime.

## Modules

- `chat`: Chat model traits and orchestration types.
- `embedding`: Embedding model traits.
- `chunking`, `ocr`, `vector_store`: Traits for chunkers, OCR models, and vector stores. Work in progress, very experimental, even more unstable than everything else, not yet used by the Markhor CLI.
- `dependencies`: A small IoC container. `Session` holds registered extensions and API keys, and `Provide` resolves components from it.
- `extension`: The async `Extension` trait, whose factory methods supply components. `Comp<T>` wraps a component with its owning extension.
- `library`: The on-disk knowledge base. A collection is a directory with a `.markhor/` config folder. Documents are Markdown files with YAML metadata.
- `markdown`: Markdown handling.
- `permissions`: `Restricted` resources and `Authorized` actors. An actor may access a resource only if it holds all required permissions.
- `tool`: Tool abstractions. Not yet implemented.
- `content`, `event`: Shared content and event types.

## Test

```
cargo test -p markhor_core
```

Some tests create temporary collections with `tempfile`. No API keys are needed.
