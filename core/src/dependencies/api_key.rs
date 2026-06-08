use std::{fmt::Debug, sync::{Arc, atomic::{AtomicBool, Ordering}}};

use crate::{dependencies::{Provide, ResolveDependencyError}, permissions::{Authorized, Permission}};


#[derive(Clone)]
pub struct ApiKey {
    inner: Arc<ApiKeyInner>,
}

struct ApiKeyInner {
    key: String,
    provider: String,
    project: String,
    permissions: Vec<Permission>,
    active: AtomicBool,
}

impl ApiKey {
    pub fn new(key: String, provider: String, project: String, permissions: Vec<Permission>) -> Self {
        Self {
            inner: Arc::new(ApiKeyInner {
                key,
                provider,
                project,
                permissions,
                active: AtomicBool::new(false),
            }),
        }
    }

    pub(crate) fn to_inactive(&self) -> Self {
        Self {
            inner: Arc::new(ApiKeyInner {
                key: self.inner.key.clone(),
                provider: self.inner.provider.clone(),
                project: self.inner.project.clone(),
                permissions: self.inner.permissions.clone(),
                active: AtomicBool::new(false),
            }),
        }
    }

    pub fn key(&self) -> &str {
        self.inner.active.store(true, Ordering::Release);
        &self.inner.key
    }

    /// Checks if the API key is active.
    /// 
    /// A key being active means that even if this clone of the key was dropped immediately, the 
    /// key itself might continue being used (because other clones exist, or because it has been
    /// read and might have been converted to a string). A key being inactive means that if this 
    /// clone of the key is not read until it is dropped, we can assume that the key will not be 
    /// used.
    /// 
    /// Obviously, this only pertains to the keys associated with a single session (which should 
    /// all be clones of each other). Whether the same key is used in other sessions is 
    /// irrelevant, as a far as managing session permissions is concerned.
    pub(crate) fn is_active(&self) -> bool {
        Arc::strong_count(&self.inner) > 1
        || self.inner.active.load(Ordering::Acquire)
    }

    pub fn provider(&self) -> &str {
        &self.inner.provider
    }

    /// Checks if the API key is associated with the given provider.
    /// 
    /// Unlike `==`, the comparison is case-insensitive.
    pub fn provider_is(&self, provider: &str) -> bool {
        self.inner.provider.eq_ignore_ascii_case(provider)
    }
    
    pub fn project(&self) -> &str {
        &self.inner.project
    }
}

impl Authorized for ApiKey {
    fn permissions_granted(&self) -> &[Permission] {
        &self.inner.permissions
    }
}

impl Debug for ApiKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ApiKey")
            .field("key", &format!("...{}", &self.inner.key[self.inner.key.len().saturating_sub(4)..]))
            .field("provider", &self.inner.provider)
            .field("project", &self.inner.project)
            .field("permissions", &self.inner.permissions)
            .field("active", &self.inner.active.load(Ordering::Acquire))
            .finish()
    }
}

impl Provide for ApiKey {
    type Item = Self;

    fn iter(session: &super::Session) -> Result<impl Iterator<Item = Self>, ResolveDependencyError> {
        Ok(session.active_api_keys.iter().cloned())
    }

    fn iter_from_items<I>(items: I) -> Result<impl Iterator<Item = Self>, ResolveDependencyError>
    where
        I: Iterator<Item = Self::Item>
    {
        Ok(items)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dependencies::Session;

    #[test]
    fn new_api_key() {
        let api_key = ApiKey::new(
            "1234567890abcdef".to_string(),
            "TestProvider".to_string(),
            "TestProject".to_string(),
            vec![],
        );

        // Should start out inactive
        assert!(!api_key.is_active());
        assert_eq!(api_key.key(), "1234567890abcdef");
        // Active after read
        assert!(api_key.is_active());
        assert_eq!(api_key.provider(), "TestProvider");
        assert_eq!(api_key.project(), "TestProject");
        // Clone is also active
        let api_key_clone = api_key.clone();
        assert!(api_key_clone.is_active());
    }

    #[test]
    fn provider_is() {
        let api_key = ApiKey::new(
            "1234567890abcdef".to_string(),
            "TestProvider".to_string(),
            "TestProject".to_string(),
            vec![],
        );

        assert!(api_key.provider_is("TestProvider"));
        assert!(api_key.provider_is("testprovider"));
        assert!(!api_key.provider_is("OtherProvider"));
    }

    #[test]
    fn debug_format() {
        let api_key = ApiKey::new(
            "1234567890abcdef".to_string(),
            "TestProvider".to_string(),
            "TestProject".to_string(),
            vec![],
        );

        let debug_string = format!("{:?}", api_key);
        // Must contain only the last 4 chars of the key
        assert!(debug_string.contains("cdef"));
        assert!(!debug_string.contains("bcdef"));
        assert!(debug_string.contains("TestProvider"));
        assert!(debug_string.contains("TestProject"));
    }

    #[test]
    fn provide() {
        let api_key1 = ApiKey::new(
            "1234567890abcdef".to_string(),
            "TestProvider".to_string(),
            "TestProject".to_string(),
            vec![],
        );
        let api_key2 = ApiKey::new(
            "abcdef1234567890".to_string(),
            "AnotherProvider".to_string(),
            "AnotherProject".to_string(),
            vec![],
        );

        let mut session = Session::new();
        session.active_api_keys.push(api_key1);
        session.active_api_keys.push(api_key2);

        let provided_keys: Vec<ApiKey> = ApiKey::iter(&session).unwrap().collect();
        assert_eq!(provided_keys.len(), 2);
        assert!(provided_keys.iter().any(|k| k.key() == "1234567890abcdef"));
        assert!(provided_keys.iter().any(|k| k.key() == "abcdef1234567890"));
    }
}

