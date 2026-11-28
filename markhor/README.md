# Markhor 

Markhor is a platform connecting AI models, documents, and workflows for knowledge work.

## `markhor` library crate

This crate adds abstractions and model context using types defined in `markhor_core`. It provides the application layer used by `markhor_cli`.

## Modules

- `agent`: Agent builder and configuration.
- `chat`: `Simple` chat orchestration.
- `import`: Document import using OCR models.
- `user`: User builder and configuration.

## See also

The Markhor project consists of several other crates:

- `markhor_cli` contains the CLI app.
- `markhor_core` contains the basic building blocks with which the platform is built.
- `markhor_core_derive` contains the derive macro for the trait `Provide`.
- `markhor_extensions` contains standard extensions (primarily model clients).

See the repository README for more information.
