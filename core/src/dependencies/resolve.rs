use std::{any::type_name, hash::Hash};
use thiserror::Error;
use tracing::warn;

use crate::dependencies::Session;

pub use derive_resolve::Resolve;

pub trait Resolve {
    type Item;

    fn first(session: &Session) -> Result<Self, ResolveDependencyError> 
    where 
        Self: Sized
    {
        Self::iter(session)?
            .next()
            .ok_or_else(|| {
                // Get name of the missing struct
                let name = type_name::<Self>().to_string();
                ResolveDependencyError::DependencyNotAvailable(name)
            })
    }

    fn iter(session: &Session) -> Result<impl Iterator<Item = Self>, ResolveDependencyError>;

    fn iter_from_items<I>(items: I) -> Result<impl Iterator<Item = Self>, ResolveDependencyError>
    where
        I: Iterator<Item = Self::Item>;
}

impl<T> Resolve for std::marker::PhantomData<T> {
    type Item = Self;

    fn iter(_session: &Session) -> Result<impl Iterator<Item = Self>, ResolveDependencyError> 
    {
        Ok(std::iter::once(std::marker::PhantomData))
    }

    fn iter_from_items<I>(items: I) -> Result<impl Iterator<Item = Self>, ResolveDependencyError>
    where
        I: Iterator<Item = Self::Item>
    {
        let iter = items.map(|_| std::marker::PhantomData);
        Ok(iter)
    }
}

impl<T: Resolve> Resolve for Option<T> {
    type Item = T;

    fn iter(session: &Session) -> Result<impl Iterator<Item = Self>, ResolveDependencyError> 
    {
        let items = T::iter(session)?;
        Self::iter_from_items(items)
    }

    fn iter_from_items<I>(items: I) -> Result<impl Iterator<Item = Self>, ResolveDependencyError>
    where
        I: Iterator<Item = Self::Item>,
    {
        let iter = OnceOrMore::new(items);
        Ok(iter)
    }
}

/// Iterator that yields at least one item.
/// 
/// Helper struct used by `impl<T: Resolve> Resolve for Option<T>`.
/// Yields items of type `Option<T>`, always yielding  `Some` at least once.
/// 
/// - If the original iterator is empty: `Some(None)`, `None`, `None`, ...
/// - If the original iterator has items: `Some(Some(item1))`, `Some(Some(item2))`, ..., `None`, `None`, ...
struct OnceOrMore<T, I: Iterator<Item = T>>(Option<Option<T>>, I);

impl<T, I: Iterator<Item = T>> OnceOrMore<T, I> {
    fn new(mut items: I) -> Self {
        let first = items.next();
        Self(Some(first), items)
    }
}

impl<T, I: Iterator<Item = T>> Iterator for OnceOrMore<T, I> {
    type Item = Option<T>;

    fn next(&mut self) -> Option<Self::Item> {
        if let Some(item) = self.0.take() {
            Some(item)
        } else {
            self.1.next().map(Some)
        }
    }
}

impl<T: Resolve> Resolve for Vec<T> {
    type Item = T;

    fn iter(session: &Session) -> Result<impl Iterator<Item = Self>, ResolveDependencyError> 
    {
        let items = T::iter(session)?;
        Self::iter_from_items(items)
    }

    fn iter_from_items<I>(items: I) -> Result<impl Iterator<Item = Self>, ResolveDependencyError>
    where
        I: Iterator<Item = Self::Item>,
    {
        let vec = items.collect();
        Ok(std::iter::once(vec))
    }
}

impl<K, V> Resolve for std::collections::HashMap<K, V>
where
    K: Eq + Hash,
{
    type Item = (K, V);

    fn iter(_session: &Session) -> Result<impl Iterator<Item = Self>, ResolveDependencyError> 
    {
        // TODO: It would probably be better to introduce a specific error variant to be returned
        // in such cases. Or we could panic.
        // Regardless, the solution is not ideal. It would be much more Rustaceous to get a 
        // compiler error.
        warn!("Keys cannot be resolved, so HashMap will be empty. Use a mapping function to generate key-value tuples.");
        Self::iter_from_items(std::iter::empty())
    }

    fn iter_from_items<I>(items: I) -> Result<impl Iterator<Item = Self>, ResolveDependencyError>
    where
        I: Iterator<Item = Self::Item>
    {
        let map = items.collect();
        Ok(std::iter::once(map))
    }
}

#[derive(Debug, Error)]
pub enum ResolveDependencyError {
    #[error("Missing dependency: {0}")]
    DependencyNotAvailable(String),
}


#[cfg(test)]
mod tests {
    use std::fmt::Debug;

use super::*;

    mod simple {
        use super::*;

