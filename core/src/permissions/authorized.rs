use crate::permissions::{Permission, Restricted};



pub trait Authorized {
    fn permissions_granted(&self) -> &[Permission];

    fn has_permission(&self, permission: &Permission) -> bool {
        self.permissions_granted().iter().any(|p| p >= permission)
    }

    fn may_access<T: Restricted + ?Sized>(&self, resource: &T) -> bool {
        resource.permissions_required().iter().all(|p| self.has_permission(p))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::permissions::{ON_DEVICE, NOT_USED_FOR_TRAINING, PUBLIC};

    struct Actor(Vec<Permission>);

    impl Authorized for Actor {
        fn permissions_granted(&self) -> &[Permission] {
            &self.0
        }
    }

    struct Resource(Vec<Permission>);

    impl Restricted for Resource {
        fn permissions_required(&self) -> &[Permission] {
            &self.0
        }
    }

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
