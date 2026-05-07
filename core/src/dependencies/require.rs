use std::any::type_name;
use thiserror::Error;

use crate::dependencies::Assets;

pub use derive_require::Require;

pub trait Require {
    fn require(assets: &Assets) -> Result<Self, MeetRequirementError> 
    where 
        Self: Sized 
    {
        Self::require_iter(assets)?
            .next()
            .ok_or_else(|| {
                // Get name of the missing struct
                let name = type_name::<Self>().to_string();
                MeetRequirementError::DependencyNotAvailable(name)
            })
    }

    fn require_iter(assets: &Assets) -> Result<impl Iterator<Item = Self>, MeetRequirementError> 
    where 
        Self: Sized;
}


#[derive(Debug, Error)]
pub enum MeetRequirementError {
    #[error("Missing dependency: {0}")]
    DependencyNotAvailable(String),
}


#[cfg(test)]
mod tests {
    use super::*;

    mod simple {
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

    #[test]
    fn derive_require() {
        let assets = Assets::new();
        let foo = simple::Foo::require(&assets).unwrap();
        assert_eq!(foo.bar, simple::Bar);
        assert_eq!(foo.baz, simple::Baz);

        let vec = simple::Foo::require_iter(&assets).unwrap().collect::<Vec<_>>();
        assert_eq!(vec.len(), 1);
        assert_eq!(vec[0].bar, simple::Bar);
        assert_eq!(vec[0].baz, simple::Baz);
    }

    mod enumerated {
        use super::*;

        #[derive(Debug, PartialEq, Eq)]
        pub struct Foo {
            pub idx: usize,
            pub bar: Bar,
        }

        impl Require for Foo {
            fn require_iter(assets: &Assets) -> Result<impl Iterator<Item = Self>, MeetRequirementError> {
                let vec = (0..5)
                    .map(|idx| {
                        let bar = Bar::require(assets).unwrap();
                        Self { idx, bar }
                    })
                    .collect::<Vec<_>>();
                Ok(vec.into_iter())
            }
        }

        #[derive(Debug, PartialEq, Eq, Require)]
        pub struct Bar;

        #[derive(Debug, PartialEq, Eq)]
        pub struct Blank;

        impl Require for Blank {
            fn require_iter(_assets: &Assets) -> Result<impl Iterator<Item = Self>, MeetRequirementError> {
                Result::<std::iter::Empty<Self>, _>::Err(
                    MeetRequirementError::DependencyNotAvailable("Blank".to_string())
                )
            }
        }
    }

    #[test]
    fn vec_and_option_without_filters() {
        #[derive(Debug, Require)]
        struct TestNoFilter {
            all_foos: Vec<enumerated::Foo>,
            first_foo: Option<enumerated::Foo>,
            missing_blank: Option<enumerated::Blank>, // Should silently become None
        }

        let assets = Assets::new();
        let result = TestNoFilter::require(&assets).expect("Failed to build TestNoFilter");

        // Vec should collect all 5 Foos
        assert_eq!(result.all_foos.len(), 5);
        
        // Option should just grab the first one (idx 0)
        assert_eq!(result.first_foo.unwrap().idx, 0);
        
        // Option<Blank> should swallow the Blank error and return None
        assert!(result.missing_blank.is_none());
    }

    #[test]
    fn bare_field_with_filter() {
        #[derive(Debug, Require)]
        struct TestBareFilter {
            #[require(filter = |f| f.idx == 2)]
            target_foo: enumerated::Foo,
        }

        #[derive(Debug, Require)]
        struct TestBareFilterFail {
            #[require(filter = |f| f.idx == 99)]
            _missing_foo: enumerated::Foo, // Should error because 99 doesn't exist
        }

        let assets = Assets::new();
        
        // Success case
        let result = TestBareFilter::require(&assets).expect("Failed to build TestBareFilter");
        assert_eq!(result.target_foo.idx, 2);

        // Failure case (predicate matches nothing)
        let fail_result = TestBareFilterFail::require(&assets);
        assert!(fail_result.is_err(), "Expected an error because no Foo has idx 99");
        
        if let Err(MeetRequirementError::DependencyNotAvailable(name)) = fail_result {
            assert!(name.contains("Foo"));
        } else {
            panic!("Wrong error type returned");
        }
    }

    #[test]
    fn option_field_with_filter() {
    #[derive(Debug, Require)]
        struct TestOptionFilter {
            #[require(filter = |f| f.idx == 3)]
            target_foo: Option<enumerated::Foo>,
            
            #[require(filter = |f| f.idx == 99)]
            missing_foo: Option<enumerated::Foo>, // Should silently become None
        }

        let assets = Assets::new();
        let result = TestOptionFilter::require(&assets).expect("Failed to build TestOptionFilter");

        // Should find the specific target
        assert_eq!(result.target_foo.unwrap().idx, 3);
        
        // Should swallow the error and return None because nothing matches 99
        assert!(result.missing_foo.is_none());
    }

    #[test]
    fn vec_field_with_filter() {
        #[derive(Debug, Require)]
        struct TestVecFilter {
            #[require(filter = |f| f.idx % 2 == 0)]
            even_foos: Vec<enumerated::Foo>,
        }

        let assets = Assets::new();
        let result = TestVecFilter::require(&assets).expect("Failed to build TestVecFilter");

        // Should only collect Foos with even indices
        assert_eq!(result.even_foos.len(), 3);
        assert_eq!(result.even_foos[0].idx, 0);
        assert_eq!(result.even_foos[1].idx, 2);
        assert_eq!(result.even_foos[2].idx, 4);
    }
}
