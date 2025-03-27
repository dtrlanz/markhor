# Markhor

**Intelligent, Markdown-based knowledge management:** Markhor is a platform for connecting AI models, documents, and workflows for knowledge work.

## Project Status

This project is still very much in pre-alpha development. Assume all APIs are experimental.

## Project Goals

-   Simplify the use of AI for interacting with documents.
-   Integrate various AI models for personal, local knowledge management.
-   Enable user-driven automation for common tasks using workflows and prompt templates.
-   Facilitate portability, future-proof access, and compatibility with other software (e.g., backup, sync) by privileging plain text.
-   Prioritize user control over data privacy.
-   Foster extensibility and customizability.

## Non-Goals

-   Create an entire AI application framework (like LangChain): Instead, focus on core use cases related to local knowledge management, integrate with existing frameworks, and avoid excessive abstraction while maintaining extensibility.
-   Create a general-purpose AI agent: Instead, offer a system that facilitates integration and/or creation of various AI agents.
-   Offer a fully integrated, feature-rich document editor or IDE: Instead, follow a road map that would allow for some of this functionality to be implemented via extensions.
-   Offer synchronization and collaboration features: Instead, aim for enough compatibility and extensibility that would allow other software (e.g., file sync) and extensions to meet this need.

Additionally, these are non-goals *for the time being* though they may be desirable in the long term:

-   Executor-agnostic implmentation: We're unapologetically relying on `tokio` for now and awaiting further developments in the world of `async` Rust.
-   ...

## Packages

-   `core`: Core AI functionalities (models, embeddings, etc.).
-   `cli`: Command-line interface.
-   `util`: Utility functions and modules.

## Getting Started

1.  Clone the repository.
2.  Navigate to the project directory.
3.  Build the project using `cargo build`.
4.  Run the CLI application using `cargo run --package cli`.

## License

Licensed under either of <a href="LICENSE-APACHE">Apache License, Version
2.0</a> or <a href="LICENSE-MIT">MIT license</a> at your option.

Unless you explicitly state otherwise, any contribution intentionally submitted for inclusion in this crate by you, as defined in the Apache-2.0 license, shall be dual licensed as above, without any additional terms or conditions. 