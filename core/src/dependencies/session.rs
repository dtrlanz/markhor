use thiserror::Error;

use crate::{dependencies::Provide, extension::{Extension, InitExtensionError}, library::{Document, Scope, Workspace}, permissions::{Authorized, Permission, Restricted}};

use std::{mem, sync::Arc};

pub struct Session {
    workspace: Option<Workspace>,
    documents: Vec<Document>,
    extensions: Vec<ExtensionSlot>,
    permissions: Vec<Permission>,
}

impl Session {
    pub fn new() -> Self {
        Self {
            workspace: None,
            documents: vec![],
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
        let (extension, permissions) = self.track_permissions(async |session| {
            let e = E::first(session)?;
            Ok::<_, InitExtensionError>(e)
        }).await;
        self.add_extension(extension?, permissions).await
    }

    async fn track_permissions<T, F: AsyncFnOnce(&Session) -> T>(&mut self, f: F) -> (T, Vec<Permission>) {
        // NOTE
        // This is not a clean solution. Envision the following scenario:
        // 1. Extension A is added to the session
        // 2. Task X uses extension A
        // 3. Extension B is added. It has zero dependencies.
        // 4. At the same time, task X clones some component of extension A
        // 5. `track_permissions` observes increased usage of extension A, imputing this to extension B
        // 6. Extension B is unnecessarily burdened with permissions required by extension A
        //
        // Still, this is good enough for now. The false positives don't cause any trouble unless 
        // `elevate_permissions` is called, and even then only in certain edge cases.
        //
        // A more robust solution would require tracking usage more explicitly than via
        // `Arc::strong_count`, which is very doable but not an immediate priority. (TODO)

        // Provide an isolated Session instance without library access
        let temp_session = Session {
            workspace: None,
            documents: vec![],
            extensions: mem::take(&mut self.extensions),
            permissions: vec![],
        };

        // Note current asset usage counts
        let usage_before = temp_session.extensions.iter().map(|slot| Arc::strong_count(&slot.ext)).collect::<Vec<_>>();

        // Execute the function with the temporary session
        let result = f(&temp_session).await;

        // Compare asset usage counts after execution and collect permissions from any extensions that were used
        let Session { extensions, .. } = temp_session;
        let mut permissions = vec![];
        for idx in 0..extensions.len() {
            let usage_after = Arc::strong_count(&extensions[idx].ext);
            if usage_after > usage_before[idx] {
                // Extension was used, collect its permissions
                for perm in extensions[idx].permissions_granted() {
                    Permission::insert(&mut permissions, perm.clone());
                }
            }
        }

        // Restore extensions to the main session
        self.extensions = extensions;
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

        // Check if extensions have necessary permissions or if those that lack permissions can be removed
        let mut remove_indices = vec![];
        let mut error_indices = vec![];
        for idx in 0..self.extensions.len() {
            let ext_slot = &self.extensions[idx];
            if !ext_slot.may_access(&added_permissions) {
                // Check if the extension is actually being used
                if Arc::strong_count(&ext_slot.ext) > 1 {
                    // Extension lacks permission and cannot be removed
                    // Include in error list
                    error_indices.push(idx);
                } else {
                    // Extension is not in use and can be removed
                    remove_indices.push(idx);
                }
            }
        }
        if !error_indices.is_empty() {
            let error_names = error_indices.into_iter()
                .map(|i| self.extensions[i].ext.name().to_string()).collect();
            return Err(RestrictAccessError::ExtensionNotAuthorized(error_names));
        }

        // Remove any unauthorized extensions that are not in use
        let mut removed_extensions = vec![];
        for idx in remove_indices.into_iter().rev() {
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
            extensions: vec![],
            permissions: vec![],
        }
    }
}

#[derive(Debug, Error)]
pub enum RestrictAccessError {
    #[error("Extensions lack required permissions: {}", .0.join(", "))]
    ExtensionNotAuthorized(Vec<String>),
}

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
    use crate::extension::Comp;
    use crate::permissions::{GDPR, NOT_USED_FOR_TRAINING, PUBLIC};

    #[tokio::test]
    async fn track_permissions() {
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
    async fn resolve_extension() {
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
}