use thiserror::Error;
use tokio::fs;
use tracing::warn;

use crate::{
    dependencies::{
        BuildFrom, Provide, ResolveDependencyError, api_key::ApiKey,
        dependency_slot::DependencySlot,
    },
    extension::{Extension, ExtensionConfig, InitExtensionError},
    library::{AccessLibraryError, Document, Folder, Scope},
    permissions::{self, Actor, Authorized, Permission, Resource, Restricted, all_permissions},
};

use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::Arc,
};

pub struct Session {
    folder: Option<Folder>,
    documents: BTreeMap<PathBuf, Document>,
    api_keys: Vec<DependencySlot<ApiKey>>,
    extensions: Vec<DependencySlot<Arc<dyn Extension>>>,
    // Acts as an upper bound for what the session may access (imposed externally downstream, all
    // permissions by default). Not totally sure yet if this is helpful or confusing (consider
    // renaming field to avoid parallel with `permissions_required` and/or remove `Authorized`
    // implementation).
    permissions_granted: Vec<Permission>,
    // Represents the permissions required to access any documents (and other resources) used by
    // the session. Extensions used by the session must meet this threshold as well as any outside
    // actors with access to the session.
    permissions_required: Vec<Permission>,
}

impl Session {
    pub fn new() -> Self {
        Self {
            folder: None,
            documents: BTreeMap::new(),
            api_keys: vec![],
            extensions: vec![],
            permissions_granted: all_permissions(),
            permissions_required: vec![],
        }
    }

    /// Creates a session whose working folder is `folder`.
    pub fn with_folder(folder: Folder) -> Self {
        Self {
            folder: Some(folder),
            ..Self::new()
        }
    }

    pub fn resolve<T: Provide>(&self) -> Result<T, ResolveDependencyError> {
        let assets = Assets::new_for_resolve_call(self);
        T::first(&assets)
    }

    pub(crate) fn add_api_key(&mut self, api_key: ApiKey, permissions: Vec<Permission>) {
        self.api_keys
            .push(DependencySlot::new(api_key, permissions));
    }

    /// Reads API keys from environment variables and adds them to the session.
    ///
    /// Each key is declared as `[PROJECT_]PROVIDER_API_KEY=<key>` and must have a matching
    /// `[PROJECT_]PROVIDER_API_KEY_PERMISSIONS=<names>` variable containing a comma-separated list
    /// of permission names (e.g. `public,on-device`). The provider is the last segment before
    /// `_API_KEY`; any preceding segments form the project, which is optional.
    ///
    /// Nothing is added if any variable is invalid.
    pub fn add_api_keys_from_env(&mut self) -> Result<(), AddApiKeysError> {
        self.add_api_keys_from_vars(std::env::vars())
    }

    fn add_api_keys_from_vars<I>(&mut self, vars: I) -> Result<(), AddApiKeysError>
    where
        I: IntoIterator<Item = (String, String)>,
    {
        let vars: BTreeMap<String, String> = vars.into_iter().collect();

        for name in vars.keys().filter(|n| n.ends_with("_API_KEY_PERMISSIONS")) {
            let key_name = &name[..name.len() - "_PERMISSIONS".len()];
            if !vars.contains_key(key_name) {
                return Err(AddApiKeysError::MissingKey(key_name.to_string()));
            }
        }

        let mut api_keys = vec![];
        for (name, key) in &vars {
            let Some(prefix) = name.strip_suffix("_API_KEY") else {
                continue;
            };
            let (project, provider) = prefix.rsplit_once('_').unwrap_or(("", prefix));
            if provider.is_empty() {
                return Err(AddApiKeysError::InvalidName(name.clone()));
            }

            let permissions_var = format!("{name}_PERMISSIONS");
            let permissions_value = vars
                .get(&permissions_var)
                .ok_or_else(|| AddApiKeysError::MissingPermissions(name.clone()))?;
            let permissions = permissions_value
                .split(',')
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(|s| {
                    permissions::permission_by_name(s).ok_or_else(|| {
                        AddApiKeysError::UnknownPermission {
                            var: permissions_var.clone(),
                            permission: s.to_string(),
                        }
                    })
                })
                .collect::<Result<Vec<_>, _>>()?;

            let api_key = ApiKey::new(key.clone(), provider.to_string(), project.to_string());
            api_keys.push((api_key, permissions));
        }

        for (api_key, permissions) in api_keys {
            self.add_api_key(api_key, permissions);
        }
        Ok(())
    }

    pub async fn add_extension<T, E>(
        &mut self,
        value: T,
        config: ExtensionConfig,
    ) -> Result<(), InitExtensionError>
    where
        T: Clone,
        E: Extension + BuildFrom<T> + 'static,
        E::Requirements: Provide,
    {
        self.add_extension_with::<T, _, E>(move || value.clone(), config)
            .await
    }

