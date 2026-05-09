use std::any::type_name;
use thiserror::Error;

use crate::dependencies::Session;

pub use derive_resolve::Resolve;

pub trait Resolve {
    fn resolve(session: &Session) -> Result<Self, ResolveDependencyError> 
    where 
        Self: Sized
    {
        Self::resolve_iter(session)?
            .next()
            .ok_or_else(|| {
                // Get name of the missing struct
                let name = type_name::<Self>().to_string();
                ResolveDependencyError::DependencyNotAvailable(name)
            })
    }

    fn resolve_iter(session: &Session) -> Result<impl Iterator<Item = Self>, ResolveDependencyError> 
    where 
        Self: Sized;
}


#[derive(Debug, Error)]
pub enum ResolveDependencyError {
    #[error("Missing dependency: {0}")]
    DependencyNotAvailable(String),
}


#[cfg(test)]
mod tests {
    use super::*;

    mod simple {
        use super::*;

        #[derive(Debug, PartialEq, Eq, Resolve)]
        pub struct Foo {
            pub bar: Bar,
            pub baz: Baz,
        }

        #[derive(Debug, PartialEq, Eq, Resolve)]
        pub struct Bar;

        #[derive(Debug, PartialEq, Eq, Resolve)]
        pub struct Baz;
    }    

    #[test]
    fn derive_resolve() {
        let session = Session::new();
        let foo = simple::Foo::resolve(&session).unwrap();
        assert_eq!(foo.bar, simple::Bar);
        assert_eq!(foo.baz, simple::Baz);

        let vec = simple::Foo::resolve_iter(&session).unwrap().collect::<Vec<_>>();
        assert_eq!(vec.len(), 1);
        assert_eq!(vec[0].bar, simple::Bar);
        assert_eq!(vec[0].baz, simple::Baz);
    }

    mod enumerated {
        use super::*;

        #[derive(Debug, Clone, PartialEq, Eq)]
        pub struct Foo {
            pub idx: usize,
            pub bar: Bar,
        }

        impl Resolve for Foo {
            fn resolve_iter(session: &Session) -> Result<impl Iterator<Item = Self>, ResolveDependencyError> {
                let vec = (0..5)
                    .map(|idx| {
                        let bar = Bar::resolve(session).unwrap();
                        Self { idx, bar }
                    })
                    .collect::<Vec<_>>();
                Ok(vec.into_iter())
            }
        }

        #[derive(Debug, Clone, PartialEq, Eq, Resolve)]
        pub struct Bar;

        #[derive(Debug, PartialEq, Eq)]
        pub struct Blank;

        impl Resolve for Blank {
            fn resolve_iter(_assets: &Session) -> Result<impl Iterator<Item = Self>, ResolveDependencyError> {
                Result::<std::iter::Empty<Self>, _>::Err(
                    ResolveDependencyError::DependencyNotAvailable("Blank".to_string())
                )
            }
        }
    }

    #[test]
    fn vec_and_option_without_filters() {
        #[derive(Debug, Resolve)]
        struct TestNoFilter {
            all_foos: Vec<enumerated::Foo>,
            first_foo: Option<enumerated::Foo>,
            missing_blank: Option<enumerated::Blank>, // Should silently become None
        }

        let session = Session::new();
        let result = TestNoFilter::resolve(&session).expect("Failed to build TestNoFilter");

        // Vec should collect all 5 Foos
        assert_eq!(result.all_foos.len(), 5);
        
        // Option should just grab the first one (idx 0)
        assert_eq!(result.first_foo.unwrap().idx, 0);
        
        // Option<Blank> should swallow the Blank error and return None
        assert!(result.missing_blank.is_none());
    }

    #[test]
    fn bare_field_with_filter() {
        #[derive(Debug, Resolve)]
        struct TestBareFilter {
            #[resolve(filter = |f| f.idx == 2)]
            target_foo: enumerated::Foo,
        }

        #[derive(Debug, Resolve)]
        struct TestBareFilterFail {
            #[resolve(filter = |f| f.idx == 99)]
            _missing_foo: enumerated::Foo, // Should error because 99 doesn't exist
        }

        let session = Session::new();
        
        // Success case
        let result = TestBareFilter::resolve(&session).expect("Failed to build TestBareFilter");
        assert_eq!(result.target_foo.idx, 2);

        // Failure case (predicate matches nothing)
        let fail_result = TestBareFilterFail::resolve(&session);
        assert!(fail_result.is_err(), "Expected an error because no Foo has idx 99");
        
        if let Err(ResolveDependencyError::DependencyNotAvailable(name)) = fail_result {
            assert!(name.contains("Foo"));
        } else {
            panic!("Wrong error type returned");
        }
    }

