use std::{fmt::Debug, sync::{Arc, atomic::{AtomicBool, Ordering}}};

use crate::{dependencies::{Provide, ResolveDependencyError, TrackingGuard}, permissions::{Authorized, Permission}};


#[derive(Clone)]
pub struct ApiKey {
    key: String,
    provider: String,
    project: String,
    tracking_guard: TrackingGuard,
}

impl ApiKey {
    pub fn new(key: String, provider: String, project: String) -> Self {
        Self {
            key,
            provider,
            project,
            tracking_guard: Default::default(),
        }
    }

    pub(crate) fn tracked(&self, tracking_guard: TrackingGuard) -> Self {
        Self {
            key: self.key.clone(),
            provider: self.provider.clone(),
            project: self.project.clone(),
            tracking_guard,
        }
    }

    pub fn key(&self) -> &str {
        self.tracking_guard.mark_active();
        &self.key
    }

    pub fn provider(&self) -> &str {
        &self.provider
    }

    /// Checks if the API key is associated with the given provider.
    /// 
    /// Unlike `==`, the comparison is case-insensitive.
    pub fn provider_is(&self, provider: &str) -> bool {
        self.provider.eq_ignore_ascii_case(provider)
    }
    
    pub fn project(&self) -> &str {
        &self.project
    }
}

impl Debug for ApiKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ApiKey")
            .field("key", &format!("...{}", &self.key[self.key.len().saturating_sub(4)..]))
            .field("provider", &self.provider)
            .field("project", &self.project)
            .finish()
    }
}

impl Provide for ApiKey {
    type Item = Self;

    fn iter(session: &super::Session) -> Result<impl Iterator<Item = Self>, ResolveDependencyError> {
        Ok(session.api_keys().into_iter().map(|slot| slot.item.tracked(slot.tracking_guard())))
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
        );

        assert_eq!(api_key.key(), "1234567890abcdef");
        assert_eq!(api_key.provider(), "TestProvider");
        assert_eq!(api_key.project(), "TestProject");
    }

    #[test]
    fn provider_is() {
        let api_key = ApiKey::new(
            "1234567890abcdef".to_string(),
            "TestProvider".to_string(),
            "TestProject".to_string(),
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
        let mut session = Session::new();
        session.add_api_key(
            ApiKey::new(
                "1234567890abcdef".to_string(),
                "TestProvider".to_string(),
                "TestProject".to_string(),
            ),
            vec![],
        );
        session.add_api_key(
            ApiKey::new(
                "abcdef1234567890".to_string(),
                "AnotherProvider".to_string(),
                "AnotherProject".to_string(),
            ),
            vec![],
        );

        session.provide_api_keys();
        let provided_keys: Vec<ApiKey> = ApiKey::iter(&session).unwrap().collect();
        assert_eq!(provided_keys.len(), 2);
        assert!(provided_keys.iter().any(|k| k.key() == "1234567890abcdef"));
        assert!(provided_keys.iter().any(|k| k.key() == "abcdef1234567890"));
    }
}

