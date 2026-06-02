//! Permissions for controlling access to resources
//! 
//! This module defines a simple permission system that can be used to control access to resources
//! based on permissions granted to actors.
//! 
//! # Resources
//! 
//! Access to resources is controlled via the `Restricted` trait.
//! 
//! Resources are entities that can be accessed, such as documents, user input, or model output. 
//! Each resource can have a set of permissions required to access it.
//! 
//! # Actors
//! 
//! Actors are granted access to resources via the `Authorized` trait.
//! 
//! Actors are entities that can access resources, such as models, tools, agents. etc. Each actor 
//! can be granted a set of permissions. An actor can access a resource if it has all the 
//! permissions required by that resource.


mod authorized;
mod permission;
mod restricted;

pub use permission::{Permission, ON_DEVICE, NOT_USED_FOR_TRAINING, PUBLIC};
pub use authorized::Authorized;
pub use restricted::Restricted;