    pub async fn add_extension_with<T, F, E>(
        &mut self,
        mut f: F,
        config: ExtensionConfig,
    ) -> Result<(), InitExtensionError>
    where
        F: FnMut() -> T,
        E: Extension + BuildFrom<T> + 'static,
        E::Requirements: Provide,
    {
        // Begin dependency tracking for API keys and extensions
        for slot in &mut self.api_keys {
            slot.replace_tracker();
        }
        for slot in &mut self.extensions {
            slot.replace_tracker();
        }
        let tracker = PermissionTracker { session: &self };

        let assets = Assets::new_for_extension_dependencies(&self);
        let mut requirements_by_instances = vec![];
        let r = match E::Requirements::iter(&assets) {
            Ok(iter) => {
                for item in iter {
                    let permissions = tracker.permissions_granted(config.permissions.clone());
                    tracker.reset_tracking_count();
                    if Actor(&permissions).may_access(self) {
                        requirements_by_instances.push((item, permissions));
                    } else {
                        // TODO: More information would be nice here, at least via a parent span,
                        // if not directly
                        warn!(
                            "Extension instance has insufficient permissions: {:?}",
                            permissions
                        );
                    }
                }
                if requirements_by_instances.is_empty() {
                    return Err(InitExtensionError::Permission(
                        self.permissions_required().to_vec(),
                    ));
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

        for (requirements, permissions) in requirements_by_instances {
            let mut extension = E::build_from(f(), requirements);
            extension.initialize().await?;
            self.extensions
                .push(DependencySlot::new(Arc::new(extension), permissions));
        }

        r
    }

    // This method will probably not be the only (or even the primary) way to add documents to a
    // session. Still, this implementation outlines how document permissions relate to session
    // permissions. Other ways of adding documents should conform to the same constraints.
    fn add_document(&mut self, document: Document) -> Result<(), AddDocumentError> {
        self.check_access(&document)?;
        self.documents
            .insert(document.absolute_path.clone(), document);
        Ok(())
    }

    fn check_access(&self, document: &Document) -> Result<(), AddDocumentError> {
        // This session must have been granted permission to access the document
        if !self.may_access(document) {
            return Err(AddDocumentError::NotAuthorized(
                document.permissions_required().to_vec(),
            ));
        }

        // Anything with access to this session must have permission to access the document
        // This includes the session's own extensions
        if !Actor(&self.permissions_required).may_access(document) {
            return Err(AddDocumentError::ElevationRequired(
                document.permissions_required().to_vec(),
            ));
        }

        Ok(())
    }

    /// Returns the document at `path` and adds it to the session. A relative `path` is resolved
    /// against the working folder.
    ///
    /// Documents are cached by canonical path. The document must be in the working folder's
    /// collection, and the session must have the permissions it requires (see `add_document`).
    pub async fn document(
        &mut self,
        path: impl AsRef<Path>,
    ) -> Result<&Document, AccessDocumentError> {
        let folder = self
            .folder
            .as_ref()
            .ok_or(AccessDocumentError::NoWorkingFolder)?;
        let key = canonical_document_path(folder, path.as_ref()).await?;
        if self.documents.contains_key(&key) {
            // Permissions may have changed since the document was added
            self.check_access(&self.documents[&key])?;
        } else {
            let document = folder.document(&key).await?;
            self.add_document(document)?;
        }
        Ok(&self.documents[&key])
    }

    // TODO: Bikeshed method name and error type
    // The thing that's being restricted is access to documents (and any other `Restricted`
    // resources) in the session. The things whose access is being restricted are extensions, API
    // keys (and any other `Authorized` assets within the session or outside actors with accesss
    // to the session).
    // The use case is that you might want to add a protected document to a session (e.g., data
    // that must not leave the device). To do that, you first have to remove any extensions etc.
    // that lack the necessary permission, which is what this method is for. As a result, you're
    // actually elevating the session's authorization in the sense that the restricted document
    // can now be added. But you're also removing access from certain actors. So you're granting
    // or restricting access depending on what you're talking about.
    // The method name doesn't commnicate any of that very well, which could be confusing.
    /// Raises the permissions required of the session's actors to include `permissions`.
    ///
    /// Fails without changing the session if the resulting requirement exceeds what the session
    /// was granted, or if an in-use API key or extension lacks the required permissions. Unused
    /// API keys and extensions that lack them are removed.
    pub fn restrict_access(
        &mut self,
        permissions: Vec<Permission>,
    ) -> Result<(), RestrictAccessError> {
        let target = Resource(Permission::union(&self.permissions_required, &permissions));

        if !self.may_access(&target) {
            // Would entail removing all extensions (& other actors), which is unlikely to be what
            // is desired
            return Err(RestrictAccessError::PermissionDenied(
                self.permissions_granted.to_vec(),
            ));
        }

        // Check every actor before changing anything, so a failed call leaves the session as it was
        let active_keys: Vec<String> = self
            .api_keys
            .iter()
            .filter(|slot| slot.is_active() && !slot.may_access(&target))
            .map(|slot| slot.item.provider().to_string())
            .collect();
        if !active_keys.is_empty() {
            return Err(RestrictAccessError::ApiKeyNotAuthorized(active_keys));
        }

        let active_extensions: Vec<String> = self
            .extensions
            .iter()
            .filter(|slot| slot.is_active() && !slot.may_access(&target))
            .map(|slot| slot.item.name().to_string())
            .collect();
        if !active_extensions.is_empty() {
            return Err(RestrictAccessError::ExtensionNotAuthorized(
                active_extensions,
            ));
        }

        // Only inactive actors lacking the permissions remain, so they can be removed
        self.api_keys.retain(|slot| slot.may_access(&target));
        self.extensions.retain(|slot| slot.may_access(&target));
        self.permissions_required = target.0;
        Ok(())
    }
}

impl Authorized for Session {
    fn permissions_granted(&self) -> &[Permission] {
        &self.permissions_granted
    }
}

impl Restricted for Session {
    fn permissions_required(&self) -> &[Permission] {
        &self.permissions_required
    }
}

pub struct Assets<'a> {
    session: &'a Session,
    context: AssetCx,
}

impl<'a> Assets<'a> {
    pub(crate) fn new_for_extension_dependencies(session: &'a Session) -> Self {
        Self {
            session,
            context: AssetCx::ExtensionDependencies,
        }
    }

    pub(crate) fn new_for_resolve_call(session: &'a Session) -> Self {
        Self {
            session,
            context: AssetCx::ResolveCall,
        }
    }

    pub(crate) fn api_keys(&self) -> &[DependencySlot<ApiKey>] {
        if self.context.api_keys() {
            &self.session.api_keys
        } else {
            &[]
        }
    }

    pub(crate) fn extensions(&self) -> &[DependencySlot<Arc<dyn Extension>>] {
        if self.context.extensions() {
            &self.session.extensions
        } else {
            &[]
        }
    }
}

enum AssetCx {
    ExtensionDependencies,
    ResolveCall,
}

impl AssetCx {
    fn api_keys(&self) -> bool {
        matches!(self, AssetCx::ExtensionDependencies)
    }

    fn extensions(&self) -> bool {
        true
    }

    fn library(&self) -> bool {
        matches!(self, AssetCx::ResolveCall)
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

/// Resolves `path` (relative to `folder` unless absolute) to a canonical path, which is the key
/// under which a session caches documents.
async fn canonical_document_path(
    folder: &Folder,
    path: &Path,
) -> Result<PathBuf, AccessLibraryError> {
    let path = if path.is_absolute() {
        path.to_path_buf()
    } else {
        folder.collection().path().join(folder.path()).join(path)
    };
    fs::canonicalize(&path).await.map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            AccessLibraryError::FileNotFound(path.clone())
        } else {
            AccessLibraryError::Io(e)
        }
    })
}

#[derive(Debug, Error)]
pub enum AddDocumentError {
    #[error("Session lacks required permissions: {}",
        .0.iter().map(|p| p.name()).collect::<Vec<_>>().join(", "))]
    NotAuthorized(Vec<Permission>),

    #[error("Session restrictions must be elevated to required permissions: {}",
        .0.iter().map(|p| p.name()).collect::<Vec<_>>().join(", "))]
    ElevationRequired(Vec<Permission>),
}

