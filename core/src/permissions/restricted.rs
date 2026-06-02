use crate::permissions::{Authorized, Permission};



pub trait Restricted {
    fn permissions_required(&self) -> &[Permission];

    fn is_accessible_by<T: Authorized + ?Sized>(&self, actor: &T) -> bool {
        actor.may_access(self)
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