    #[test]
    fn option_field_with_filter() {
    #[derive(Debug, Resolve)]
        struct TestOptionFilter {
            #[resolve(filter = |f| f.idx == 3)]
            target_foo: Option<enumerated::Foo>,
            
            #[resolve(filter = |f| f.idx == 99)]
            missing_foo: Option<enumerated::Foo>, // Should silently become None
        }

        let session = Session::new();
        let result = TestOptionFilter::resolve(&session).expect("Failed to build TestOptionFilter");

        // Should find the specific target
        assert_eq!(result.target_foo.unwrap().idx, 3);
        
        // Should swallow the error and return None because nothing matches 99
        assert!(result.missing_foo.is_none());
    }

    #[test]
    fn vec_field_with_filter() {
        #[derive(Debug, Resolve)]
        struct TestVecFilter {
            #[resolve(filter = |f| f.idx % 2 == 0)]
            even_foos: Vec<enumerated::Foo>,
        }

        let session = Session::new();
        let result = TestVecFilter::resolve(&session).expect("Failed to build TestVecFilter");

        // Should only collect Foos with even indices
        assert_eq!(result.even_foos.len(), 3);
        assert_eq!(result.even_foos[0].idx, 0);
        assert_eq!(result.even_foos[1].idx, 2);
        assert_eq!(result.even_foos[2].idx, 4);
    }

    #[derive(Debug, Clone, PartialEq, Eq)]
    pub struct Letter {
        pub ch: char,
    }

    impl Resolve for Letter {
        fn resolve_iter(_assets: &Session) -> Result<impl Iterator<Item = Self>, ResolveDependencyError> {
            // Yields exactly two letters
            Ok(vec![Letter { ch: 'A' }, Letter { ch: 'B' }].into_iter())
        }
    }

    #[test]
    fn single_each() {
    #[derive(Debug, Resolve)]
        struct SingleEach {
            // Outer loop: 5 iterations
            #[resolve(each)]
            foo: enumerated::Foo,
            
            // Standard field: Fetched freshly on every iteration. 
            bar: enumerated::Bar, 
        }


        let session = Session::new();
        
        // Resolve iterator should yield multiple items
        let results: Vec<_> = SingleEach::resolve_iter(&session)
            .expect("Failed to build SingleEach")
            .collect();

        // Foo yields 5 items, so we expect 5 SingleEach instances
        assert_eq!(results.len(), 5);
        
        // Verify the loop executed correctly
        assert_eq!(results[0].foo.idx, 0);
        assert_eq!(results[4].foo.idx, 4);
        
        // Verify standard fields are present
        assert_eq!(results[0].bar, enumerated::Bar);
    }

    #[test]
    fn cartesian_each() {
        #[derive(Debug, Resolve)]
        struct CartesianEach {
            // Outer loop: 2 iterations
            #[resolve(each)]
            letter: Letter,
            
            // Inner loop: 5 iterations
            #[resolve(each)]
            foo: enumerated::Foo,
        }


        let session = Session::new();
        
        let results: Vec<_> = CartesianEach::resolve_iter(&session)
            .expect("Failed to build CartesianEach")
            .collect();

        // 2 Letters * 5 Foos = 10 combinations
        assert_eq!(results.len(), 10);

        // Because `letter` is defined first, it forms the OUTER loop.
        // Therefore, we expect all 5 'A's, followed by all 5 'B's.
        assert_eq!(results[0].letter.ch, 'A');
        assert_eq!(results[0].foo.idx, 0);
        
        assert_eq!(results[4].letter.ch, 'A');
        assert_eq!(results[4].foo.idx, 4);

        assert_eq!(results[5].letter.ch, 'B');
        assert_eq!(results[5].foo.idx, 0);
        
        assert_eq!(results[9].letter.ch, 'B');
        assert_eq!(results[9].foo.idx, 4);
    }

    #[test]
    fn filtered_each() {
    #[derive(Debug, Resolve)]
        struct FilteredEach {
            // Filtered loop: Only yields 3 items (idx 0, 2, 4)
            #[resolve(each, filter = |f: &enumerated::Foo| f.idx % 2 == 0)]
            even_foo: enumerated::Foo,
            
            // Inner loop: 2 iterations
            #[resolve(each)]
            letter: Letter,
        }

        let session = Session::new();
        
        let results: Vec<_> = FilteredEach::resolve_iter(&session)
            .expect("Failed to build FilteredEach")
            .collect();

        // 3 Even Foos * 2 Letters = 6 combinations
        assert_eq!(results.len(), 6);

        // Because `even_foo` is defined first, it forms the OUTER loop.
        // Therefore, we expect idx 0 (with A and B), then idx 2 (with A and B), etc.
        assert_eq!(results[0].even_foo.idx, 0);
        assert_eq!(results[0].letter.ch, 'A');

        assert_eq!(results[1].even_foo.idx, 0);
        assert_eq!(results[1].letter.ch, 'B');

        assert_eq!(results[2].even_foo.idx, 2);
        assert_eq!(results[2].letter.ch, 'A');

        assert_eq!(results[5].even_foo.idx, 4);
        assert_eq!(results[5].letter.ch, 'B');
    }
}