use thiserror::Error;

use crate::{dependencies::{Provide, api_key::ApiKey}, extension::{Extension, InitExtensionError}, library::{Document, Scope, Workspace}, permissions::{Authorized, Permission, Restricted}};

use std::{mem, sync::Arc};

pub struct Session {
    workspace: Option<Workspace>,
    documents: Vec<Document>,
    pub(crate) api_keys: Vec<ApiKey>,
    provides_api_keys: bool,
    extensions: Vec<ExtensionSlot>,
    permissions: Vec<Permission>,
}

impl Session {
    pub fn new() -> Self {
        Self {
            workspace: None,
            documents: vec![],
            api_keys: vec![],
            provides_api_keys: false,
            extensions: vec![], 
            permissions: vec![] 
        }
    }

    // Crate-public because it should be called with accurate permissions
    // This method is more of a temporary solution anyway
    pub(crate) async fn add_extension<E: Extension + 'static>(
        &mut self,
        mut extension: E,
        permissions: Vec<Permission>,
    ) -> Result<(), InitExtensionError> {
        extension.initialize().await?;
        self.extensions.push(ExtensionSlot { 
            ext: Arc::new(extension),
            perm: permissions,
        });
        Ok(())
    }

    pub async fn resolve_extension<E: Extension + Provide + 'static>(&mut self) -> Result<(), InitExtensionError> {
        // The following (incorrect) implementation pursues the goal of allowing multiple
        // instances of a single extension type. Extensions providing multiple instances of 
        // themselves would most typically do this via the macro attribute `#[provide(each)]`.

        // This does not work (tests failing correctly, etc.); just committing this to illustrate
        // the challenge.
        // Here we're providing (potentially) multiple instances of the extension (e.g.,
        // a chat client with one or more different API keys). But we can only pass a single
        // `Session` ref (in this case `self`).
        let mut iter = E::iter(self)?;
        let mut ext_instances = Vec::new();
        loop {
            let (Some(ext), perm) = self.track_permissions(async |_| {
                // We could, in principle iterate inside of a loop where we create an isolated
                // `Session` instance for each iteration (as indeed we're doing here). But of
                // course, the iterator is still working off of the original reference (`self`).
                // So we can't track permissions for each individual extension instance, which
                // is precisely what we'd need to do in the aforementioned example. (The point
                // of having different API keys for the same model is to select them based on
                // their different permissions and price points, but that information will not now
                // be connected to the specific extension instance).
                // If we really wanted to, we could find a way to swap out `Session` references on
                // each iteration via internal mutability, but there should be an easier way to do
                // this. (And even if we did that, it's still not bulletproof, see below.)
                let Some(e) = iter.next() else {
                    return None;
                };
                Some(e)
            }).await else {
                break;
            };
            ext_instances.push((ext, perm));
        }
        mem::drop(iter);
        for (ext, perm) in ext_instances {
            // TODO: consider calling `Extension::initialize` inside of `track_permissions`
            // so we could better accommodate dependency resolution during `initialize` (in case 
            // we update `Extension` trait to include `&Session` in method signature)
            self.add_extension(ext, perm).await?;
        }
        Ok(())

        // However this is solved, it's worth noting that we still need to assume that the 
        // `Provide::iter` implementation is well-behaved and lazy. That's true if it's 
        // macro-generated or if follows a similar strategy as macro-generated ones. But if an 
        // implementation eagerly iterates all (e.g.) API keys and then shuffles or clones them,
        // passing them to arbitrary instences of itself, there's nothing we can do to track that.

        // Theoretically, we could go so far as to redesign the `Provide` trait around this 
        // challenge adding a method to deal with `#[provide(each)]` fields explicitly and
        // adding a bunch of code elsewhere to implement iteration over Cartesian products from 
        // the outside. But at the end of the day, if an extension really wanted to subvert our
        // permissions system by sharing assets like API keys with instances that are not tagged
        // with the corresponding permissions, that's always possible (e.g., via global state,
        // file system, config, etc.). The real concern is that if it's too difficult to write 
        // extensions that work correctly with the built-in permissions system, bugs and 
        // annoyances will multiply mightily.
        
        // So the point here is not to police extensions to the extreme, but to avoid unnecessary
        // gotchas. In other words, however we solve this, the design goal is to make it easy 
        // and natural to write extensions that just work.

        // As a side note, it's worth mentioning that the "extensions" referred to here are those
        // included at compile time (which would be vetted, if not written, by application 
        // authors). Sooner or later, there will also be plugins which can be added by the user 
        // and loaded dynamically. Those will require stricter security measures (e.g., 
        // sandboxing), but that's a story for another day.
    }

    async fn track_permissions<T, F: AsyncFnOnce(&Session) -> T>(&self, f: F) -> (T, Vec<Permission>) {
        // Provide an isolated Session instance without library access
        
        // The point of this separation is that if an extension's dependencies cannot be resolved,
        // it would ideally not get any chance to exert side effects on session resources. If
        // side effects might be caused by extensions that are not even part of the session,
        // unexpected changes might be harder to debug.
        //
        // The problem with this separation is that it's artificial. Even if incompletely resolved
        // extensions cannot affect documents, they could still affect extensions. Such side 
        // effects might be just as difficult to debug. (It's also worth mentioning that while
        // extensions currently only have the role of actors, not resources, it's unlikely to stay
        // that way. So they're not all that different from documents.)
        //
        // A possible compromise would be to introduce extension manifests, and to grant access
        // only (?) to actors and resources mentioned in the manifest until the extension is fully
        // initialized. Another option, of course, is simply to add documentation discouraging 
        // side effects during initialization (though I think we can do better than that).
        //
        // To clarify, the concern here is not to guard against malicious extensions. If we're
        // using those, we have bigger problems anyway. The concern is just to constrain how
        // extensions should be designed, particularly how they should behave during dependency
        // resolution and initialization (before they become part of a session's extension list).
        
        let api_keys = self.api_keys.iter().map(ApiKey::to_inactive).collect();
        let temp_session = Session {
            workspace: None,
            documents: vec![],
            api_keys,
            provides_api_keys: true,
            extensions: self.extensions.clone(),
            permissions: vec![],
        };

        // Note current extension usage counts
        
        // NOTE: This is not a clean solution. Envision the following scenario.
        //
        // 1. Extension A is added to the session
        // 2. Task X uses extension A
        // 3. Extension B is added. It has zero dependencies.
        // 4. At the same time, task X clones some component of extension A
        // 5. `track_permissions` observes increased usage of extension A, imputing this to extension B
        // 6. Extension B is unnecessarily burdened with permissions required by extension A
        //
        // Conversely, if task X dropped some previously cloned component of extension A during 
        // step 4, `track_permissions` might fail to recognize that extension B uses extension A
        // and that it therefore requires the same permissions.
        //
        // Still, this is good enough for now. The false positives don't cause any trouble unless 
        // `elevate_permissions` is called, and even then only in certain edge cases. The false 
        // negatives are more concerning but again should not make a difference in practice unless 
        // `elevate_permissions` is called (and it's not totally clear yet whether that method will
        // actually be needed).
        //
        // A more robust solution would require tracking usage more explicitly than via
        // `Arc::strong_count`, which is very doable but not an immediate priority.
        //
        // TODO: should probably be fixed sooner rather than later, before beta at the latest
        
        let usage_before = temp_session.extensions.iter().map(|slot| Arc::strong_count(&slot.ext)).collect::<Vec<_>>();

        // Execute the function with the temporary session
        let result = f(&temp_session).await;

        // Collect permissions from any API keys and extensions that were accessed
        let mut permissions = vec![];

        // Collect permissions from any API keys that were accessed
        for (idx, api_key) in temp_session.api_keys.iter().enumerate() {
            if api_key.is_active() {
                for perm in api_key.permissions_granted() {
                    Permission::insert(&mut permissions, perm.clone());
                }
                // Mark original as active
                self.api_keys[idx].key();
            }
        }

        // Compare extension usage counts after execution and collect permissions from any extensions that were used
        for idx in 0..temp_session.extensions.len() {
            let usage_after = Arc::strong_count(&temp_session.extensions[idx].ext);
            if usage_after > usage_before[idx] {
                // Extension was used, collect its permissions
                for perm in temp_session.extensions[idx].permissions_granted() {
                    Permission::insert(&mut permissions, perm.clone());
                }
            }
        }

        (result, permissions)
    }

    pub fn extensions(&self) -> impl Iterator<Item = &Arc<dyn Extension>> {
        self.extensions.iter().map(|slot| &slot.ext)
    }

    pub fn permissions(&self) -> &[Permission] {
        &self.permissions
    }

    pub fn elevate_permissions(&mut self, permissions: impl IntoIterator<Item = Permission>) -> Result<Vec<String>, RestrictAccessError> {
        let added_permissions = Perms(permissions.into_iter().collect::<Vec<_>>());

        // Check if active API keys have necessary permissions
        let mut api_key_error_indices = vec![];
        for api_key in &self.api_keys {
            if !api_key.may_access(&added_permissions) {
                api_key_error_indices.push(format!("{} ({})", api_key.provider(), api_key.project()));
            }
        }
        if !api_key_error_indices.is_empty() {
            return Err(RestrictAccessError::ApiKeyNotAuthorized(api_key_error_indices));
        }

        // Check if extensions have necessary permissions or if those that lack permissions can be removed
        let mut ext_remove_indices = vec![];
        let mut ext_error_indices = vec![];
        for idx in 0..self.extensions.len() {
            let ext_slot = &self.extensions[idx];
            if !ext_slot.may_access(&added_permissions) {
                // Check if the extension is actually being used
                if Arc::strong_count(&ext_slot.ext) > 1 {
                    // Extension lacks permission and cannot be removed
                    // Include in error list
                    ext_error_indices.push(idx);
                } else {
                    // Extension is not in use and can be removed
                    ext_remove_indices.push(idx);
                }
            }
        }
        if !ext_error_indices.is_empty() {
            let error_names = ext_error_indices.into_iter()
                .map(|i| self.extensions[i].ext.name().to_string()).collect();
            return Err(RestrictAccessError::ExtensionNotAuthorized(error_names));
        }

        // Remove any unauthorized API keys that are not in use
        let mut removed_api_keys = vec![];
        let mut idx = 0;
        while idx < self.api_keys.len() {
            if !self.api_keys[idx].may_access(&added_permissions) {
                let api_key = self.api_keys.remove(idx);
                removed_api_keys.push(format!("{} ({})", api_key.provider(), api_key.project()));
            } else {
                idx += 1
            }
        }

        // Remove any unauthorized extensions that are not in use
        let mut removed_extensions = vec![];
        for idx in ext_remove_indices.into_iter().rev() {
            let ext = self.extensions.remove(idx);
            removed_extensions.push(ext.ext.name().to_string());
        }

        // Update session permissions
        for permission in added_permissions.0.into_iter() {
            Permission::insert(&mut self.permissions, permission);
        }

        Ok(removed_extensions)
    }
}

