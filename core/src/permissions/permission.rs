use std::cmp::Ordering;

/// Permission that can be used to control access to resources
/// 
/// Permissions may be applied to resources and actors. Resources with a given permission can only
/// be accessed by actors with that permission or a higher one.
/// 
/// See module-level documentation for more details.
#[derive(Debug, Clone, Eq, Hash)]
pub struct Permission {
    name: &'static str,
    resource_descr: Option<&'static str>,
    actor_descr: Option<&'static str>,
    entailed: &'static [&'static Permission],
}

impl Permission {
    /// Permission name
    /// 
    /// Must be unique across all permissions.
    pub fn name(&self) -> &str {
        self.name
    }

    /// Optional description of what this permission means for resources (e.g. documents)
    pub fn resource_description(&self) -> Option<&str> {
        self.resource_descr
    }

    /// Optional description of what this permission means for actors (e.g. models, tools)
    pub fn actor_description(&self) -> Option<&str> {
        self.actor_descr
    }

    /// Permissions that are entailed by this permission
    /// 
    /// For example, if a permission "A" entails "B" and "C", then any actor with permission "A" also has permissions "B" and "C".
    pub fn entailed_permissions(&self) -> &[&Permission] {
        self.entailed
    }

    pub fn insert(vec: &mut Vec<Permission>, permission: Permission) {
        for i in 0..vec.len() {
            match permission.partial_cmp(&vec[i]) {
                Some(Ordering::Less) => return,  // Higher permission already in the list
                Some(Ordering::Equal) => return, // Same permission already in the list
                Some(Ordering::Greater) => {
                    // Lower permissions can be replaced with the higher one
                    let mut j = i + 1;
                    while j < vec.len() {
                        if permission > vec[j] {
                            vec.swap_remove(j);
                        } else {
                            j += 1;
                        }
                    }
                    vec[i] = permission;
                    return;
                }
                None => (), // Uncomparable permissions, keep looking
            }
        }
        vec.push(permission);
    }
}

impl PartialEq for Permission {
    fn eq(&self, other: &Self) -> bool {
        self.name == other.name
    }
}

impl PartialOrd for Permission {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        if self == other {
            Some(Ordering::Equal)
        } else if self.entailed.contains(&other) {
            Some(Ordering::Greater)
        } else if other.entailed.contains(&self) {
            Some(Ordering::Less)
        } else {
            None
        }
    }
}

// Hard-coding a few permissions for proof of concept

pub const ON_DEVICE: Permission = Permission {
    name: "on-device",
    resource_descr: Some("May only be processed locally on the device"),
    actor_descr: Some("Processes data locally on the device"),
    entailed: &[&NOT_USED_FOR_TRAINING, &PUBLIC],
};

pub const NOT_USED_FOR_TRAINING: Permission = Permission {
    name: "not-used-for-training",
    resource_descr: Some("May not be used for training"),
    actor_descr: Some("Does not use data for training"),
    entailed: &[&PUBLIC],
};

pub const PUBLIC: Permission = Permission {
    name: "public",
    resource_descr: Some("Publically accessible data"),
    actor_descr: Some("May access data that is shared publicly"),
    entailed: &[],
};

#[cfg(test)]
pub(crate) const GDPR: Permission = Permission {
    name: "gdpr",
    resource_descr: Some("Subject to GDPR regulations"),
    actor_descr: Some("Complies with GDPR regulations"),
    entailed: &[&PUBLIC],
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn permission_hierarchy() {
        assert!(ON_DEVICE > NOT_USED_FOR_TRAINING);
        assert!(NOT_USED_FOR_TRAINING > PUBLIC);
        assert!(ON_DEVICE > PUBLIC);
        assert!(PUBLIC < NOT_USED_FOR_TRAINING);
        assert!(PUBLIC < ON_DEVICE);

        assert!(ON_DEVICE == ON_DEVICE);
        assert!(NOT_USED_FOR_TRAINING == NOT_USED_FOR_TRAINING);
        assert!(PUBLIC == PUBLIC);
        assert!(ON_DEVICE != NOT_USED_FOR_TRAINING);
        assert!(NOT_USED_FOR_TRAINING != PUBLIC);
        assert!(PUBLIC != ON_DEVICE);

        let unsorted = vec![&NOT_USED_FOR_TRAINING, &PUBLIC, &ON_DEVICE];
        let mut sorted = unsorted.clone();
        sorted.sort_by(|a, b| a.partial_cmp(b).unwrap());
        assert_eq!(sorted, vec![&PUBLIC, &NOT_USED_FOR_TRAINING, &ON_DEVICE]);

        // Comparisons agains GDPR, which is not in the same hierarchy as ON_DEVICE and
        // NOT_USED_FOR_TRAINING
        assert!(GDPR > PUBLIC);
        assert!(GDPR != ON_DEVICE);
        assert!(GDPR != NOT_USED_FOR_TRAINING);
        assert!(!(GDPR <= ON_DEVICE));
        assert!(!(GDPR >= ON_DEVICE));
        assert!(!(GDPR <= NOT_USED_FOR_TRAINING));
        assert!(!(GDPR >= NOT_USED_FOR_TRAINING));
    }

    #[test]
    fn permission_insert() {
        let mut permissions = vec![PUBLIC];
        Permission::insert(&mut permissions, NOT_USED_FOR_TRAINING);
        assert_eq!(permissions, vec![NOT_USED_FOR_TRAINING]);

        Permission::insert(&mut permissions, ON_DEVICE);
        assert_eq!(permissions, vec![ON_DEVICE]);

        Permission::insert(&mut permissions, PUBLIC);
        assert_eq!(permissions, vec![ON_DEVICE]);

        Permission::insert(&mut permissions, GDPR);
        assert_eq!(permissions, vec![ON_DEVICE, GDPR]);

        let mut permissions = vec![];
        Permission::insert(&mut permissions, GDPR);
        assert_eq!(permissions, vec![GDPR]);

        Permission::insert(&mut permissions, PUBLIC);
        assert_eq!(permissions, vec![GDPR]);

        Permission::insert(&mut permissions, NOT_USED_FOR_TRAINING);
        assert_eq!(permissions, vec![GDPR, NOT_USED_FOR_TRAINING]);

        Permission::insert(&mut permissions, ON_DEVICE);
        assert_eq!(permissions, vec![GDPR, ON_DEVICE]);
    }
}
