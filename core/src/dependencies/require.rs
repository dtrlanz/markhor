use thiserror::Error;

use crate::dependencies::Assets;


pub trait Require {
    fn require(assets: &Assets) -> Result<Self, MeetRequirementError> where Self: Sized;
}


#[derive(Debug, Error)]
pub enum MeetRequirementError {
    #[error("Missing dependency: {0}")]
    DependencyNotAvailable(String),
}