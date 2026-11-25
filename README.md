# Markhor

***Intelligent, Markdown-based knowledge management*** 

Markhor is a platform connecting AI models, documents, and workflows for knowledge work.

## Project status

**This project is in alpha.** Every version has breaking changes.

## Project structure

The project currently consists of four packages as outlined below.

If you want a binary, you'll find that in `markhor_cli`.

If you want libraries to use for your own projects, start with `markhor` and add any extensions from `markhor_extensions` that you want to include. Switch to `markhor_core` if you want fewer dependencies or more control over agent experience. If you're writing your own extensions, import the relevant traits from `markhor_core`.

### `markhor_core`

Contains the basic building blocks with which the platform is built.

- Core types/abstractions for AI (models, messages, etc.)
- Extension system
- Permission & session management
- File management (collections, documents, metadata)

A long-term goal is to make at least `markhor_core` runtime-agnostic, but this is not an immediate priority. Currently, the entire project uses `tokio`.

### `markhor`

Adds abstractions over core types and model context to facilitate app/agent development.

- Agent experience
- Workflows & orchestration
- Re-exports of relevant core types

### `markhor_cli`

Contains the CLI app.

- Command-line interface
- App configuration
- Extensions exclusive to the CLI app

### `markhor_extensions`

Contains standard extensions.

- Clients for model providers (Anthropic, Google, etc.)
- Other extensions such as chunkers

## Project goals

### Goals

- Simplify the use of AI for interacting with documents.
- Integrate various AI models for personal, local knowledge management.
- Make agentic AI more affordable by designing with low-cost and local models in mind.
- Enable user-driven automation with easily customizable workflows and agents.
- Facilitate portability, future-proof access, and compatibility with other software (e.g., backup, sync) by privileging plain text.
- Give users control over their own data and its privacy.
- Foster extensibility and customizability.

### Non-Goals

- Create an entire AI application framework (like LangChain): Instead, focus on core use cases related to local knowledge management, integrate with existing frameworks, and avoid excessive abstraction while maintaining extensibility.
- Create a general-purpose AI agent (like OpenClaw): Instead, simplify design and maintenance of agents or agentic workflows for specific use cases.
- Offer a coding agent or harness for software development: Instead, prioritize knowledge management and channel AI models' coding capabilities toward this goal.
- Offer a feature-rich document editor: Instead, prioritize offering highly customizable ways of viewing data over a general-purpose editing experience.
- Support non-text media types as first-class citizens: Instead, maximize the effectiveness of plain text in its various forms (Markdown, HTML, YAML, etc.), support text-related media (e.g., inline images, speech), and facilitate conversion to/from plain text.
- Provide synchronization and backup: Instead, prioritize compatibility with other software (e.g., file sync, git) and allow for extensions that may meet this need.

## License

Licensed under either of <a href="LICENSE-APACHE">Apache License, Version
2.0</a> or <a href="LICENSE-MIT">MIT license</a> at your option.

Unless you explicitly state otherwise, any contribution intentionally submitted for inclusion in this repository by you shall be licensed as MIT or Apache-2, without any additional terms or conditions.