#[derive(Debug, Error)]
pub enum AccessDocumentError {
    #[error("Session has no working folder")]
    NoWorkingFolder,

    #[error(transparent)]
    Library(#[from] AccessLibraryError),

    #[error(transparent)]
    Permission(#[from] AddDocumentError),
}

#[derive(Debug, Error)]
pub enum AddApiKeysError {
    #[error("Environment variable {0} has no matching {0}_PERMISSIONS variable")]
    MissingPermissions(String),

    #[error("Environment variable {0}_PERMISSIONS has no matching {0} variable")]
    MissingKey(String),

    #[error("Unknown permission '{permission}' in environment variable {var}")]
    UnknownPermission { var: String, permission: String },

    #[error("Cannot determine provider from environment variable {0}")]
    InvalidName(String),
}

#[derive(Debug, Error)]
pub enum RestrictAccessError {
    #[error("API keys lack required permissions: {}", .0.join(", "))]
    ApiKeyNotAuthorized(Vec<String>),

    #[error("Extensions lack required permissions: {}", .0.join(", "))]
    ExtensionNotAuthorized(Vec<String>),

    #[error("Target permissions exceed those granted to extension: {}",
        .0.iter().map(|p| p.name()).collect::<Vec<_>>().join(", "))]
    PermissionDenied(Vec<Permission>),
}

#[cfg(test)]
mod tests {
    use tokio::task;

