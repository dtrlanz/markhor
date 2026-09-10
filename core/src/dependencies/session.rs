use thiserror::Error;

use crate::{dependencies::{Provide, api_key::ApiKey, dependency_slot::DependencySlot}, extension::{Extension, InitExtensionError}, library::{Document, Scope, Workspace}, permissions::{self, Authorized, Permission, Restricted}};

use std::{mem, sync::Arc};

pub struct Session {
    workspace: Option<Workspace>,
    documents: Vec<Document>,
    api_keys: Vec<DependencySlot<ApiKey>>,
    extensions: Vec<DependencySlot<Arc<dyn Extension>>>,
    permissions_required: Vec<Permission>,
    provides_api_keys: bool,
    provides_library_access: bool,
}

impl Session {
    pub fn new() -> Self {
        Self {
            workspace: None,
            documents: vec![],
            api_keys: vec![],
            provides_api_keys: false,
            provides_library_access: true,
            extensions: vec![], 
            permissions_required: vec![] 
        }
    }

    pub(crate) fn add_api_key(&mut self, api_key: ApiKey, permissions: Vec<Permission>) {
        self.api_keys.push(
            DependencySlot::new(api_key, permissions)
        );
    }

    // Crate-public because it should be called with accurate permissions
    // This method is more of a temporary solution anyway
    pub(crate) async fn add_extension<E: Extension + 'static>(
        &mut self,
        mut extension: E,
        permissions: Vec<Permission>,
    ) -> Result<(), InitExtensionError> {
        extension.initialize().await?;
        for p in &permissions {
            // TODO: avoid cloning when permission is already present (`ToOwned` etc.)
            Permission::insert(&mut self.permissions_required, p.clone());
        }
        self.extensions.push(DependencySlot::new(Arc::new(extension), permissions));
        Ok(())
    }

    pub async fn resolve_extension<E: Extension + Provide + 'static>(&mut self) -> Result<(), InitExtensionError> {
        let default_permissions = permissions::all_permissions();
        let (extension, permissions) = self.track_permissions(
            default_permissions, 
            async |session| {
                let e = E::first(session)?;
                Ok::<_, InitExtensionError>(e)
            }
        ).await;
        self.add_extension(extension?, permissions).await
    }

    /// Executes a function while tracking its dependencies and the permissions entailed by those
    /// dependencies.
    /// Returns the result of the function and a list of permissions to be granted based on the 
    /// dependencies used by the function.
    /// 
    /// This is used when resolving the dependencies of an extension, so that the extension can be
    /// added to the session with the correct permissions. When an extension uses dependencies,
    /// the permissions granted to the extension are limited by those granted to the dependencies.
    /// For example, if an extension uses an API key that is only allowed to be used for 
    /// non-training purposes, then the extension itself will also be limited to non-training 
    /// purposes.
    /// 
    /// The resulting permissions are the *intersection* of the permissions granted to each 
    /// dependency and of a default set of permissions.
    /// 
    /// While executing the function, the [`Session`] disallows access to [`Restricted`] assets 
    /// (e.g., documents). The reason is that it is not yet known what permissions the extension 
    /// will have, so it should not be allowed to access any assets that require permissions.
    // 
    // *Note*: This may become trickier to implement as the variety and complexity of assets 
    // increases. Extensions used as dependencies may themselves be restricted; tools offered by 
    // extensions may offer access to documents; etc. A possible solution might be to limit 
    // functionality during dependency resolution (e.g., no tool calls, enforced by `Comp`).
    async fn track_permissions<T, F: AsyncFnOnce(&Session) -> T>(&mut self, default: Vec<Permission>, f: F) -> (T, Vec<Permission>) {
        // Reset dependency tracking for API keys and extensions
        for slot in &mut self.api_keys {
            slot.reset_tracking();
        }
        for slot in &mut self.extensions {
            slot.reset_tracking();
        }
        // Provide API keys during dependency resolution since extensions may depend on them
        self.provides_api_keys = true;

        // Disallow library access during dependency resolution
        self.provides_library_access = false;

        
        for slot in &mut self.api_keys {
            slot.reset_tracking();
        }
        for slot in &mut self.extensions {
            slot.reset_tracking();
        }

        // Execute the function with the temporary session
        let result = f(&self).await;

        // Find intersection of permissions, starting with the default set of permissions
        let mut permissions = default;

        // Permissions granted to API keys in use
        for slot in &mut self.api_keys {
            if slot.is_active() {
                permissions = Permission::intersection(&permissions, slot.permissions_granted());
            }
        }

        // Permissions granted to extensions in use
        for slot in &mut self.extensions {
            if slot.is_active() {
                permissions = Permission::intersection(&permissions, slot.permissions_granted());
            }
        }

        self.provides_api_keys = false;
        self.provides_library_access = true;

        (result, permissions)
    }

    pub(crate) fn api_keys(&self) -> &[DependencySlot<ApiKey>] {
        if self.provides_api_keys {
            &self.api_keys
        } else {
            &[]
        }
    }

    pub(crate) fn extensions(&self) -> &[DependencySlot<Arc<dyn Extension>>] {
        &self.extensions
    }

    #[cfg(test)]
    pub(crate) fn provide_api_keys(&mut self) {
        self.provides_api_keys = true;
    }
}

