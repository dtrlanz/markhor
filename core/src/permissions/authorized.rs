use crate::permissions::{Permission, Restricted};

pub trait Authorized {
    fn permissions_granted(&self) -> &[Permission];

    fn has_permission(&self, permission: &Permission) -> bool {
        self.permissions_granted().iter().any(|p| p >= permission)
    }

    fn may_access<T: Restricted + ?Sized>(&self, resource: &T) -> bool {
        resource
            .permissions_required()
            .iter()
            .all(|p| self.has_permission(p))
    }
}

/// Wrapper for using methods of the `Authorized` trait on any type that does not implement it
/// directly, so long as it implements `AsRef<[Permission]>`.
///
/// This can be useful if you have a `Vec<Permission>` and need a convenient way to check whether
/// that set of permissions would grant access to a type implementing `Restricted`.
pub struct Actor<T: AsRef<[Permission]>>(pub T);

impl<T: AsRef<[Permission]>> Authorized for Actor<T> {
    fn permissions_granted(&self) -> &[Permission] {
        self.0.as_ref()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::permissions::{NOT_USED_FOR_TRAINING, ON_DEVICE, PUBLIC, Resource};

    #[test]
    fn access_control() {
        let actor = Actor(vec![NOT_USED_FOR_TRAINING]);
        let resources = vec![
            Resource(vec![ON_DEVICE]),
            Resource(vec![NOT_USED_FOR_TRAINING]),
            Resource(vec![PUBLIC]),
        ];

        assert!(!actor.may_access(&resources[0]));
        assert!(actor.may_access(&resources[1]));
        assert!(actor.may_access(&resources[2]));
    }
}
