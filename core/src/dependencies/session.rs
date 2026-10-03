use thiserror::Error;

use crate::{dependencies::{Provide, ResolveDependencyError, api_key::ApiKey, dependency_slot::DependencySlot}, extension::{Extension, ExtensionConfig, InitExtensionError}, library::{Document, Scope, Workspace}, permissions::{self, Authorized, Permission, Restricted}};

use std::sync::Arc;

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

    // Currently, this method is just a wrapper for `Provide::first`, similar to the
    // relationship between `String::parse` and `FromStr::from_str`. 
    // I suspect that additional functionality may be added in the future. It might turn
    // out to be desirable to track asset usage or permissions here, or we might want to
    // load/initialize assets lazily when they are first resolved (though that would require
    // async).
    // In any case, this method is the recommended way of resolving dependencies, rather than 
    // calling `Provide::first` directly, if only for the sake of readability (as readers
    // should not have to understand the `Provide` trait to make sense of code using 
    // `Session`).
    pub fn resolve<T: Provide>(&self) -> Result<T, ResolveDependencyError> {
        T::first(self)
    }

    pub(crate) fn add_api_key(&mut self, api_key: ApiKey, permissions: Vec<Permission>) {
        self.api_keys.push(
            DependencySlot::new(api_key, permissions)
        );
    }

    pub async fn initialize_extension<F: FnOnce(&Session) -> Result<E, InitExtensionError>, E: Extension + 'static>(&mut self, config: ExtensionConfig, f: F) -> Result<(), InitExtensionError> {
        let default_permissions = config.permissions;
        let (extension, permissions) = self.track_permissions(
            default_permissions, 
            async |s: &Session| f(s)
        ).await;
        let mut extension = extension?;
        extension.initialize().await?;
        for p in &permissions {
            // TODO: avoid cloning when permission is already present (`ToOwned` etc.)
            Permission::insert(&mut self.permissions_required, p.clone());
        }
        self.extensions.push(DependencySlot::new(Arc::new(extension), permissions));
        Ok(())
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
        // Provide API keys during dependency resolution since extensions may depend on them
        self.provides_api_keys = true;

        // Disallow library access during dependency resolution
        self.provides_library_access = false;

        // Begin dependency tracking for API keys and extensions
        for slot in &mut self.api_keys {
            slot.replace_tracker();
        }
        for slot in &mut self.extensions {
            slot.replace_tracker();
        }
        let tracker = PermissionTracker { session: &self };

        // Execute the function
        let result = f(&self).await;

        // Get permissions granted
        let permissions = tracker.permissions_granted(default);

        // Stop dependency tracking
        for slot in &mut self.api_keys {
            slot.replace_tracker();
        }
        for slot in &mut self.extensions {
            slot.replace_tracker();
        }

        // Reset session state
        self.provides_api_keys = false;
        self.provides_library_access = true;

        (result, permissions)
    }

    // Will eventually replace `initialize_extension`
    //
    // Currently limited to extensions that `impl Provide`. That may change.
    pub async fn initialize_extension_with_multiple_instances<E: Extension + Provide + 'static>(&mut self, config: ExtensionConfig) -> Result<(), InitExtensionError> {
        // Provide API keys during dependency resolution since extensions may depend on them
        self.provides_api_keys = true;

        // Disallow library access during dependency resolution
        self.provides_library_access = false;

        // Begin dependency tracking for API keys and extensions
        for slot in &mut self.api_keys {
            slot.replace_tracker();
        }
        for slot in &mut self.extensions {
            slot.replace_tracker();
        }
        let tracker = PermissionTracker { session: &self };

        let mut instances = vec![];
        let r = match E::iter(&self) {
            Ok(iter) => {
                for item in iter {
                    let permissions = tracker.permissions_granted(config.permissions.clone());
                    tracker.reset_tracking_count();
                    instances.push((item, permissions));
                }
                Ok(())
            }
            Err(e) => Err(e.into()),
        };

        // Stop dependency tracking
        for slot in &mut self.api_keys {
            slot.replace_tracker();
        }
        for slot in &mut self.extensions {
            slot.replace_tracker();
        }

        // Reset session state
        self.provides_api_keys = false;
        self.provides_library_access = true;

        for (mut extension, permissions) in instances {
            extension.initialize().await?;
            self.extensions.push(DependencySlot::new(Arc::new(extension), permissions));
        }

        r
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

