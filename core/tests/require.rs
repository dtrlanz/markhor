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
