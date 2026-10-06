/// Used to do value-to-value comparisons where two values are combined to produce a single value.
/// 
/// This trait is similar to `From` but allows specifying additional requirements to be combined
/// with the source value to produce the target value. In the context of dependency resolution, 
/// these requirements are typically dependencies that must be provided at runtime.
pub trait BuildFrom<T> {
    type Requirements;

    fn build_from(source: T, requirements: Self::Requirements) -> Self;
}