impl Restricted for Session {
    fn permissions_required(&self) -> &[Permission] {
        self.permissions()
    }
}

impl From<Workspace> for Session {
    fn from(workspace: Workspace) -> Self {
        Self {
            workspace: Some(workspace),
            documents: vec![],
            api_keys: vec![],
            provides_api_keys: false,
            extensions: vec![],
            permissions: vec![],
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

#[derive(Clone)]
struct ExtensionSlot {
    ext: Arc<dyn Extension>,
    perm: Vec<Permission>,
}

impl Authorized for ExtensionSlot {
    fn permissions_granted(&self) -> &[Permission] {
        &self.perm
     }
}

struct Perms(Vec<Permission>);

impl Restricted for Perms {
    fn permissions_required(&self) -> &[Permission] {
        &self.0
     }
}


#[cfg(test)]
mod tests {
    use super::*;
    use crate::chunking::{Chunker, test_chunker::FixedSizeChunkerExtension};
    use crate::dependencies::{ResolveDependencyError};
    use crate::extension::Comp;
    use crate::permissions::{GDPR, NOT_USED_FOR_TRAINING, PUBLIC};

    #[tokio::test]
    async fn track_permissions_for_extensions() {
        let ext0 = FixedSizeChunkerExtension::new(10);
        let ext1 = FixedSizeChunkerExtension::new(20);
        let ext2 = FixedSizeChunkerExtension::new(30);

        let mut session = Session::new();
        session.add_extension(ext0, vec![PUBLIC]).await.unwrap();
        session.add_extension(ext1, vec![NOT_USED_FOR_TRAINING]).await.unwrap();
        session.add_extension(ext2, vec![GDPR]).await.unwrap();
        assert_eq!(session.extensions.len(), 3);

        let (_, perm) = session.track_permissions(async |sess| {
            // Use ext0
            Comp::<dyn Chunker>::first(sess).unwrap()
        }).await;
        assert_eq!(perm, vec![PUBLIC]);

        let (_, perm) = session.track_permissions(async |sess| {
            // Use all three extensions
            Vec::<Comp<dyn Chunker>>::first(sess).unwrap()
        }).await;
        assert_eq!(perm, vec![NOT_USED_FOR_TRAINING, GDPR]);

        let (_, perm) = session.track_permissions(async |sess| {
            // Use ext0 and ext1
            let mut chunkers = Vec::<Comp<dyn Chunker>>::first(sess).unwrap();
            chunkers.pop();
            chunkers
        }).await;
        assert_eq!(perm, vec![NOT_USED_FOR_TRAINING]);

        let (_, perm) = session.track_permissions(async |sess| {
            // Use ext0 and ext2
            let mut chunkers = Vec::<Comp<dyn Chunker>>::first(sess).unwrap();
            chunkers.remove(1);
            chunkers
        }).await;
        assert_eq!(perm, vec![GDPR]);
    }

    #[tokio::test]
    async fn track_permissions_for_api_keys() {
        let api_key1 = ApiKey::new(
            "1234567890abcdef".to_string(),
            "TestProvider".to_string(),
            "TestProject".to_string(),
            vec![NOT_USED_FOR_TRAINING],
        );
        let api_key2 = ApiKey::new(
            "abcdef1234567890".to_string(),
            "AnotherProvider".to_string(),
            "AnotherProject".to_string(),
            vec![GDPR],
        );

        let mut session = Session::new();
        session.api_keys.push(api_key1);
        session.api_keys.push(api_key2);

        let (_, perm) = session.track_permissions(async |sess| {
            // Use no API keys
            let _api_keys = Vec::<ApiKey>::first(sess).unwrap();
        }).await;
        assert_eq!(perm, vec![]);

        let (_, perm) = session.track_permissions(async |sess| {
            // Use only the first API key
            let api_keys = Vec::<ApiKey>::first(sess).unwrap();
            api_keys[0].key();
        }).await;
        assert_eq!(perm, vec![NOT_USED_FOR_TRAINING]);

        let (_, perm) = session.track_permissions(async |sess| {
            // Use only the second API key
            let api_keys = Vec::<ApiKey>::first(sess).unwrap();
            api_keys[1].key();
        }).await;
        assert_eq!(perm, vec![GDPR]);

        let (_, perm) = session.track_permissions(async |sess| {
            // Use both API keys
            let api_keys = Vec::<ApiKey>::first(sess).unwrap();
            api_keys[0].key();
            api_keys[1].key();
        }).await;
        assert_eq!(perm, vec![NOT_USED_FOR_TRAINING, GDPR]);
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
        session.add_extension(ext0, vec![PUBLIC]).await.unwrap();
        session.add_extension(ext1, vec![NOT_USED_FOR_TRAINING]).await.unwrap();
        assert_eq!(session.extensions.len(), 2);

        // Resolve TestExtension0, which should only require permission PUBLIC
        session.resolve_extension::<TestExtension0>().await.unwrap();
        assert_eq!(session.extensions.len(), 3);
        assert_eq!(session.extensions[2].ext.name(), "test-extension-0");
        assert_eq!(session.extensions[2].perm, vec![PUBLIC]);

        // Resolve TestExtension1, which should require permission NOT_USED_FOR_TRAINING
        session.resolve_extension::<TestExtension1>().await.unwrap();
        assert_eq!(session.extensions.len(), 4);
        assert_eq!(session.extensions[3].ext.name(), "test-extension-1");
        assert_eq!(session.extensions[3].perm, vec![NOT_USED_FOR_TRAINING]);
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
                // Get API key during resolution, convert to string, then drop
                // (not a recommended design pattern, but possible)
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
            // uses single API key, but not the first one
            #[provide(filter = |k| k.provider_is("AnotherProvider"))]
            _api_key: ApiKey,
        }

        impl Extension for TestExtension3 {
            fn uri(&self) -> &str           { "markhor://test-extension-3" }
            fn name(&self) -> &str          { "test-extension-3" }
            fn description(&self) ->  &str  { "Test extension 3" }
        }

        let mut session = Session::new();
        session.api_keys.extend(vec![
            ApiKey::new (
                "1234567890abcdef".to_string(),
                "TestProvider".to_string(),
                "TestProject".to_string(),
                vec![GDPR],
            ),
            ApiKey::new (
                "abcdef1234567890".to_string(),
                "AnotherProvider".to_string(),
                "AnotherProject".to_string(),
                vec![PUBLIC],
            ),
            ApiKey::new (
                "0987654321fedcba".to_string(),
                "YetAnotherProvider".to_string(),
                "YetAnotherProject".to_string(),
                vec![NOT_USED_FOR_TRAINING],
            ),
        ]);

        fn count_active_api_keys(session: &Session) -> usize {
            session.api_keys.iter().filter(|k| k.is_active()).count()
        }
        assert_eq!(count_active_api_keys(&session), 0);
        
        // Resolve TestExtension0, which should require only permission GDPR
        session.resolve_extension::<TestExtension0>().await.unwrap();
        assert_eq!(session.extensions.len(), 1);
        assert_eq!(session.extensions[0].ext.name(), "test-extension-0");
        assert_eq!(session.extensions[0].perm, vec![GDPR]);
        // Only 1 active API key
        assert_eq!(count_active_api_keys(&session), 1);

        // Resolve TestExtension1, which should require permissions of all 3 API keys
        session.resolve_extension::<TestExtension1>().await.unwrap();
        assert_eq!(session.extensions.len(), 2);
        assert_eq!(session.extensions[1].ext.name(), "test-extension-1");
        assert_eq!(session.extensions[1].perm, vec![GDPR, NOT_USED_FOR_TRAINING]);
        // All 3 API keys are now active
        assert_eq!(count_active_api_keys(&session), 3);

        // Resolve TestExtension2, which should require permission GDPR
        // even though the extension doesn't store the key
        session.resolve_extension::<TestExtension2>().await.unwrap();
        assert_eq!(session.extensions.len(), 3);
        assert_eq!(session.extensions[2].ext.name(), "test-extension-2");
        assert_eq!(session.extensions[2].perm, vec![GDPR]);

        // Resolve TestExtension3, which should require only permission PUBLIC
        // as per API key "AnotherProvider"
        session.resolve_extension::<TestExtension3>().await.unwrap();
        assert_eq!(session.extensions.len(), 4);
        assert_eq!(session.extensions[3].ext.name(), "test-extension-3");
        assert_eq!(session.extensions[3].perm, vec![PUBLIC]);
    }
}