impl Restricted for Session {
    fn permissions_required(&self) -> &[Permission] {
        &self.permissions_required
    }
}

impl From<Workspace> for Session {
    fn from(workspace: Workspace) -> Self {
        Self {
            workspace: Some(workspace),
            documents: vec![],
            api_keys: vec![],
            extensions: vec![],
            permissions_required: vec![],
            provides_api_keys: false,
            provides_library_access: true,
        }
    }
}

#[derive(Debug, Error)]
pub enum RestrictAccessError {
    #[error("API keys lack required permissions: {}", .0.join(", "))]
    ApiKeyNotAuthorized(Vec<String>),

    #[error("Extensions lack required permissions: {}", .0.join(", "))]
    ExtensionNotAuthorized(Vec<String>),
}


#[cfg(test)]
mod tests {
    use super::*;
    use crate::chunking::{Chunker, test_chunker::FixedSizeChunkerExtension};
    use crate::dependencies::{ResolveDependencyError};
    use crate::extension::Comp;
    use crate::permissions::{GDPR, NOT_USED_FOR_TRAINING, ON_DEVICE, PUBLIC};
    use std::sync::{Barrier, OnceLock};

    struct Actor(Vec<Permission>);

    impl Authorized for Actor {
        fn permissions_granted(&self) -> &[Permission] {
            &self.0
        }
    }

    #[tokio::test]
    async fn track_permissions_for_extensions() {
        let ext0 = FixedSizeChunkerExtension::new(10);
        let ext1 = FixedSizeChunkerExtension::new(20);
        let ext2 = FixedSizeChunkerExtension::new(30);
        let ext3 = FixedSizeChunkerExtension::new(40);

        let mut session = Session::new();
        assert!(Actor(vec![]).may_access(&session));

        session.add_extension(ext0, vec![PUBLIC]).await.unwrap();
        assert!(!Actor(vec![]).may_access(&session));
        assert!(Actor(vec![PUBLIC]).may_access(&session));

        session.add_extension(ext1, vec![NOT_USED_FOR_TRAINING]).await.unwrap();
        assert!(!Actor(vec![]).may_access(&session));
        assert!(!Actor(vec![PUBLIC]).may_access(&session));
        assert!(Actor(vec![NOT_USED_FOR_TRAINING]).may_access(&session));

        session.add_extension(ext2, vec![GDPR]).await.unwrap();
        assert!(!Actor(vec![]).may_access(&session));
        assert!(!Actor(vec![NOT_USED_FOR_TRAINING]).may_access(&session));
        assert!(!Actor(vec![GDPR]).may_access(&session));
        assert!(Actor(vec![NOT_USED_FOR_TRAINING, GDPR]).may_access(&session));
        assert_eq!(session.extensions.len(), 3);

        session.add_extension(ext3, vec![ON_DEVICE]).await.unwrap();
        assert!(!Actor(vec![NOT_USED_FOR_TRAINING, GDPR]).may_access(&session));
        assert_eq!(session.extensions.len(), 4);

        let default = permissions::all_permissions();

        let (_, perm) = session.track_permissions(default.clone(), async |sess| {
            // Use ext0
            Comp::<dyn Chunker>::first(sess).unwrap()
        }).await;
        assert_eq!(perm, vec![PUBLIC]);

        let (_, perm) = session.track_permissions(default.clone(), async |sess| {
            // Use all four extensions
            Vec::<Comp<dyn Chunker>>::first(sess).unwrap()
        }).await;
        assert_eq!(perm, vec![PUBLIC]);

        let (_, perm) = session.track_permissions(default.clone(), async |sess| {
            // Use ext0 and ext1
            let mut chunkers = Vec::<Comp<dyn Chunker>>::first(sess).unwrap();
            chunkers.truncate(2);
            chunkers
        }).await;
        assert_eq!(perm, vec![PUBLIC]);

        let (_, perm) = session.track_permissions(default.clone(), async |sess| {
            // Use ext1 and ext2
            let mut chunkers = Vec::<Comp<dyn Chunker>>::first(sess).unwrap();
            chunkers.remove(0);
            chunkers.pop();
            chunkers
        }).await;
        assert_eq!(perm, vec![PUBLIC]);

        let (_, perm) = session.track_permissions(default.clone(), async |sess| {
            // Use ext1 and ext3
            let mut chunkers = Vec::<Comp<dyn Chunker>>::first(sess).unwrap();
            chunkers.remove(2);
            chunkers.remove(0);
            chunkers
        }).await;
        assert_eq!(perm, vec![NOT_USED_FOR_TRAINING]);

        let (_, perm) = session.track_permissions(default.clone(), async |sess| {
            // Use ext3
            let mut chunkers = Vec::<Comp<dyn Chunker>>::first(sess).unwrap();
            let c = chunkers.pop().unwrap();
            c
        }).await;
        assert_eq!(perm, vec![ON_DEVICE]);
    }

