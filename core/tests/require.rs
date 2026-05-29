use markhor_core::dependencies::{Session, Resolve, ResolveDependencyError};


#[test]
fn resolve_simple_struct() {
    #[derive(Debug, PartialEq, Eq, Resolve)]
    struct Foo {
        bar: Bar,
    }

    #[derive(Debug, PartialEq, Eq, Resolve)]
    struct Bar;

    let assets = Session::new();
    let foo = Foo::first(&assets).unwrap();
    assert_eq!(foo.bar, Bar);
}


#[cfg(test)]
mod hygiene_tests {
    use super::*;

    // A simple dependency we can use in our malicious structs
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub struct Dummy {
        pub value: usize,
    }

    impl Resolve for Dummy {
        type Item = Dummy;

        fn iter(_session: &Session) -> Result<impl Iterator<Item = Self>, ResolveDependencyError> {
            Ok(std::iter::once(Dummy { value: 42 }))
        }

        fn iter_from_items<I>(items: I) -> Result<impl Iterator<Item = Self>, ResolveDependencyError>
        where
            I: Iterator<Item = Self::Item>
        {
            Ok(items)
        }
    }

    // =========================================================================
    // 1. FUNCTION ARGUMENT SHADOWING
    // =========================================================================
    mod argument_shadowing {
        use super::*;

        #[allow(dead_code)]
        #[derive(Debug, Resolve)]
        pub struct ShadowSession {
            // Does this shadow the `session: &Session` argument for the next field?
            pub session: Dummy,
            
            // If `session` was shadowed above, `<Dummy as Resolve>::iter(session)?` 
            // will try to pass `Dummy` instead of `&Session`!
            pub next_field: Dummy,
        }

        #[allow(dead_code)]
        #[derive(Debug, Resolve)]
        pub struct ShadowSessionEach {
            // Same as above, but creates `let session = ...` as an iterator or Vec!
            #[resolve(each)]
            pub session: Dummy,
            
            pub next_field: Dummy,
        }
    }

    // =========================================================================
    // 2. INTERNAL MACRO VARIABLE COLLISION
    // =========================================================================
    mod internal_variables {
        use super::*;

        // We try to use field names that match the macro's internal `let __x` bindings.
        #[allow(dead_code)]
        #[derive(Debug, Resolve)]
        pub struct InternalShadowing {
            pub __iter: Dummy,
            pub __items: Dummy,
            pub __field_iter: Dummy,
            pub __key_fn: Dummy,
            pub __item: Dummy,
            pub __k: Dummy,
            pub __vec: Dummy,
            
            // Does the macro use `results` anywhere still? Let's check.
            pub results: Dummy,
            
            // Tuple struct internal field names (`__field_0`, `__field_1`)
            pub __field_0: Dummy,
        }

        // What if we use `map` and our argument name matches a macro internal?
        #[allow(dead_code)]
        #[derive(Debug, Resolve)]
        pub struct MapArgumentCollision {
            #[resolve(map = |__item: Dummy| Dummy { value: __item.value + 1 })]
            pub dummy: Dummy,
        }
    }

    // =========================================================================
    // 3. GENERIC PARAMETER COLLISION
    // =========================================================================
    mod generic_collisions {
        use super::*;
        use std::marker::PhantomData;

        // The macro generates helper functions with generics like `__I`, `__K`, `__V`, `__F`.
        // What if the struct ITSELF uses those generic names?
        #[allow(dead_code)]
        #[derive(Debug, Resolve)]
        pub struct GenericShadowing<__I, __K, __V, __F> {
            pub dummy: Dummy,
            pub _marker: PhantomData<(__I, __K, __V, __F)>,
        }
    }

    // =========================================================================
    // 4. PRELUDE & STANDARD LIBRARY HIJACKING
    // =========================================================================
    mod std_hijacking {
        use super::*;

        // Let's redefine standard library names the macro might rely on without absolute paths.
        
        #[allow(dead_code, non_upper_case_globals)]
        const Ok: () = (); // Hijack `Ok`
        
        #[allow(dead_code)]
        enum Result { // Hijack `Result`
            SomethingElse,
        }
        
        #[allow(dead_code)]
        mod std { // Hijack `std` itself!
            pub mod iter {
                pub trait Iterator {}
            }
        }

        #[allow(dead_code)]
        #[derive(Debug, Resolve)]
        pub struct PreludeHijacked {
            // Will fail if macro uses `Ok(...)` instead of `::std::result::Result::Ok(...)`
            // Will fail if macro uses `std::iter::...` instead of `::std::iter::...`
            pub dummy: Dummy,
        }
    }

    // =========================================================================
    // 5. TRAIT SHADOWING
    // =========================================================================
    mod trait_shadowing {
        use super::*;

        // Hijack the names of the traits the macro relies on being in scope or uses relative paths for.
        #[allow(dead_code)]
        trait Iterator {}
        #[allow(dead_code)]
        trait IntoIterator {}
        #[allow(dead_code)]
        trait FnMut {}

        #[allow(dead_code)]
        #[derive(Debug, Resolve)]
        pub struct TraitHijacked {
            #[resolve(map = |d: Dummy| d)]
            pub dummy: Dummy,
        }
    }

    // =========================================================================
    // 6. MULTIPLE KEY TRAIT DEFINITIONS
    // =========================================================================
    mod multiple_keys {
        use super::*;
        use std::collections::HashMap;

        #[allow(dead_code)]
        #[derive(Debug, Resolve)]
        pub struct DoubleKey {
            // The macro creates `trait __ResolveKeyTupleExtractor { ... }`.
            // If it creates it twice in the same scope, this will fail!
            #[resolve(key = |d| d.value)]
            pub dummy_map_1: HashMap<usize, Dummy>,
            
            #[resolve(key = |d| d.value)]
            pub dummy_map_2: HashMap<usize, Dummy>,
        }
    }

    // =========================================================================
    // 7. MISSING SCOPE (EXTERN CRATE HYGIENE)
    // =========================================================================
    mod missing_error_type {
        #[allow(unused_imports)]
        use super::Session;
        use super::Resolve;
        // Notice we explicitly DO NOT import `ResolveDependencyError` here!
        
        #[allow(dead_code)]
        #[derive(Debug, Resolve)]
        pub struct MissingErrorImport {
            // If the macro generates `ResolveDependencyError::DependencyNotAvailable`
            // without a `crate::` or `super::` prefix, this will fail to compile.
            pub dummy: super::Dummy,
        }
    }

    // =========================================================================
    // 8. RENAMED IMPORTS (MACRO, TRAIT, AND TYPES)
    // =========================================================================
    mod renamed_imports {
        // We explicitly DO NOT use `super::*` so the original names aren't in scope.
        use super::Resolve as RenamedResolve;
        #[allow(unused_imports)]
        use super::Session as RenamedSession;
        #[allow(unused_imports)]
        use super::ResolveDependencyError as RenamedError;
        use super::Dummy;

        #[allow(dead_code)]
        #[derive(Debug, RenamedResolve)]
        pub struct RenamedMacroUsage {
            // Will fail because macro generates `impl Resolve for RenamedMacroUsage`
            // instead of using the path the trait was actually imported under (or an absolute path).
            
            // Will fail because macro generates `fn iter(session: &Session)`
            // but `Session` is not in scope!
            
            // Will fail because macro calls `<Dummy as Resolve>::iter(session)`
            // but `Resolve` is not in scope!
            pub dummy: Dummy,
        }
    }
}