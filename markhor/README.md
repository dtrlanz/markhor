# Markhor Library

This crate adds abstractions and model context using types defined in `markhor_core`. It provides the application layer used by `markhor_cli`.

## Modules

- `agent`: Agent builder and configuration.
- `chat`: `Simple` chat orchestration.
- `import`: Document import using OCR models.
- `user`: User builder and configuration.

## Test

```
cargo test -p markhor
```

Some tests call live APIs and are marked `#[ignore]`. They require API keys to be set.