    use super::*;
    use crate::chunking::{Chunker, test_chunker::FixedSizeChunkerExtension};
    use crate::dependencies::ResolveDependencyError;
    use crate::embedding::EmbeddingModel;
    use crate::embedding::test_utils::MockEmbedderExtension;
    use crate::extension::Comp;
    use crate::library::NewSource;
    use crate::library::fs_test_utils::{TempTree, fs_tree};
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
        let mut iter = std::iter::once(extension);
        session
            .add_extension_with::<E, _, E>(
                // Will never be called more than once because <E as BuildFrom<E>>::Requirements = (),
                // which only has one value.
                move || iter.next().unwrap(),
                ExtensionConfig { permissions },
            )
            .await
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
            fn uri(&self) -> &str {
                "markhor://test-extension-0"
            }
            fn name(&self) -> &str {
                "test-extension-0"
            }
            fn description(&self) -> &str {
                "Test extension 0"
            }
        }

        #[derive(Provide)]
        #[provide(crate = "crate")]
        struct TestExtension1 {
            // uses all chunkers
            _chunkers: Vec<Comp<dyn Chunker>>,
        }

        impl Extension for TestExtension1 {
            fn uri(&self) -> &str {
                "markhor://test-extension-1"
            }
            fn name(&self) -> &str {
                "test-extension-1"
            }
            fn description(&self) -> &str {
                "Test extension 1"
            }
        }

        // Session setup
        let ext0 = FixedSizeChunkerExtension::new(10);
        let ext1 = FixedSizeChunkerExtension::new(20);
        let mut session = Session::new();
        add_extension(&mut session, ext0, vec![NOT_USED_FOR_TRAINING])
            .await
            .unwrap();
        add_extension(&mut session, ext1, vec![PUBLIC])
            .await
            .unwrap();
        assert_eq!(session.extensions.len(), 2);

        // Initialize TestExtension0, which should be granted permission NOT_USED_FOR_TRAINING
        session
            .add_extension::<_, TestExtension0>((), Default::default())
            .await
            .unwrap();
        assert_eq!(session.extensions.len(), 3);
        assert_eq!(session.extensions[2].item.name(), "test-extension-0");
        assert_eq!(
            session.extensions[2].permissions_granted(),
            vec![NOT_USED_FOR_TRAINING]
        );

        // Initialize TestExtension1, which should be granted only permission PUBLIC
        session
            .add_extension::<_, TestExtension1>((), Default::default())
            .await
            .unwrap();
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
            fn uri(&self) -> &str {
                "markhor://test-extension-0"
            }
            fn name(&self) -> &str {
                "test-extension-0"
            }
            fn description(&self) -> &str {
                "Test extension 0"
            }
        }

        #[derive(Provide)]
        #[provide(crate = "crate")]
        struct TestExtension1 {
            // uses all API keys
            _api_keys: Vec<ApiKey>,
        }

        impl Extension for TestExtension1 {
            fn uri(&self) -> &str {
                "markhor://test-extension-1"
            }
            fn name(&self) -> &str {
                "test-extension-1"
            }
            fn description(&self) -> &str {
                "Test extension 1"
            }
        }

        struct TestExtension2 {
            // uses single API key, but as a string!
            _api_key: String,
        }

        impl Extension for TestExtension2 {
            fn uri(&self) -> &str {
                "markhor://test-extension-2"
            }
            fn name(&self) -> &str {
                "test-extension-2"
            }
            fn description(&self) -> &str {
                "Test extension 2"
            }
        }

        impl Provide for TestExtension2 {
            type Item = TestExtension2;

            fn iter(assets: &Assets) -> Result<impl Iterator<Item = Self>, ResolveDependencyError> {
                // Get API key during resolution, convert to string, then drop `ApiKey`
                let api_key = ApiKey::first(assets)?;
                Ok(std::iter::once(TestExtension2 {
                    _api_key: api_key.key().to_string(),
                }))
            }

            fn iter_from_items<I>(
                items: I,
            ) -> Result<impl Iterator<Item = Self>, ResolveDependencyError>
            where
                I: Iterator<Item = Self::Item>,
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
            fn uri(&self) -> &str {
                "markhor://test-extension-3"
            }
            fn name(&self) -> &str {
                "test-extension-3"
            }
            fn description(&self) -> &str {
                "Test extension 3"
            }
        }

        let mut session = Session::new();
        session.add_api_key(
            ApiKey::new(
                "1234567890abcdef".to_string(),
                "TestProvider".to_string(),
                "TestProject".to_string(),
            ),
            vec![GDPR],
        );
        session.add_api_key(
            ApiKey::new(
                "abcdef1234567890".to_string(),
                "AnotherProvider".to_string(),
                "AnotherProject".to_string(),
            ),
            vec![NOT_USED_FOR_TRAINING],
        );
        session.add_api_key(
            ApiKey::new(
                "0987654321fedcba".to_string(),
                "YetAnotherProvider".to_string(),
                "YetAnotherProject".to_string(),
            ),
            vec![PUBLIC],
        );

        // Initialize TestExtension0, which should be granted only permission GDPR
        session
            .add_extension::<_, TestExtension0>((), Default::default())
            .await
            .unwrap();
        assert_eq!(session.extensions.len(), 1);
        assert_eq!(session.extensions[0].item.name(), "test-extension-0");
        assert_eq!(session.extensions[0].permissions_granted(), vec![GDPR]);

        // Initialize TestExtension1, which should be granted only permission PUBLIC
        session
            .add_extension::<_, TestExtension1>((), Default::default())
            .await
            .unwrap();
        assert_eq!(session.extensions.len(), 2);
        assert_eq!(session.extensions[1].item.name(), "test-extension-1");
        assert_eq!(session.extensions[1].permissions_granted(), vec![PUBLIC]);

        // Initialize TestExtension2, which should be granted permission GDPR
        // even though the extension doesn't store the key
        session
            .add_extension::<_, TestExtension2>((), Default::default())
            .await
            .unwrap();
        assert_eq!(session.extensions.len(), 3);
        assert_eq!(session.extensions[2].item.name(), "test-extension-2");
        assert_eq!(session.extensions[2].permissions_granted(), vec![GDPR]);

        // Initialize TestExtension3, which should be granted only permission NOT_USED_FOR_TRAINING
        // as per API key "AnotherProvider"
        session
            .add_extension::<_, TestExtension3>((), Default::default())
            .await
            .unwrap();
        assert_eq!(session.extensions.len(), 4);
        assert_eq!(session.extensions[3].item.name(), "test-extension-3");
        assert_eq!(
            session.extensions[3].permissions_granted(),
            vec![NOT_USED_FOR_TRAINING]
        );
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

            fn iter(assets: &Assets) -> Result<impl Iterator<Item = Self>, ResolveDependencyError> {
                let mut chunker = Some(Comp::<dyn Chunker>::first(assets)?);
                Ok(std::iter::from_fn(move || {
                    chunker.take().map(|c| {
                        BARRIER.get().unwrap().wait();
                        // Some potentially problematic activity occurs here on another thread
                        BARRIER.get().unwrap().wait();
                        TestExtension0 { _chunker: c }
                    })
                }))
            }

            fn iter_from_items<I>(
                items: I,
            ) -> Result<impl Iterator<Item = Self>, ResolveDependencyError>
            where
                I: Iterator<Item = Self::Item>,
            {
                Ok(items)
            }
        }

        impl Extension for TestExtension0 {
            fn uri(&self) -> &str {
                "markhor://test-extension-0"
            }
            fn name(&self) -> &str {
                "test-extension-0"
            }
            fn description(&self) -> &str {
                "Test extension 0"
            }
        }

        // Session setup
        let ext0 = FixedSizeChunkerExtension::new(10);
        let mut session = Session::new();
        add_extension(&mut session, ext0, vec![NOT_USED_FOR_TRAINING])
            .await
            .unwrap();
        assert_eq!(session.extensions.len(), 1);
        let assets = Assets::new_for_resolve_call(&session);

        // Simulate that `FixedSizeChunkerExtension` is already in use and its pointer dropped at
        // an inopportune time.
        let chunker0 = Some(Comp::<dyn Chunker>::first(&assets).unwrap());
        tokio::spawn(async move {
            BARRIER.get().unwrap().wait();
            mem::drop(chunker0);
            BARRIER.get().unwrap().wait();
        });

        // Initialize TestExtension0, which should be granted only permission NOT_USED_FOR_TRAINING
        // However, the existing `Comp` referencing the same `FixedSizeChunkerExtension` will be
        // dropped *while* `TestExtension0` is initialized.
        session
            .add_extension::<_, TestExtension0>((), Default::default())
            .await
            .unwrap();

        // Nevertheless, if permissions are tracked correctly, `TestExtension0` should end up
        // with the same requirements as `FixedSizeChunkerExtension`.
        assert_eq!(session.extensions.len(), 2);
        assert_eq!(session.extensions[1].item.name(), "test-extension-0");
        assert_eq!(
            session.extensions[0].permissions_granted(),
            vec![NOT_USED_FOR_TRAINING]
        );
        assert_eq!(
            session.extensions[1].permissions_granted(),
            vec![NOT_USED_FOR_TRAINING]
        );
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
            fn uri(&self) -> &str {
                "markhor://test-extension"
            }
            fn name(&self) -> &str {
                "test-extension"
            }
            fn description(&self) -> &str {
                "Test extension"
            }
        }

        // Session setup
        let ext0 = FixedSizeChunkerExtension::new(10);
        let ext1 = FixedSizeChunkerExtension::new(20);
        let mut session = Session::new();
        add_extension(&mut session, ext0, vec![NOT_USED_FOR_TRAINING])
            .await
            .unwrap();
        add_extension(&mut session, ext1, vec![GDPR]).await.unwrap();
        assert_eq!(session.extensions.len(), 2);

        // Initialize TestExtension
        session
            .add_extension::<_, TestExtension>((), Default::default())
            .await
            .unwrap();
        assert_eq!(session.extensions.len(), 4);
        assert_eq!(session.extensions[2].item.name(), "test-extension");
        assert_eq!(session.extensions[3].item.name(), "test-extension");
        assert_eq!(
            session.extensions[2].permissions_granted(),
            vec![NOT_USED_FOR_TRAINING]
        );
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
        add_extension(&mut session, ext0, vec![NOT_USED_FOR_TRAINING])
            .await
            .unwrap();
        add_extension(&mut session, ext1, vec![GDPR]).await.unwrap();
        add_extension(&mut session, ext2, vec![NOT_USED_FOR_TRAINING])
            .await
            .unwrap();
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
        let vec_tuple: (Vec<Comp<dyn Chunker>>, Vec<Comp<dyn EmbeddingModel>>) =
            session.resolve().unwrap();
        assert_eq!(vec_tuple.0.len(), 2);
        assert_eq!(vec_tuple.1.len(), 2);
        assert_eq!(vec_tuple.1[0].dimensions().unwrap(), 5);

        // Unit value
        let unit: () = session.resolve().unwrap();
        assert_eq!(unit, ());
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
    #[ignore = "waiting for dependency tracking redesign/alternative"]
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
            fn uri(&self) -> &str {
                "markhor://test-extension"
            }
            fn name(&self) -> &str {
                "test-extension"
            }
            fn description(&self) -> &str {
                "Test extension"
            }
        }

        // Session setup
        let ext0 = MockEmbedderExtension::new(vec!["dog", "barked", "cat"]);
        let ext1 = MockEmbedderExtension::new(vec!["the", "cat", "sat", "on", "mat"]);
        let mut session = Session::new();
        add_extension(&mut session, ext0, vec![NOT_USED_FOR_TRAINING])
            .await
            .unwrap();
        add_extension(&mut session, ext1, vec![PUBLIC])
            .await
            .unwrap();
        assert_eq!(session.extensions.len(), 2);

        // Initialize TestExtension
        session
            .add_extension::<_, TestExtension>((), Default::default())
            .await
            .unwrap();
        assert_eq!(session.extensions.len(), 4);
        assert_eq!(session.extensions[2].item.name(), "test-extension");
        assert_eq!(session.extensions[3].item.name(), "test-extension");

        // FAILS
        // Before 1st instance of TestExtension is constructed, both Comp<dyn EmbeddingModel>
        // instances are provided and sorted. Session attributes their permission constraints to
        // the 1st instance of TestExtension, granting only PUBLIC.
        assert_eq!(
            session.extensions[2].permissions_granted(),
            vec![NOT_USED_FOR_TRAINING]
        );

        // Also FAILS
        // When 2nd instance of TestExtension is constructed, 2nd Comp<dyn EmbeddingModel> has
        // already been cached. Session tracks no further permissions constraints, granting
        // the default ON_DEVICE (!), higher than either NOT_USED_FOR_TRAINING or PUBLIC.
        assert_eq!(session.extensions[3].permissions_granted(), vec![PUBLIC]);
    }

    #[tokio::test]
    #[ignore = "waiting for dependency tracking redesign/alternative"]
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
            fn uri(&self) -> &str {
                "markhor://test-extension"
            }
            fn name(&self) -> &str {
                "test-extension"
            }
            fn description(&self) -> &str {
                "Test extension"
            }
        }

        // Session setup
        let ext0 = MockEmbedderExtension::new(vec!["dog", "barked", "cat"]);
        let ext1 = MockEmbedderExtension::new(vec!["the", "cat", "sat", "on", "mat"]);
        let mut session = Session::new();
        add_extension(&mut session, ext0, vec![NOT_USED_FOR_TRAINING])
            .await
            .unwrap();
        add_extension(&mut session, ext1, vec![PUBLIC])
            .await
            .unwrap();
        assert_eq!(session.extensions.len(), 2);

        // Initialize TestExtension
        session
            .add_extension::<_, TestExtension>((), Default::default())
            .await
            .unwrap();
        assert_eq!(session.extensions.len(), 6);

        // NOT_USED_FOR_TRAINING & NOT_USED_FOR_TRAINING = NOT_USED_FOR_TRAINING
        // FAILS (mistakenly takes all instances into account, granting only PUBLIC)
        assert_eq!(
            session.extensions[2].permissions_granted(),
            vec![NOT_USED_FOR_TRAINING]
        );

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
    #[ignore = "waiting for dependency tracking redesign/alternative"]
    async fn provide_with_values_cached_due_to_shared_ownership() {
        /// Struct for which `Provide` offers exactly two instances.
        struct Twice;

        impl Provide for Twice {
            type Item = Self;

            fn iter(
                _assets: &Assets,
            ) -> Result<impl Iterator<Item = Self>, ResolveDependencyError> {
                Ok(std::iter::once(Twice).chain(std::iter::once(Twice)))
            }

            fn iter_from_items<I>(
                items: I,
            ) -> Result<impl Iterator<Item = Self>, ResolveDependencyError>
            where
                I: Iterator<Item = Self::Item>,
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
            fn uri(&self) -> &str {
                "markhor://test-extension"
            }
            fn name(&self) -> &str {
                "test-extension"
            }
            fn description(&self) -> &str {
                "Test extension"
            }
        }

        // Session setup
        let ext = MockEmbedderExtension::new(vec!["the", "cat", "sat", "on", "mat"]);
        let mut session = Session::new();
        add_extension(&mut session, ext, vec![NOT_USED_FOR_TRAINING])
            .await
            .unwrap();
        assert_eq!(session.extensions.len(), 1);

        // Initialize TestExtension
        session
            .add_extension::<_, TestExtension>((), Default::default())
            .await
            .unwrap();
        assert_eq!(session.extensions.len(), 3);
        assert_eq!(session.extensions[1].item.name(), "test-extension");
        assert_eq!(session.extensions[2].item.name(), "test-extension");

        // One instance of Comp<dyn EmbeddingModel> is constructed, then each TestExtension
        // instance only requires cloning an Arc. Session has nothing further to track.
        // Still OK
        assert_eq!(
            session.extensions[1].permissions_granted(),
            vec![NOT_USED_FOR_TRAINING]
        );

        // FAILS
        // Session defaults to granting ON_DEVICE.
        assert_eq!(
            session.extensions[2].permissions_granted(),
            vec![NOT_USED_FOR_TRAINING]
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    #[ignore = "waiting for dependency tracking redesign/alternative"]
    async fn provide_with_iteration_and_concurrent_cloning() {
        #[derive(Provide)]
        #[provide(crate = "crate")]
        struct TestExtension {
            #[provide(each, map = |api_key: ApiKey| clone_later(api_key))]
            api_key: ApiKey,
        }

        impl Extension for TestExtension {
            fn uri(&self) -> &str {
                "markhor://test-extension"
            }
            fn name(&self) -> &str {
                "test-extension"
            }
            fn description(&self) -> &str {
                self.api_key.project()
            }
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
        session.add_api_key(
            ApiKey::new(
                "1234567890abcdef".to_string(),
                "TestProvider".to_string(),
                "project-a".to_string(),
            ),
            vec![NOT_USED_FOR_TRAINING],
        );
        session.add_api_key(
            ApiKey::new(
                "abcdef1234567890".to_string(),
                "TestProvider".to_string(),
                "project-b".to_string(),
            ),
            vec![GDPR],
        );

        // Initialize TestExtension
        session
            .add_extension::<_, TestExtension>((), Default::default())
            .await
            .unwrap();
        assert_eq!(session.extensions.len(), 2);
        assert_eq!(session.extensions[0].item.description(), "project-a");
        assert_eq!(session.extensions[1].item.description(), "project-b");

        // Permissions should be tracked correctly, even though the first API key was cloned
        // concurrently during the resolution of the second instance of `TextExtension`.
        assert_eq!(
            session.extensions[0].permissions_granted(),
            vec![NOT_USED_FOR_TRAINING]
        );
        // FAILS (only granted permission PUBLIC)
        assert_eq!(session.extensions[1].permissions_granted(), vec![GDPR]);
    }

    fn env_vars(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    fn permission_names(api_key_slot: &DependencySlot<ApiKey>) -> Vec<String> {
        api_key_slot
            .permissions_granted()
            .iter()
            .map(|p| p.name().to_string())
            .collect()
    }

    #[test]
    fn add_api_keys_from_env_with_and_without_project() {
        let mut session = Session::new();
        session
            .add_api_keys_from_vars(env_vars(&[
                ("GEMINI_API_KEY", "gemini-key"),
                ("GEMINI_API_KEY_PERMISSIONS", "public"),
                ("MY_PROJECT_GEMINI_API_KEY", "project-key"),
                (
                    "MY_PROJECT_GEMINI_API_KEY_PERMISSIONS",
                    "on-device, not-used-for-training",
                ),
                ("PATH", "/usr/bin"),
            ]))
            .unwrap();

        assert_eq!(session.api_keys.len(), 2);

        let plain = &session.api_keys[0];
        assert_eq!(plain.item.key(), "gemini-key");
        assert_eq!(plain.item.provider(), "GEMINI");
        assert_eq!(plain.item.project(), "");
        assert_eq!(permission_names(plain), vec!["public"]);

        let project = &session.api_keys[1];
        assert_eq!(project.item.key(), "project-key");
        assert_eq!(project.item.provider(), "GEMINI");
        assert_eq!(project.item.project(), "MY_PROJECT");
        assert_eq!(
            permission_names(project),
            vec!["on-device", "not-used-for-training"]
        );
    }

    #[test]
    fn add_api_keys_from_env_missing_permissions() {
        let mut session = Session::new();
        let err = session
            .add_api_keys_from_vars(env_vars(&[("GEMINI_API_KEY", "gemini-key")]))
            .unwrap_err();
        assert!(matches!(err, AddApiKeysError::MissingPermissions(ref v) if v == "GEMINI_API_KEY"));
        assert!(session.api_keys.is_empty());
    }

    #[test]
    fn add_api_keys_from_env_permissions_without_key() {
        let mut session = Session::new();
        let err = session
            .add_api_keys_from_vars(env_vars(&[("GEMINI_API_KEY_PERMISSIONS", "public")]))
            .unwrap_err();
        assert!(matches!(err, AddApiKeysError::MissingKey(ref v) if v == "GEMINI_API_KEY"));
        assert!(session.api_keys.is_empty());
    }

    #[test]
    fn add_api_keys_from_env_unknown_permission() {
        let mut session = Session::new();
        let err = session
            .add_api_keys_from_vars(env_vars(&[
                ("GEMINI_API_KEY", "gemini-key"),
                ("GEMINI_API_KEY_PERMISSIONS", "public,bogus"),
            ]))
            .unwrap_err();
        assert!(matches!(
            err,
            AddApiKeysError::UnknownPermission { ref permission, .. } if permission == "bogus"
        ));
        assert!(session.api_keys.is_empty());
    }

    #[test]
    fn add_api_keys_from_env_empty_provider() {
        let mut session = Session::new();
        let err = session
            .add_api_keys_from_vars(env_vars(&[
                ("_API_KEY", "key"),
                ("_API_KEY_PERMISSIONS", "public"),
            ]))
            .unwrap_err();
        assert!(matches!(err, AddApiKeysError::InvalidName(_)));
    }

    #[tokio::test]
    async fn document_requires_working_folder() {
        let mut session = Session::new();
        assert!(matches!(
            session.document("doc.md").await,
            Err(AccessDocumentError::NoWorkingFolder)
        ));
    }

    #[tokio::test]
    async fn document_resolves_path_relative_to_working_folder() {
        let dir = TempTree::new(fs_tree! {
            "child" => {},
        })
        .await
        .unwrap();
        let root = Folder::open(&dir).await.unwrap();
        let child = root.folder("child").await.unwrap();
        child
            .create_document("doc.md", NewSource::Bytes(b"nested"), Some(&[]))
            .await
            .unwrap();

        let mut session = Session::with_folder(root);
        let doc = session.document("child/doc.md").await.unwrap();
        assert_eq!(doc.path(), "child/doc.md");

        let mut session = Session::with_folder(child);
        let doc = session.document("doc.md").await.unwrap();
        assert_eq!(doc.path(), "child/doc.md");
    }

    #[tokio::test]
    async fn document_rejects_path_outside_collection() {
        let dir = TempTree::new(fs_tree! {
            "doc.md" => "foo",
        })
        .await
        .unwrap();
        let other = TempTree::new(fs_tree! {
            "other.md" => "bar",
        })
        .await
        .unwrap();
        let mut session = Session::with_folder(Folder::open(&dir).await.unwrap());

        assert!(matches!(
            session.document(other.join("other.md")).await,
            Err(AccessDocumentError::Library(
                AccessLibraryError::NotInCollection(_)
            ))
        ));
    }

    #[tokio::test]
    async fn document_requires_session_permissions() {
        // Documents without metadata default to `ON_DEVICE`
        let dir = TempTree::new(fs_tree! {
            "doc.md" => "foo",
        })
        .await
        .unwrap();
        let mut session = Session::with_folder(Folder::open(&dir).await.unwrap());

        // Sessions grant all permissions by default, but require none
        assert!(matches!(
            session.document("doc.md").await,
            Err(AccessDocumentError::Permission(
                AddDocumentError::ElevationRequired(_)
            ))
        ));
        assert!(session.documents.is_empty());

        session.permissions_granted = vec![];
        session.permissions_required = vec![ON_DEVICE];
        assert!(matches!(
            session.document("doc.md").await,
            Err(AccessDocumentError::Permission(
                AddDocumentError::NotAuthorized(_)
            ))
        ));
        assert!(session.documents.is_empty());

        session.permissions_granted = vec![ON_DEVICE];
        let doc = session.document("doc.md").await.unwrap();
        assert_eq!(doc.path(), "doc.md");
        assert_eq!(session.documents.len(), 1);
    }

    #[tokio::test]
    async fn document_is_cached_by_canonical_path() {
        // Documents without metadata default to `ON_DEVICE`
        let dir = TempTree::new(fs_tree! {
            "child" => {
                "doc.md" => "nested",
            },
        })
        .await
        .unwrap();
        let mut session = Session::with_folder(Folder::open(&dir).await.unwrap());
        session.permissions_required = vec![ON_DEVICE];

        session.document("child/doc.md").await.unwrap();
        session.document("child/../child/./doc.md").await.unwrap();
        assert_eq!(session.documents.len(), 1);

        // Cached documents are still subject to the session's current permissions
        session.permissions_required = vec![];
        assert!(matches!(
            session.document("child/doc.md").await,
            Err(AccessDocumentError::Permission(
                AddDocumentError::ElevationRequired(_)
            ))
        ));
        assert_eq!(session.documents.len(), 1);
    }

    #[test]
    fn restrict_access_raises_requirements() {
        let mut session = Session::new();
        session.restrict_access(vec![ON_DEVICE]).unwrap();
        // Lower permissions don't reduce the requirement
        session.restrict_access(vec![PUBLIC]).unwrap();
        assert_eq!(session.permissions_required, vec![ON_DEVICE]);
    }

    #[test]
    fn restrict_access_denied_beyond_granted_permissions() {
        let mut session = Session::new();
        session.permissions_granted = vec![PUBLIC];
        assert!(matches!(
            session.restrict_access(vec![ON_DEVICE]),
            Err(RestrictAccessError::PermissionDenied(_))
        ));
        assert!(session.permissions_required.is_empty());
    }

    #[test]
    fn restrict_access_removes_inactive_api_keys_lacking_permissions() {
        let mut session = Session::new();
        session.add_api_key(
            ApiKey::new("key".into(), "Kept".into(), String::new()),
            vec![ON_DEVICE],
        );
        session.add_api_key(
            ApiKey::new("key".into(), "Removed".into(), String::new()),
            vec![PUBLIC],
        );

        session.restrict_access(vec![ON_DEVICE]).unwrap();
        assert_eq!(session.api_keys.len(), 1);
        assert_eq!(session.api_keys[0].item.provider(), "Kept");
    }

    #[test]
    fn restrict_access_rejects_active_api_key_without_changes() {
        let mut session = Session::new();
        session.add_api_key(
            ApiKey::new("key".into(), "Active".into(), String::new()),
            vec![PUBLIC],
        );
        session.add_api_key(
            ApiKey::new("key".into(), "Inactive".into(), String::new()),
            vec![PUBLIC],
        );
        let guard = session.api_keys[0].track_one();

        match session.restrict_access(vec![ON_DEVICE]).unwrap_err() {
            RestrictAccessError::ApiKeyNotAuthorized(names) => {
                assert_eq!(names, vec!["Active"]);
            }
            other => panic!("unexpected error: {other:?}"),
        }
        assert_eq!(session.api_keys.len(), 2);
        assert!(session.permissions_required.is_empty());

        // Once unused, both keys lack the permissions and are removed
        drop(guard);
        session.restrict_access(vec![ON_DEVICE]).unwrap();
        assert!(session.api_keys.is_empty());
    }

    #[tokio::test]
    async fn restrict_access_removes_inactive_extensions_lacking_permissions() {
        struct TestExtension {
            uri: &'static str,
            name: &'static str,
        }

        impl Extension for TestExtension {
            fn uri(&self) -> &str {
                self.uri
            }
            fn name(&self) -> &str {
                self.name
            }
            fn description(&self) -> &str {
                ""
            }
        }

        let mut session = Session::new();
        add_extension(
            &mut session,
            TestExtension {
                uri: "markhor://kept",
                name: "Kept",
            },
            vec![ON_DEVICE],
        )
        .await
        .unwrap();
        add_extension(
            &mut session,
            TestExtension {
                uri: "markhor://active",
                name: "Active",
            },
            vec![PUBLIC],
        )
        .await
        .unwrap();
        let guard = session.extensions[1].track_one();

        match session.restrict_access(vec![ON_DEVICE]).unwrap_err() {
            RestrictAccessError::ExtensionNotAuthorized(names) => {
                assert_eq!(names, vec!["Active"]);
            }
            other => panic!("unexpected error: {other:?}"),
        }
        assert_eq!(session.extensions.len(), 2);

        drop(guard);
        session.restrict_access(vec![ON_DEVICE]).unwrap();
        assert_eq!(session.extensions.len(), 1);
        assert_eq!(session.extensions[0].item.name(), "Kept");
    }
}