        #[derive(Debug, PartialEq, Eq, Resolve)]
        #[resolve(crate = "crate")]
        pub struct Foo {
            pub bar: Bar,
            pub baz: Baz,
        }

        #[derive(Debug, PartialEq, Eq, Resolve)]
        #[resolve(crate = "crate")]
        pub struct FooTuple(pub Bar, pub Baz);

        #[derive(Debug, PartialEq, Eq, Resolve)]
        #[resolve(crate = "crate")]
        pub struct Bar;

        #[derive(Debug, PartialEq, Eq, Resolve)]
        #[resolve(crate = "crate")]
        pub struct Baz;
    }

    #[test]
    fn derive_resolve() {
        let session = Session::new();

        // --- Classic struct ---

        let foo = simple::Foo::first(&session).unwrap();
        assert_eq!(foo.bar, simple::Bar);
        assert_eq!(foo.baz, simple::Baz);

        let vec = simple::Foo::iter(&session).unwrap().collect::<Vec<_>>();
        assert_eq!(vec.len(), 1);
        assert_eq!(vec[0].bar, simple::Bar);
        assert_eq!(vec[0].baz, simple::Baz);

        // --- Tuple struct ---
        
        let foo_tuple = simple::FooTuple::first(&session).unwrap();
        assert_eq!(foo_tuple.0, simple::Bar);
        assert_eq!(foo_tuple.1, simple::Baz);

        let vec = simple::FooTuple::iter(&session).unwrap().collect::<Vec<_>>();
        assert_eq!(vec.len(), 1);
        assert_eq!(vec[0].0, simple::Bar);
        assert_eq!(vec[0].1, simple::Baz);
    }

    #[test]
    fn once_or_more_iterator() {
        let mut empty_iter = OnceOrMore::new(std::iter::empty::<i32>());
        assert_eq!(empty_iter.next(), Some(None));
        assert_eq!(empty_iter.next(), None);

        let mut some_iter = OnceOrMore::new(vec![1, 2].into_iter());
        assert_eq!(some_iter.next(), Some(Some(1)));
        assert_eq!(some_iter.next(), Some(Some(2)));
        assert_eq!(some_iter.next(), None);
    }

    mod enumerated {
        use super::*;

        #[derive(Debug, Clone, PartialEq, Eq)]
        pub struct Foo {
            pub idx: usize,
            pub bar: Bar,
        }

        impl Resolve for Foo {
            type Item = Self;

            fn iter(session: &Session) -> Result<impl Iterator<Item = Self>, ResolveDependencyError> 
            {
                let items = (0..5).map(|idx| {
                    let bar = Bar::first(session).unwrap();
                    Self { idx, bar }
                });
                Self::iter_from_items(items)
            }

            fn iter_from_items<I>(items: I) -> Result<impl Iterator<Item = Self>, ResolveDependencyError>
            where
                I: Iterator<Item = Self::Item>
            {
                Ok(items)
            }
        }

        #[derive(Debug, Clone, PartialEq, Eq, Resolve)]
        #[resolve(crate = "crate")]
        pub struct Bar;

        #[derive(Debug, PartialEq, Eq)]
        pub struct Blank;

        impl Resolve for Blank {
            type Item = Self;

            fn iter(_assets: &Session) -> Result<impl Iterator<Item = Self>, ResolveDependencyError>
            {
                Ok(std::iter::empty())
            }

            fn iter_from_items<I>(items: I) -> Result<impl Iterator<Item = Self>, ResolveDependencyError>
            where
                I: Iterator<Item = Self::Item>
            {
                Ok(items)
            }
        }
    }

    #[test]
    fn vec_and_option_without_filters() {
        // --- Classic struct ---

        #[derive(Debug, Resolve)]
        #[resolve(crate = "crate")]
        struct TestNoFilter0 {
            all_foos: Vec<enumerated::Foo>,
            first_foo: Option<enumerated::Foo>,
            missing_blank: Option<enumerated::Blank>, // Should silently become None
        }

        let session = Session::new();
        let result = TestNoFilter0::first(&session).expect("Failed to build TestNoFilter0");

        // Vec should collect all 5 Foos
        assert_eq!(result.all_foos.len(), 5);
        
        // Option should just grab the first one (idx 0)
        assert_eq!(result.first_foo.unwrap().idx, 0);
        
        // Option<Blank> should swallow the Blank error and return None
        assert!(result.missing_blank.is_none());

        // --- Tuple struct ---

        #[derive(Debug, Resolve)]
        #[resolve(crate = "crate")]
        struct TestNoFilter1 (
            Vec<enumerated::Foo>,
            Option<enumerated::Foo>,
            Option<enumerated::Blank>,
        );
        
        let result = TestNoFilter1::first(&session).expect("Failed to build TestNoFilter1");

        // Same assertions as above
        assert_eq!(result.0.len(), 5);
        assert_eq!(result.0[0].idx, 0);
        assert_eq!(result.1.unwrap().idx, 0);
        assert!(result.2.is_none());
    }

