//! Dynamic, context-aware dependency resolution and extension management.
//!
//! This module provides a inversion-of-control (IoC) system designed around the
//! [`Session`] container and the [`Provide`] trait. It allows components to request 
//! their dependencies dynamically, filter them, and use them based on the current 
//! runtime context, permissions, and available services.
//!
//! # Core Concepts
//! 
//! * **[`Session`]**: The state of the world for a specific execution run. It holds 
//!   active services, tracks access control, and enforces permission boundaries.
//! * **[`Provide`]**: A trait implemented by components to describe how they are 
//!   made available within a `Session`. 
//!
//! # Basic Usage
//! 
//! The easiest way to implement dependency resolution is via the `#[derive(Provide)]` macro. 
//! You can then extract your component from the session using [`Session::resolve`].
//!
//! TODO: add example
//!

mod provide;
mod session;

pub use provide::{Provide, ResolveDependencyError};
pub use session::Session;

