use crate::permissions::{Authorized, Permission};

pub trait Restricted {
    fn permissions_required(&self) -> &[Permission];

    fn is_accessible_by<T: Authorized + ?Sized>(&self, actor: &T) -> bool {
        actor.may_access(self)
    }
}

/// Wrapper for using methods of the `Restricted` trait on any type that does not implement it
/// directly, so long as it implements `AsRef<[Permission]>`.
/// 
/// This can be useful if you have a `Vec<Permission>` and need a convenient way to check whether
/// that set of permissions could be accessed to a type implementing `Authorized`.
pub struct Resource<T: AsRef<[Permission]>>(pub T);

impl<T: AsRef<[Permission]>> Restricted for Resource<T> {
    fn permissions_required(&self) -> &[Permission] {
        self.0.as_ref()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::permissions::{Actor, NOT_USED_FOR_TRAINING, ON_DEVICE, PUBLIC};

    #[test]
    fn access_control() {
        let resource = Resource(vec![NOT_USED_FOR_TRAINING]);
        let actors = vec![
            Actor(vec![ON_DEVICE]),
            Actor(vec![NOT_USED_FOR_TRAINING]),
            Actor(vec![PUBLIC]),
        ];

        assert!(resource.is_accessible_by(&actors[0]));
        assert!(resource.is_accessible_by(&actors[1]));
        assert!(!resource.is_accessible_by(&actors[2]));
    }
}