    #[tokio::test]
    async fn track_permissions_for_api_keys() {
        let mut session = Session::new();
        session.add_api_key(
            ApiKey::new(
                "1234567890abcdef".to_string(),
                "TestProvider".to_string(),
                "TestProject".to_string(),
            ),
            vec![NOT_USED_FOR_TRAINING],
        );
        session.add_api_key(
            ApiKey::new(
                "abcdef1234567890".to_string(),
                "AnotherProvider".to_string(),
                "AnotherProject".to_string(),
            ),
            vec![GDPR],
        );

        let default = permissions::all_permissions();

        session.provide_api_keys();
        let (_, perm) = session.track_permissions(default.clone(), async |sess| {
            // Use no API keys
            let _api_keys = Vec::<ApiKey>::first(sess).unwrap();
        }).await;
        assert_eq!(perm, default);

        session.provide_api_keys();
        let (_, perm) = session.track_permissions(default.clone(), async |sess| {
            // Use only the first API key
            let api_keys = Vec::<ApiKey>::first(sess).unwrap();
            api_keys[0].key();
        }).await;
        assert_eq!(perm, vec![NOT_USED_FOR_TRAINING]);

        session.provide_api_keys();
        let (_, perm) = session.track_permissions(default.clone(), async |sess| {
            // Use only the second API key
            let api_keys = Vec::<ApiKey>::first(sess).unwrap();
            api_keys[1].key();
        }).await;
        assert_eq!(perm, vec![GDPR]);

        session.provide_api_keys();
        let (_, perm) = session.track_permissions(default.clone(), async |sess| {
            // Use both API keys
            let api_keys = Vec::<ApiKey>::first(sess).unwrap();
            api_keys[0].key();
            api_keys[1].key();
        }).await;
        assert_eq!(perm, vec![PUBLIC]);
    }

    #[tokio::test]
    async fn resolve_extension_depending_on_extensions() {
        #[derive(Provide)]
        #[provide(crate = "crate")]
        struct TestExtension0 {
            // uses single chunker
            _chunker: Comp<dyn Chunker>,
        }

        impl Extension for TestExtension0 {
            fn uri(&self) -> &str           { "markhor://test-extension-0" }
            fn name(&self) -> &str          { "test-extension-0" }
            fn description(&self) ->  &str  { "Test extension 0" }
        }

        #[derive(Provide)]
        #[provide(crate = "crate")]
        struct TestExtension1 {
            // uses all chunkers
            _chunkers: Vec<Comp<dyn Chunker>>,
        }

        impl Extension for TestExtension1 {
            fn uri(&self) -> &str           { "markhor://test-extension-1" }
            fn name(&self) -> &str          { "test-extension-1" }
            fn description(&self) ->  &str  { "Test extension 1" }
        }

        // Session setup
        let ext0 = FixedSizeChunkerExtension::new(10);
        let ext1 = FixedSizeChunkerExtension::new(20);
        let mut session = Session::new();
        session.add_extension(ext0, vec![NOT_USED_FOR_TRAINING]).await.unwrap();
        session.add_extension(ext1, vec![PUBLIC]).await.unwrap();
        assert_eq!(session.extensions.len(), 2);

        // Resolve TestExtension0, which should be granted permission NOT_USED_FOR_TRAINING
        session.resolve_extension::<TestExtension0>().await.unwrap();
        assert_eq!(session.extensions.len(), 3);
        assert_eq!(session.extensions[2].item.name(), "test-extension-0");
        assert_eq!(session.extensions[2].permissions_granted(), vec![NOT_USED_FOR_TRAINING]);

        // Resolve TestExtension1, which should be granted only permission PUBLIC
        session.resolve_extension::<TestExtension1>().await.unwrap();
        assert_eq!(session.extensions.len(), 4);
        assert_eq!(session.extensions[3].item.name(), "test-extension-1");
        assert_eq!(session.extensions[3].permissions_granted(), vec![PUBLIC]);
    }

