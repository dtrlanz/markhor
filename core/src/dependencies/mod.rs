//! Dynamic, context-aware dependency resolution and extension management.
//!
//! This module provides a inversion-of-control (IoC) system designed around the
//! [`Session`] container and the [`Resolve`] trait. It allows components to request 
//! their dependencies dynamically, filter them, and use them based on the current 
//! runtime context, permissions, and available services.
//!
//! # Core Concepts
//! 
//! * **[`Session`]**: The state of the world for a specific execution run. It holds 
//!   active services, tracks access control, and enforces permission boundaries.
//! * **[`Resolve`]**: A trait implemented by components to describe how they are 
//!   constructed from a `Session`. 
//!
//! # Basic Usage
//! 
//! The easiest way to implement resolution is via the `#[derive(Resolve)]` macro. 
//! You can then extract your component from the session using [`Session::resolve`].
//!
//! TODO: add example
//!
//! # Advanced Resolution (The Macro)
//!
//! The `#[derive(Resolve)]` macro natively understands several standard Rust types and
//! provides the `#[resolve(...)]` attribute for fine-grained control over how dependencies
//! are fetched.
//!
//! ### Collections and Optional Dependencies
//! 
//! The macro automatically adjusts its behavior based on the field type:
//! 
//! * **`T` (Bare Type)**: Demands exactly one `T`. Returns an error if missing.
//! * **`Option<T>`**: Attempts to resolve `T`. If `T` is missing (or filtered out), 
//!   it silently evaluates to `None` without bubbling up an error.
//! * **`Vec<T>`**: Resolves all available instances of `T` in the session and collects 
//!   them into a vector.
//!
//! ### Filtering Dependencies
//! 
//! You can use `#[resolve(filter = |x| ...)]` to supply a raw closure that filters 
//! the resolved items. 
//!
//! TODO: add example
//!
//! ### Dependency Combinations with `#[resolve(each)]`
//! 
//! In LLM-driven applications, you may need to evaluate a Cartesian product of 
//! configurations (e.g., trying a prompt against 3 different models and 2 different 
//! tool sets). 
//!
//! The `each` attribute transforms the resolution into a nested loop, yielding 
//! one instance of your struct for every possible combination of the specified fields.
//!
//! TODO: add example
//! 
//! **Note on Cloning:** When using multiple `each` attributes, the types forming the 
//! *outer* loops must implement `Clone`. Standard dependencies do *not* need `Clone`,
//! as they are freshly evaluated inside the innermost loop.
//!

mod resolve;
mod session;

pub use resolve::{Resolve, ResolveDependencyError};
pub use session::Session;