struct PermissionTracker<'a> {
    session: &'a Session,
}

impl<'a> PermissionTracker<'a> {
    fn permissions_granted(&self, default: Vec<Permission>) -> Vec<Permission> {
        // Find intersection of permissions, starting with the default set of permissions
        let mut permissions = default;

        // Permissions granted to API keys in use
        for slot in &self.session.api_keys {
            if slot.is_active() {
                permissions = Permission::intersection(&permissions, slot.permissions_granted());
            }
        }

        // Permissions granted to extensions in use
        for slot in &self.session.extensions {
            if slot.is_active() {
                permissions = Permission::intersection(&permissions, slot.permissions_granted());
            }
        }
        permissions
    }

    fn reset_tracking_count(&self) {
        for slot in &self.session.api_keys {
            slot.reset_tracking_count();
        }
        for slot in &self.session.extensions {
            slot.reset_tracking_count();
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
    use tokio::task;

    use super::*;
    use crate::chunking::{Chunker, test_chunker::FixedSizeChunkerExtension};
    use crate::dependencies::{ResolveDependencyError};
    use crate::embedding::EmbeddingModel;
    use crate::embedding::test_utils::MockEmbedderExtension;
    use crate::extension::Comp;
    use crate::permissions::{GDPR, NOT_USED_FOR_TRAINING, ON_DEVICE, PUBLIC};
    use std::mem;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Barrier, Mutex, OnceLock};

    struct Actor(Vec<Permission>);

    impl Authorized for Actor {
        fn permissions_granted(&self) -> &[Permission] {
            &self.0
        }
    }

    /// Utility function for session setup. Adds an extension to the session with the given permissions.
    async fn add_extension<E: Extension + 'static>(
        session: &mut Session,
        extension: E,
        permissions: Vec<Permission>,
    ) -> Result<(), InitExtensionError> {
        session.initialize_extension(
            ExtensionConfig { permissions }, 
            |_| Ok(extension)
        ).await
    }


