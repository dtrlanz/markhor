use crate::extension::Extension;

/// Used to do value-to-value comparisons where two values are combined to produce a single value.
/// 
/// This trait is similar to `From` but allows specifying additional requirements to be combined
/// with the source value to produce the target value. In the context of dependency resolution, 
/// these requirements are typically dependencies that must be provided at runtime.
pub trait BuildFrom<T> {
    type Requirements;

    fn build_from(source: T, requirements: Self::Requirements) -> Self;
}

// It would be nice to have the following two general implementations. They would replace
// the more specific implementations with `Extension` below. Unfortunately, they are not
// possible without specialization. So they are commented out for now.

// impl<T, U> BuildFrom<T> for U
// where
//     U: From<T>,
// {
//     type Requirements = ();

//     fn build_from(source: T, _requirements: Self::Requirements) -> Self {
//         U::from(source)
//     }
// }

// impl<T> BuildFrom<()> for T {
//     type Requirements = T;

//     fn build_from(_source: (), requirements: Self::Requirements) -> Self {
//         requirements
//     }
// }

// These are boring by comparison:

impl<E: Extension> BuildFrom<E> for E {
    type Requirements = ();

    fn build_from(source: E, _requirements: Self::Requirements) -> Self {
        source
    }
}

impl<E: Extension> BuildFrom<()> for E {
    type Requirements = E;

    fn build_from(_source: (), requirements: Self::Requirements) -> Self {
        requirements
    }
}