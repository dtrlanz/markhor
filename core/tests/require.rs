use markhor_core::dependencies::{Assets, Require, MeetRequirementError};


#[test]
fn require_simple_struct() {
    #[derive(Debug, PartialEq, Eq, Require)]
    struct Foo {
        bar: Bar,
    }

    #[derive(Debug, PartialEq, Eq, Require)]
    struct Bar;

    let assets = Assets::new();
    let foo = Foo::require(&assets).unwrap();
    assert_eq!(foo.bar, Bar);
}