    #[tokio::test]
    async fn track_permissions_for_extensions() {
        let ext0 = FixedSizeChunkerExtension::new(10);
        let ext1 = FixedSizeChunkerExtension::new(20);
        let ext2 = FixedSizeChunkerExtension::new(30);
        let ext3 = FixedSizeChunkerExtension::new(40);

        let mut session = Session::new();
        assert!(Actor(vec![]).may_access(&session));

        add_extension(&mut session, ext0, vec![PUBLIC]).await.unwrap();
        assert!(!Actor(vec![]).may_access(&session));
        assert!(Actor(vec![PUBLIC]).may_access(&session));

        add_extension(&mut session, ext1, vec![NOT_USED_FOR_TRAINING]).await.unwrap();
        assert!(!Actor(vec![]).may_access(&session));
        assert!(!Actor(vec![PUBLIC]).may_access(&session));
        assert!(Actor(vec![NOT_USED_FOR_TRAINING]).may_access(&session));

        add_extension(&mut session, ext2, vec![GDPR]).await.unwrap();
        assert!(!Actor(vec![]).may_access(&session));
        assert!(!Actor(vec![NOT_USED_FOR_TRAINING]).may_access(&session));
        assert!(!Actor(vec![GDPR]).may_access(&session));
        assert!(Actor(vec![NOT_USED_FOR_TRAINING, GDPR]).may_access(&session));
        assert_eq!(session.extensions.len(), 3);

        add_extension(&mut session, ext3, vec![ON_DEVICE]).await.unwrap();
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
    async fn initialize_extension_depending_on_extensions() {
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
        add_extension(&mut session, ext0, vec![NOT_USED_FOR_TRAINING]).await.unwrap();
        add_extension(&mut session, ext1, vec![PUBLIC]).await.unwrap();
        assert_eq!(session.extensions.len(), 2);

        // Initialize TestExtension0, which should be granted permission NOT_USED_FOR_TRAINING
        session.initialize_extension(Default::default(), |s| {
            Ok(TestExtension0::first(s)?)
        }).await.unwrap();
        assert_eq!(session.extensions.len(), 3);
        assert_eq!(session.extensions[2].item.name(), "test-extension-0");
        assert_eq!(session.extensions[2].permissions_granted(), vec![NOT_USED_FOR_TRAINING]);

        // Initialize TestExtension1, which should be granted only permission PUBLIC
        session.initialize_extension(Default::default(), |s| {
            Ok(TestExtension1::first(s)?)
        }).await.unwrap();
        assert_eq!(session.extensions.len(), 4);
        assert_eq!(session.extensions[3].item.name(), "test-extension-1");
        assert_eq!(session.extensions[3].permissions_granted(), vec![PUBLIC]);
    }

    #[tokio::test]
    async fn initialize_extension_depending_on_api_keys() {
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

        // Initialize TestExtension0, which should be granted only permission GDPR
        session.initialize_extension(Default::default(), |s| {
            Ok(TestExtension0::first(s)?)
        }).await.unwrap();
        assert_eq!(session.extensions.len(), 1);
        assert_eq!(session.extensions[0].item.name(), "test-extension-0");
        assert_eq!(session.extensions[0].permissions_granted(), vec![GDPR]);

        // Initialize TestExtension1, which should be granted only permission PUBLIC
        session.initialize_extension(Default::default(), |s| {
            Ok(TestExtension1::first(s)?)
        }).await.unwrap();
        assert_eq!(session.extensions.len(), 2);
        assert_eq!(session.extensions[1].item.name(), "test-extension-1");
        assert_eq!(session.extensions[1].permissions_granted(), vec![PUBLIC]);
        
        // Initialize TestExtension2, which should be granted permission GDPR
        // even though the extension doesn't store the key
        session.initialize_extension(Default::default(), |s| {
            Ok(TestExtension2::first(s)?)
        }).await.unwrap();
        assert_eq!(session.extensions.len(), 3);
        assert_eq!(session.extensions[2].item.name(), "test-extension-2");
        assert_eq!(session.extensions[2].permissions_granted(), vec![GDPR]);

        // Initialize TestExtension3, which should be granted only permission NOT_USED_FOR_TRAINING
        // as per API key "AnotherProvider"
        session.initialize_extension(Default::default(), |s| {
            Ok(TestExtension3::first(s)?)
        }).await.unwrap();
        assert_eq!(session.extensions.len(), 4);
        assert_eq!(session.extensions[3].item.name(), "test-extension-3");
        assert_eq!(session.extensions[3].permissions_granted(), vec![NOT_USED_FOR_TRAINING]);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn initialize_extension_with_concurrent_drop() {
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
        add_extension(&mut session, ext0, vec![NOT_USED_FOR_TRAINING]).await.unwrap();
        assert_eq!(session.extensions.len(), 1);

        // Simulate that `FixedSizeChunkerExtension` is already in use and its pointer dropped at
        // an inopportune time.
        let chunker0 = Some(Comp::<dyn Chunker>::first(&session).unwrap());
        tokio::spawn(async move {
            BARRIER.get().unwrap().wait();
            mem::drop(chunker0);
            BARRIER.get().unwrap().wait();
        });

        // Initialize TestExtension0, which should be granted only permission NOT_USED_FOR_TRAINING
        // However, the existing `Comp` referencing the same `FixedSizeChunkerExtension` will be
        // dropped *while* `TestExtension0` is initialized.
        session.initialize_extension(Default::default(), |s| {
            Ok(TestExtension0::first(s)?)
        }).await.unwrap();

        // Nevertheless, if permissions are tracked correctly, `TestExtension0` should end up
        // with the same requirements as `FixedSizeChunkerExtension`.
        assert_eq!(session.extensions.len(), 2);
        assert_eq!(session.extensions[1].item.name(), "test-extension-0");
        assert_eq!(session.extensions[0].permissions_granted(), vec![NOT_USED_FOR_TRAINING]);
        assert_eq!(session.extensions[1].permissions_granted(), vec![NOT_USED_FOR_TRAINING]);
    }

    #[tokio::test]
    async fn initialize_extension_with_multiple_instances() {
        #[derive(Provide)]
        #[provide(crate = "crate")]
        struct TestExtension {
            // Provides one instance per available chunker
            #[provide(each)]
            _chunker: Comp<dyn Chunker>,
        }

        impl Extension for TestExtension {
            fn uri(&self) -> &str           { "markhor://test-extension" }
            fn name(&self) -> &str          { "test-extension" }
            fn description(&self) ->  &str  { "Test extension" }
        }

        // Session setup
        let ext0 = FixedSizeChunkerExtension::new(10);
        let ext1 = FixedSizeChunkerExtension::new(20);
        let mut session = Session::new();
        add_extension(&mut session, ext0, vec![NOT_USED_FOR_TRAINING]).await.unwrap();
        add_extension(&mut session, ext1, vec![GDPR]).await.unwrap();
        assert_eq!(session.extensions.len(), 2);

        // Initialize TestExtension
        session.initialize_extension_with_multiple_instances::<TestExtension>(Default::default()).await.unwrap();
        assert_eq!(session.extensions.len(), 4);
        assert_eq!(session.extensions[2].item.name(), "test-extension");
        assert_eq!(session.extensions[3].item.name(), "test-extension");
        assert_eq!(session.extensions[2].permissions_granted(), vec![NOT_USED_FOR_TRAINING]);
        assert_eq!(session.extensions[3].permissions_granted(), vec![GDPR]);
    }

    #[tokio::test]
    async fn resolve_vecs_and_tuples() {
        // Session setup
        let ext0 = FixedSizeChunkerExtension::new(10);
        let ext1 = FixedSizeChunkerExtension::new(20);
        let ext2 = MockEmbedderExtension::new(vec!["the", "cat", "sat", "on", "mat"]);
        let ext3 = MockEmbedderExtension::new(vec!["dog", "barked", "cat"]);
        let mut session = Session::new();
        add_extension(&mut session, ext0, vec![NOT_USED_FOR_TRAINING]).await.unwrap();
        add_extension(&mut session, ext1, vec![GDPR]).await.unwrap();
        add_extension(&mut session, ext2, vec![NOT_USED_FOR_TRAINING]).await.unwrap();
        add_extension(&mut session, ext3, vec![GDPR]).await.unwrap();
        assert_eq!(session.extensions.len(), 4);

        // Resolve items and vectors
        let chunker: Comp<dyn Chunker> = session.resolve().unwrap();
        assert_eq!(chunker.chunk("01234567890123456789").unwrap().len(), 2);
        let chunkers: Vec<Comp<dyn Chunker>> = session.resolve().unwrap();
        assert_eq!(chunkers.len(), 2);
        assert_eq!(chunkers[0].chunk("01234567890123456789").unwrap().len(), 2);
        assert_eq!(chunkers[1].chunk("01234567890123456789").unwrap().len(), 1);

        // Resolve tuples
        let tuple: (Comp<dyn Chunker>, Comp<dyn EmbeddingModel>) = session.resolve().unwrap();
        assert_eq!(tuple.0.chunk("01234567890123456789").unwrap().len(), 2);
        assert_eq!(tuple.1.dimensions().unwrap(), 5);
        let vec_tuple: (Vec<Comp<dyn Chunker>>, Vec<Comp<dyn EmbeddingModel>>) = session.resolve().unwrap();
        assert_eq!(vec_tuple.0.len(), 2);
        assert_eq!(vec_tuple.1.len(), 2);
        assert_eq!(vec_tuple.1[0].dimensions().unwrap(), 5);
    }

    #[tokio::test]
    async fn resolve_arc_and_box() {
        // Session setup
        let ext0 = MockEmbedderExtension::new(vec!["the", "cat", "sat", "on", "mat"]);
        let ext1 = MockEmbedderExtension::new(vec!["dog", "barked", "cat"]);
        let mut session = Session::new();
        add_extension(&mut session, ext0, vec![]).await.unwrap();
        add_extension(&mut session, ext1, vec![]).await.unwrap();

        // Box
        let boxes: Vec<Box<Comp<dyn EmbeddingModel>>> = session.resolve().unwrap();
        assert_eq!(boxes[0].dimensions().unwrap(), 5);
        assert_eq!(boxes[1].dimensions().unwrap(), 3);

        // Arc
        let arcs: Vec<Arc<Comp<dyn EmbeddingModel>>> = session.resolve().unwrap();
        assert_eq!(arcs[0].dimensions().unwrap(), 5);
        assert_eq!(arcs[1].dimensions().unwrap(), 3);
    }

    #[tokio::test]
    async fn resolve_mutex() {
        // Session setup
        let ext0 = MockEmbedderExtension::new(vec!["the", "cat", "sat", "on", "mat"]);
        let ext1 = MockEmbedderExtension::new(vec!["dog", "barked", "cat"]);
        let mut session = Session::new();
        add_extension(&mut session, ext0, vec![]).await.unwrap();
        add_extension(&mut session, ext1, vec![]).await.unwrap();

        // Resolve
        let mut mutexes: Vec<Mutex<Comp<dyn EmbeddingModel>>> = session.resolve().unwrap();
        assert_eq!(mutexes[0].get_mut().unwrap().dimensions().unwrap(), 5);
        assert_eq!(mutexes[1].get_mut().unwrap().dimensions().unwrap(), 3);
    }

    #[tokio::test]
    async fn provide_with_values_cached_due_to_sorting() {
        #[derive(Provide)]
        #[provide(crate = "crate")]
        struct TestExtension {
            // Sort in ascending order of # dimensions (i.e., number of words in vocabulary, in 
            // our example). This won't actually change the order (already sorted, see below), but
            // it will cause the `Comp` instances to be provided eagerly rather than lazily.
            #[provide(each, sort_by = |a, b| a.dimensions().unwrap().cmp(&b.dimensions().unwrap()))]
            _embedder: Comp<dyn EmbeddingModel>,
        }

        impl Extension for TestExtension {
            fn uri(&self) -> &str           { "markhor://test-extension" }
            fn name(&self) -> &str          { "test-extension" }
            fn description(&self) ->  &str  { "Test extension" }
        }

        // Session setup
        let ext0 = MockEmbedderExtension::new(vec!["dog", "barked", "cat"]);
        let ext1 = MockEmbedderExtension::new(vec!["the", "cat", "sat", "on", "mat"]);
        let mut session = Session::new();
        add_extension(&mut session, ext0, vec![NOT_USED_FOR_TRAINING]).await.unwrap();
        add_extension(&mut session, ext1, vec![PUBLIC]).await.unwrap();
        assert_eq!(session.extensions.len(), 2);

        // Initialize TestExtension
        session.initialize_extension_with_multiple_instances::<TestExtension>(Default::default()).await.unwrap();
        assert_eq!(session.extensions.len(), 4);
        assert_eq!(session.extensions[2].item.name(), "test-extension");
        assert_eq!(session.extensions[3].item.name(), "test-extension");

        // FAILS
        // Before 1st instance of TestExtension is constructed, both Comp<dyn EmbeddingModel>
        // instances are provided and sorted. Session attributes their permission constraints to
        // the 1st instance of TestExtension, granting only PUBLIC.
        assert_eq!(session.extensions[2].permissions_granted(), vec![NOT_USED_FOR_TRAINING]);

        // Also FAILS
        // When 2nd instance of TestExtension is constructed, 2nd Comp<dyn EmbeddingModel> has
        // already been cached. Session tracks no further permissions constraints, granting
        // the default ON_DEVICE (!), higher than either NOT_USED_FOR_TRAINING or PUBLIC.
        assert_eq!(session.extensions[3].permissions_granted(), vec![PUBLIC]);
    }

    #[tokio::test]
    async fn provide_with_values_cached_due_to_nested_iteration() {
        #[derive(Provide)]
        #[provide(crate = "crate")]
        struct TestExtension {
            #[provide(each)]
            _embedder0: Arc<Comp<dyn EmbeddingModel>>,
            #[provide(each)]
            _embedder1: Arc<Comp<dyn EmbeddingModel>>,
        }

        impl Extension for TestExtension {
            fn uri(&self) -> &str           { "markhor://test-extension" }
            fn name(&self) -> &str          { "test-extension" }
            fn description(&self) ->  &str  { "Test extension" }
        }

        // Session setup
        let ext0 = MockEmbedderExtension::new(vec!["dog", "barked", "cat"]);
        let ext1 = MockEmbedderExtension::new(vec!["the", "cat", "sat", "on", "mat"]);
        let mut session = Session::new();
        add_extension(&mut session, ext0, vec![NOT_USED_FOR_TRAINING]).await.unwrap();
        add_extension(&mut session, ext1, vec![PUBLIC]).await.unwrap();
        assert_eq!(session.extensions.len(), 2);

        // Initialize TestExtension
        session.initialize_extension_with_multiple_instances::<TestExtension>(Default::default()).await.unwrap();
        assert_eq!(session.extensions.len(), 6);

        // NOT_USED_FOR_TRAINING & NOT_USED_FOR_TRAINING = NOT_USED_FOR_TRAINING
        // FAILS (mistakenly takes all instances into account, granting only PUBLIC)
        assert_eq!(session.extensions[2].permissions_granted(), vec![NOT_USED_FOR_TRAINING]);

        // NOT_USED_FOR_TRAINING & PUBLIC = PUBLIC
        // FAILS (no additional instances created, grants ON_DEVICE)
        assert_eq!(session.extensions[3].permissions_granted(), vec![PUBLIC]);

        // PUBLIC & NOT_USED_FOR_TRAINING = PUBLIC
        // OK (due to iterating outermost loop, which is not cached)
        assert_eq!(session.extensions[4].permissions_granted(), vec![PUBLIC]);

        // PUBLIC & PUBLIC = PUBLIC
        // FAILS (no additional instances created, grants ON_DEVICE)
        assert_eq!(session.extensions[5].permissions_granted(), vec![PUBLIC]);
    }

    #[tokio::test]
    async fn provide_with_values_cached_due_to_shared_ownership() {
        /// Struct for which `Provide` offers exactly two instances.
        struct Twice;

        impl Provide for Twice {
            type Item = Self;

            fn iter(_session: &Session) -> Result<impl Iterator<Item = Self>, ResolveDependencyError> {
                Ok(std::iter::once(Twice).chain(std::iter::once(Twice)))
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
        struct TestExtension {
            _embedder: Arc<Comp<dyn EmbeddingModel>>,
            #[provide(each)]
            _twice: Twice,
        }

        impl Extension for TestExtension {
            fn uri(&self) -> &str           { "markhor://test-extension" }
            fn name(&self) -> &str          { "test-extension" }
            fn description(&self) ->  &str  { "Test extension" }
        }

        // Session setup
        let ext = MockEmbedderExtension::new(vec!["the", "cat", "sat", "on", "mat"]);
        let mut session = Session::new();
        add_extension(&mut session, ext, vec![NOT_USED_FOR_TRAINING]).await.unwrap();
        assert_eq!(session.extensions.len(), 1);

        // Initialize TestExtension
        session.initialize_extension_with_multiple_instances::<TestExtension>(Default::default()).await.unwrap();
        assert_eq!(session.extensions.len(), 3);
        assert_eq!(session.extensions[1].item.name(), "test-extension");
        assert_eq!(session.extensions[2].item.name(), "test-extension");

        // One instance of Comp<dyn EmbeddingModel> is constructed, then each TestExtension 
        // instance only requires cloning an Arc. Session has nothing further to track.
        // Still OK
        assert_eq!(session.extensions[1].permissions_granted(), vec![NOT_USED_FOR_TRAINING]);

        // FAILS
        // Session defaults to granting ON_DEVICE.
        assert_eq!(session.extensions[2].permissions_granted(), vec![NOT_USED_FOR_TRAINING]);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn provide_with_iteration_and_concurrent_cloning() {
        #[derive(Provide)]
        #[provide(crate = "crate")]
        struct TestExtension {
            #[provide(each, map = |api_key: ApiKey| clone_later(api_key))]
            api_key: ApiKey,
        }

        impl Extension for TestExtension {
            fn uri(&self) -> &str           { "markhor://test-extension" }
            fn name(&self) -> &str          { "test-extension" }
            fn description(&self) ->  &str  { self.api_key.project() }
        }

        // The basic idea here is simply that the `Provide` implementation spawns a task that
        // clones an API key concurrently.
        // TRIGGER is just an implementation detail used to reproduce the issue consistently
        // during testing.
        static TRIGGER: OnceLock<AtomicBool> = OnceLock::new();
        TRIGGER.get_or_init(|| AtomicBool::new(false));

        fn clone_later(api_key: ApiKey) -> ApiKey {
            let trigger = TRIGGER.get().unwrap();
            if api_key.project() == "project-a" {
                let mut vec = vec![api_key.clone()];
                task::spawn(async move {
                    // Wait for signal to clone
                    while !trigger.load(Ordering::Acquire) {
                        task::yield_now().await;
                    }
                    vec.push(vec[0].clone());
                    // Indicate that cloning is done
                    trigger.store(false, Ordering::Release);
                });
            } else {
                // Send signal to clone
                trigger.store(true, Ordering::Release);
                // Wait for cloning to finish
                while trigger.load(Ordering::Acquire) {
                    std::thread::yield_now();
                }
            }
            api_key
        }

        // Session setup
        let mut session = Session::new();
        session.add_api_key(ApiKey::new(
            "1234567890abcdef".to_string(),
            "TestProvider".to_string(),
            "project-a".to_string(),
        ), vec![NOT_USED_FOR_TRAINING]);
        session.add_api_key(ApiKey::new(
            "abcdef1234567890".to_string(),
            "TestProvider".to_string(),
            "project-b".to_string(),
        ), vec![GDPR]);

        // Initialize TestExtension
        session.initialize_extension_with_multiple_instances::<TestExtension>(Default::default()).await.unwrap();
        assert_eq!(session.extensions.len(), 2);
        assert_eq!(session.extensions[0].item.description(), "project-a");
        assert_eq!(session.extensions[1].item.description(), "project-b");

        // Permissions should be tracked correctly, even though the first API key was cloned
        // concurrently during the resolution of the second instance of `TextExtension`.
        assert_eq!(session.extensions[0].permissions_granted(), vec![NOT_USED_FOR_TRAINING]);
        // FAILS (only granted permission PUBLIC)
        assert_eq!(session.extensions[1].permissions_granted(), vec![GDPR]);
    }

}