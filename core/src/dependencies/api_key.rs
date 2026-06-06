use std::{fmt::Debug, sync::{Arc, atomic::{AtomicBool, Ordering}}};

use serde::{Deserialize, Serialize};

use crate::permissions::{Authorized, Permission};


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

    pub fn key(&self) -> &str {
        self.inner.active.store(true, Ordering::Release);
        &self.inner.key
    }

    pub(crate) fn is_active(&self) -> bool {
        self.inner.active.load(Ordering::Acquire)
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


#[cfg(test)]
mod tests {
    use super::*;

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
}

