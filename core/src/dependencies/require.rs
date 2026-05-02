use thiserror::Error;

use crate::dependencies::Assets;

pub use derive_require::Require;

pub trait Require {
    fn require(assets: &Assets) -> Result<Self, MeetRequirementError> where Self: Sized;
}


#[derive(Debug, Error)]
pub enum MeetRequirementError {
    #[error("Missing dependency: {0}")]
    DependencyNotAvailable(String),
}


#[cfg(test)]
mod tests {
    use super::*;

    mod foo {
        use super::*;

        #[derive(Debug, PartialEq, Eq, Require)]
        pub struct Foo {
            pub bar: Bar,
            pub baz: Baz,
        }

        #[derive(Debug, PartialEq, Eq, Require)]
        pub struct Bar;

        #[derive(Debug, PartialEq, Eq, Require)]
        pub struct Baz;
    }    

    #[tokio::test]
    async fn derive_require() {
        let assets = Assets::new();
        let foo = foo::Foo::require(&assets).unwrap();
        assert_eq!(foo.bar, foo::Bar);
        assert_eq!(foo.baz, foo::Baz);
    }
}