    #[test]
    fn bare_field_with_filter() {
        #[derive(Debug, Resolve)]
        #[resolve(crate = "crate")]
        struct TestBareFilter {
            #[resolve(filter = |f| f.idx == 2)]
            target_foo: enumerated::Foo,
        }

        #[derive(Debug, Resolve)]
        #[resolve(crate = "crate")]
        struct TestBareFilterFail {
            #[resolve(filter = |f| f.idx == 99)]
            _missing_foo: enumerated::Foo, // Should error because 99 doesn't exist
        }

        let session = Session::new();
        
        // Success case
        let result = TestBareFilter::first(&session).expect("Failed to build TestBareFilter");
        assert_eq!(result.target_foo.idx, 2);

        // Failure case (predicate matches nothing)
        let fail_result = TestBareFilterFail::first(&session);
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
        #[resolve(crate = "crate")]
        struct TestOptionFilter {
            #[resolve(filter = |f| f.idx == 3)]
            target_foo: Option<enumerated::Foo>,
            
            #[resolve(filter = |f| f.idx == 99)]
            missing_foo: Option<enumerated::Foo>, // Should silently become None
        }

        let session = Session::new();
        let result = TestOptionFilter::first(&session).expect("Failed to build TestOptionFilter");

        // Should find the specific target
        assert_eq!(result.target_foo.unwrap().idx, 3);
        
        // Should swallow the error and return None because nothing matches 99
        assert!(result.missing_foo.is_none());
    }

    #[test]
    fn vec_field_with_filter() {
        #[derive(Debug, Resolve)]
        #[resolve(crate = "crate")]
        struct TestVecFilter {
            #[resolve(filter = |f| f.idx % 2 == 0)]
            even_foos: Vec<enumerated::Foo>,
        }

        let session = Session::new();
        let result = TestVecFilter::first(&session).expect("Failed to build TestVecFilter");

        // Should only collect Foos with even indices
        assert_eq!(result.even_foos.len(), 3);
        assert_eq!(result.even_foos[0].idx, 0);
        assert_eq!(result.even_foos[1].idx, 2);
        assert_eq!(result.even_foos[2].idx, 4);
    }

    #[test]
    fn vec_field_with_map() {
        use enumerated::Foo;

        // --- Map within the same type ---

        #[derive(Debug, Resolve)]
        #[resolve(crate = "crate")]
        struct TestVecMap0 {
            #[resolve(map = |f: Foo| Foo { idx: f.idx * 2, bar: f.bar })]
            doubled_foos: Vec<Foo>,
        }

        let session = Session::new();
        let result = TestVecMap0::first(&session).expect("Failed to build TestVecMap0");

        // Indices should be doubled
        assert_eq!(result.doubled_foos.len(), 5);
        assert_eq!(result.doubled_foos[0].idx, 0);
        assert_eq!(result.doubled_foos[1].idx, 2);
        assert_eq!(result.doubled_foos[2].idx, 4);
        assert_eq!(result.doubled_foos[3].idx, 6);
        assert_eq!(result.doubled_foos[4].idx, 8);

        // --- Map one type to another ---

        #[derive(Debug, Resolve)]
        #[resolve(crate = "crate")]
        struct Baz {
            foo: Foo,
        }

        #[derive(Debug, Resolve)]
        #[resolve(crate = "crate")]
        struct TestVecMap1 {
            #[resolve(map = |f: Foo| Baz { foo: f })]
            bazes: Vec<Baz>,
        }

        let result = TestVecMap1::first(&session).expect("Failed to build TestVecMap1");

        // Foos should be wrapped in Baz structs
        assert_eq!(result.bazes.len(), 5);
        assert_eq!(result.bazes[0].foo.idx, 0);
        assert_eq!(result.bazes[1].foo.idx, 1);
        assert_eq!(result.bazes[2].foo.idx, 2);
        assert_eq!(result.bazes[3].foo.idx, 3);
        assert_eq!(result.bazes[4].foo.idx, 4);
    }

    #[test]
    fn hash_map_field_with_map() {
        use enumerated::Foo;

        #[derive(Debug, Resolve)]
        #[resolve(crate = "crate")]
        struct TestMapMap {
            #[resolve(map = |f: Foo| (f.idx, f))]
            foo_map: std::collections::HashMap<usize, Foo>,
        }

        let session = Session::new();
        let result = TestMapMap::first(&session).expect("Failed to build TestMapMap");

        // Should produce a HashMap mapping indices to Foos
        assert_eq!(result.foo_map.len(), 5);
        for idx in 0..5 {
            assert_eq!(result.foo_map[&idx].idx, idx);
        }
    }