    #[tokio::test]
    async fn resolve_extension_depending_on_api_keys() {
        #[derive(Provide)]
        #[provide(crate = "crate")]
        struct TestExtension0 {
            // uses single API key
            _api_key: ApiKey,
        }

        impl Extension for TestExtension0 {
            fn uri(&self) -> &str           { "markhor://test-extension-0" }
            fn name(&self) -> &str          { "test-extension-0" }
            fn description(&self) ->  &str  { "Test extension 0" }
        }

        #[derive(Provide)]
        #[provide(crate = "crate")]
        struct TestExtension1 {
            // uses all API keys
            _api_keys: Vec<ApiKey>,
        }

        impl Extension for TestExtension1 {
            fn uri(&self) -> &str           { "markhor://test-extension-1" }
            fn name(&self) -> &str          { "test-extension-1" }
            fn description(&self) ->  &str  { "Test extension 1" }
        }

        struct TestExtension2 {
            // uses single API key, but as a string!
            _api_key: String,
        }

        impl Extension for TestExtension2 {
            fn uri(&self) -> &str           { "markhor://test-extension-2" }
            fn name(&self) -> &str          { "test-extension-2" }
            fn description(&self) ->  &str  { "Test extension 2" }
        }

        impl Provide for TestExtension2 {
            type Item = TestExtension2;

            fn iter(session: &Session) -> Result<impl Iterator<Item = Self>, ResolveDependencyError> {
                // Get API key during resolution, convert to string, then drop `ApiKey`
                let api_key = ApiKey::first(session)?;
                Ok(std::iter::once(TestExtension2 {
                    _api_key: api_key.key().to_string(),
                }))
            }

            fn iter_from_items<I>(items: I) -> Result<impl Iterator<Item = Self>, ResolveDependencyError>
            where
                I: Iterator<Item = Self::Item>
            {
                Ok(items)
            }
        }

        #[derive(Provide)]
        #[provide(crate = "crate")]
        struct TestExtension3 {
            // uses second API key only
            #[provide(filter = |k| k.provider_is("AnotherProvider"))]
            _api_key: ApiKey,
        }

        impl Extension for TestExtension3 {
            fn uri(&self) -> &str           { "markhor://test-extension-3" }
            fn name(&self) -> &str          { "test-extension-3" }
            fn description(&self) ->  &str  { "Test extension 3" }
        }

        let mut session = Session::new();
        session.add_api_key(
            ApiKey::new (
                "1234567890abcdef".to_string(),
                "TestProvider".to_string(),
                "TestProject".to_string(),
            ),
            vec![GDPR],
        );
        session.add_api_key(
            ApiKey::new (
                "abcdef1234567890".to_string(),
                "AnotherProvider".to_string(),
                "AnotherProject".to_string(),
            ),
            vec![NOT_USED_FOR_TRAINING],
        );
        session.add_api_key(
            ApiKey::new (
                "0987654321fedcba".to_string(),
                "YetAnotherProvider".to_string(),
                "YetAnotherProject".to_string(),
            ),
            vec![PUBLIC],
        );

        fn count_active_api_keys(session: &Session) -> usize {
            session.api_keys.iter().filter(|k| k.is_active()).count()
        }
        assert_eq!(count_active_api_keys(&session), 0);
        
        // Resolve TestExtension0, which should be granted only permission GDPR
        session.resolve_extension::<TestExtension0>().await.unwrap();
        assert_eq!(session.extensions.len(), 1);
        assert_eq!(session.extensions[0].item.name(), "test-extension-0");
        assert_eq!(session.extensions[0].permissions_granted(), vec![GDPR]);
        // Only 1 active API key
        assert_eq!(count_active_api_keys(&session), 1);

        // Resolve TestExtension1, which should be granted only permission PUBLIC
        session.resolve_extension::<TestExtension1>().await.unwrap();
        assert_eq!(session.extensions.len(), 2);
        assert_eq!(session.extensions[1].item.name(), "test-extension-1");
        assert_eq!(session.extensions[1].permissions_granted(), vec![PUBLIC]);
        // All 3 API keys are now active
        assert_eq!(count_active_api_keys(&session), 3);

        // Resolve TestExtension2, which should be granted permission GDPR
        // even though the extension doesn't store the key
        session.resolve_extension::<TestExtension2>().await.unwrap();
        assert_eq!(session.extensions.len(), 3);
        assert_eq!(session.extensions[2].item.name(), "test-extension-2");
        assert_eq!(session.extensions[2].permissions_granted(), vec![GDPR]);

        // Resolve TestExtension3, which should be granted only permission NOT_USED_FOR_TRAINING
        // as per API key "AnotherProvider"
        session.resolve_extension::<TestExtension3>().await.unwrap();
        assert_eq!(session.extensions.len(), 4);
        assert_eq!(session.extensions[3].item.name(), "test-extension-3");
        assert_eq!(session.extensions[3].permissions_granted(), vec![NOT_USED_FOR_TRAINING]);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn resolve_extension_with_concurrent_drop() {
        static BARRIER: OnceLock<Barrier> = OnceLock::new();
        BARRIER.get_or_init(|| Barrier::new(2));

        struct TestExtension0 {
            // uses first chunker only
            _chunker: Comp<dyn Chunker>,
        }

        impl Provide for TestExtension0 {
            type Item = TestExtension0;

            fn iter(session: &Session) -> Result<impl Iterator<Item = Self>, ResolveDependencyError> {
                let mut chunker = Some(Comp::<dyn Chunker>::first(session)?);
                Ok(std::iter::from_fn(move || {
                    chunker.take().map(|c| {
                        BARRIER.get().unwrap().wait();
                        // Some potentially problematic activity occurs here on another thread
                        BARRIER.get().unwrap().wait();
                        TestExtension0 {
                            _chunker: c,
                        }
                    })
                }))
            }

            fn iter_from_items<I>(items: I) -> Result<impl Iterator<Item = Self>, ResolveDependencyError>
            where
                I: Iterator<Item = Self::Item>
            {
                Ok(items)
            }
        }

        impl Extension for TestExtension0 {
            fn uri(&self) -> &str           { "markhor://test-extension-0" }
            fn name(&self) -> &str          { "test-extension-0" }
            fn description(&self) ->  &str  { "Test extension 0" }
        }

        // Session setup
        let ext0 = FixedSizeChunkerExtension::new(10);
        let mut session = Session::new();
        session.add_extension(ext0, vec![NOT_USED_FOR_TRAINING]).await.unwrap();
        assert_eq!(session.extensions.len(), 1);

        // Simulate that `FixedSizeChunkerExtension` is already in use and its pointer dropped at
        // an inopportune time.
        let chunker0 = Some(Comp::<dyn Chunker>::first(&session).unwrap());
        tokio::spawn(async move {
            BARRIER.get().unwrap().wait();
            mem::drop(chunker0);
            BARRIER.get().unwrap().wait();
        });

        // Resolve TestExtension0, which should be granted only permission NOT_USED_FOR_TRAINING
        // However, the existing `Comp` referencing the same `FixedSizeChunkerExtension` will be
        // dropped *while* `TestExtension0` is resolved.
        session.resolve_extension::<TestExtension0>().await.unwrap();

        // Nevertheless, if permissions are tracked correctly, `TestExtension0` should end up
        // with the same requirements as `FixedSizeChunkerExtension`.
        assert_eq!(session.extensions.len(), 2);
        assert_eq!(session.extensions[1].item.name(), "test-extension-0");
        assert_eq!(session.extensions[0].permissions_granted(), vec![NOT_USED_FOR_TRAINING]);
        assert_eq!(session.extensions[1].permissions_granted(), vec![NOT_USED_FOR_TRAINING]);
    }
}