    #[test]
    fn hash_map_field_with_key() {
        use enumerated::Foo;

        // --- Classic structs ---
        
        #[derive(Debug, Resolve)]
        #[resolve(crate = "crate")]
        struct TestMapMap0 {
            // with explicit type annotation
            #[resolve(key = |f: &Foo| f.idx)]
            foo_map: std::collections::HashMap<usize, Foo>,
        }

        let session = Session::new();
        let result = TestMapMap0::first(&session).expect("Failed to build TestMapMap0");

        // Should produce a HashMap mapping indices to Foos
        assert_eq!(result.foo_map.len(), 5);
        for idx in 0..5 {
            assert_eq!(result.foo_map[&idx].idx, idx);
        }

        #[derive(Debug, Resolve)]
        #[resolve(crate = "crate")]
        struct TestMapMap1 {
            // without type annotation
            #[resolve(key = |f| f.idx)]
            foo_map: std::collections::HashMap<usize, Foo>,
        }

        let session = Session::new();
        let result = TestMapMap1::first(&session).expect("Failed to build TestMapMap1");

        // Same assertions as above
        assert_eq!(result.foo_map.len(), 5);
        for idx in 0..5 {
            assert_eq!(result.foo_map[&idx].idx, idx);
        }

        // --- Tuple struct ---

        #[derive(Debug, Resolve)]
        #[resolve(crate = "crate")]
        struct TestMapMap2 (
            #[resolve(key = |f| f.idx)]
            std::collections::HashMap<usize, Foo>,
        );

        let result = TestMapMap2::first(&session).expect("Failed to build TestMapMap2");

        // Same assertions as above
        assert_eq!(result.0.len(), 5);
        for idx in 0..5 {
            assert_eq!(result.0[&idx].idx, idx);
        }
    }

    #[test]
    fn sort_by() {
        use enumerated::Foo;

        #[derive(Debug, Resolve)]
        #[resolve(crate = "crate")]
        struct TestSortBy {
            // default is ascending order; reverse it
            #[resolve(sort_by = |a, b| b.idx.cmp(&a.idx))]
            sorted_foos: Vec<Foo>,
        }

        let session = Session::new();
        let result = TestSortBy::first(&session).expect("Failed to build TestSortBy");

        // Indices should be sorted in descending order
        assert_eq!(result.sorted_foos.len(), 5);
        let indices = result.sorted_foos.iter().map(|f| f.idx).collect::<Vec<_>>();
        assert_eq!(indices, vec![4, 3, 2, 1, 0]);
    }

    #[derive(Debug, Clone, PartialEq, Eq)]
    pub struct Letter {
        pub ch: char,
    }

    impl Resolve for Letter {
        type Item = Self;

        fn iter(_assets: &Session) -> Result<impl Iterator<Item = Self>, ResolveDependencyError>
        {
            // Yields exactly two letters
            let letters = vec![
                Self { ch: 'A' },
                Self { ch: 'B' },
            ];
            Self::iter_from_items(letters.into_iter())
        }

        fn iter_from_items<I>(items: I) -> Result<impl Iterator<Item = Self>, ResolveDependencyError>
        where
            I: Iterator<Item = Self::Item>
        {
            Ok(items)
        }
    }

    #[test]
    fn single_each() {
        #[derive(Debug, Resolve)]
        #[resolve(crate = "crate")]
        struct SingleEach {
            // Outer loop: 5 iterations
            #[resolve(each)]
            foo: enumerated::Foo,
            
            // Standard field: Fetched freshly on every iteration. 
            bar: enumerated::Bar, 
        }


        let session = Session::new();
        
        // Resolve iterator should yield multiple items
        let results: Vec<_> = SingleEach::iter(&session)
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
        #[resolve(crate = "crate")]
        struct CartesianEach {
            // Outer loop: 2 iterations
            #[resolve(each)]
            letter: Letter,
            
            // Inner loop: 5 iterations
            #[resolve(each)]
            foo: enumerated::Foo,
        }


        let session = Session::new();
        
        let results: Vec<_> = CartesianEach::iter(&session)
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
        #[resolve(crate = "crate")]
        struct FilteredEach {
            // Filtered loop: Only yields 3 items (idx 0, 2, 4)
            #[resolve(each, filter = |f: &enumerated::Foo| f.idx % 2 == 0)]
            even_foo: enumerated::Foo,
            
            // Inner loop: 2 iterations
            #[resolve(each)]
            letter: Letter,
        }

        let session = Session::new();
        
        let results: Vec<_> = FilteredEach::iter(&session)
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