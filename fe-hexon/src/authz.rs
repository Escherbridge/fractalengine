//! Policy-gated registry operations — closes the Phase 8.4 RBAC gap (see src/AGENTS.md §authz).

use std::sync::{Arc, OnceLock};

use fe_policy::{
    Action, AuthContext, Decision, OwnershipPolicy, Policy, PolicyEngine, PublicReadPolicy,
    RoleLevelPolicy, Scope,
};

use crate::manifest::{HexonManifest, HexonType, InstalledHexon};
use crate::package::HexonPackageData;
pub use crate::registry::PUBLISHED_BINDING;
use crate::registry::{HexonRegistry, RegistryError};

/// Registry policy: public discovery reads; Editor+ install/uninstall; Manager+ publish.
pub fn registry_engine() -> &'static PolicyEngine {
    static ENGINE: OnceLock<PolicyEngine> = OnceLock::new();
    ENGINE.get_or_init(|| {
        PolicyEngine::new()
            .with_policy(Arc::new(PublicReadPolicy))
            .with_policy(Arc::new(RoleLevelPolicy::standard()))
    })
}

/// Deny-by-default gate used by every registry entry point below.
pub fn authorize(subject: &AuthContext, action: &Action, scope: &str) -> Result<(), RegistryError> {
    match registry_engine().evaluate(subject, action, &Scope::new(scope)) {
        Decision::Allow => Ok(()),
        Decision::Deny(reason) => Err(RegistryError::PermissionDenied(reason)),
    }
}

/// Owner-only gate: `subject` must be `owner_did` (see src/AGENTS.md §authz).
fn require_owner(
    owner_did: &str,
    subject: &AuthContext,
    action: &Action,
    resource: &str,
) -> Result<(), RegistryError> {
    let scope = Scope::new(resource);
    let decision = OwnershipPolicy::new(owner_did).evaluate(subject, action, &scope);
    fe_policy::engine::log_decision("ownership", subject, action, &scope, &decision);
    match decision {
        Decision::Allow => Ok(()),
        Decision::Deny(reason) => Err(RegistryError::PermissionDenied(reason)),
    }
}

/// Hexon-id collision rule: a hexon_id published by one DID is closed to every other signer.
fn require_hexon_id_publisher(
    registry: &HexonRegistry,
    manifest: &HexonManifest,
    action: &Action,
) -> Result<(), RegistryError> {
    let Some(existing) = registry.published_by_another(&manifest.hexon_id, &manifest.publisher_did)
    else {
        return Ok(());
    };
    let signer = AuthContext::Did {
        did: manifest.publisher_did.clone(),
        role: fe_policy::RoleLevel::None,
    };
    require_owner(
        &existing.manifest.publisher_did,
        &signer,
        action,
        &format!("hexon:{}", manifest.hexon_id),
    )
}

/// Petal installs never write the publish-scope binding; only `publish_as` does.
fn refuse_published_binding(petal_id: &str) -> Result<(), RegistryError> {
    if petal_id == PUBLISHED_BINDING {
        return Err(RegistryError::PermissionDenied(format!(
            "the {PUBLISHED_BINDING} binding is written only by publish"
        )));
    }
    Ok(())
}

impl HexonRegistry {
    /// Policy-gated publish: Manager+ (RoleLevelPolicy), then only the signed
    /// publisher DID that first published a `hexon_id` may republish it.
    pub fn publish_as(
        &mut self,
        subject: &AuthContext,
        package: HexonPackageData,
    ) -> Result<InstalledHexon, RegistryError> {
        authorize(subject, &Action::Publish, PUBLISHED_BINDING)?;
        require_hexon_id_publisher(self, &package.manifest, &Action::Publish)?;
        self.install(package, PUBLISHED_BINDING)
    }

    /// Policy-gated install: Editor+ at the petal scope; a hexon_id published by another signer is refused.
    pub fn install_as(
        &mut self,
        subject: &AuthContext,
        package: HexonPackageData,
        petal_id: &str,
    ) -> Result<InstalledHexon, RegistryError> {
        authorize(subject, &Action::Install, petal_id)?;
        refuse_published_binding(petal_id)?;
        require_hexon_id_publisher(self, &package.manifest, &Action::Install)?;
        self.install(package, petal_id)
    }

    /// Policy-gated install of a published build into a petal: Editor+ at the petal scope, like `install_as`.
    pub fn install_published_as(
        &mut self,
        subject: &AuthContext,
        hexon_uri: &str,
        petal_id: &str,
    ) -> Result<InstalledHexon, RegistryError> {
        authorize(subject, &Action::Install, petal_id)?;
        refuse_published_binding(petal_id)?;
        self.install_published(hexon_uri, petal_id)
    }

    /// Policy-gated uninstall: requires Editor+ at the petal scope.
    pub fn uninstall_as(
        &mut self,
        subject: &AuthContext,
        hexon_uri: &str,
        petal_id: &str,
    ) -> Result<(), RegistryError> {
        authorize(subject, &Action::Write, petal_id)?;
        let Some(installed) = self.get_in_petal(hexon_uri, petal_id) else {
            return Err(RegistryError::NotFound(hexon_uri.to_string()));
        };
        if petal_id == PUBLISHED_BINDING {
            require_owner(
                &installed.manifest.publisher_did,
                subject,
                &Action::Write,
                hexon_uri,
            )?;
        }
        self.uninstall(hexon_uri, petal_id)
    }

    /// Policy-gated listing: public read (anonymous allowed by PublicReadPolicy).
    pub fn list_installed_as(
        &self,
        subject: &AuthContext,
        petal_id: Option<&str>,
    ) -> Result<Vec<InstalledHexon>, RegistryError> {
        authorize(subject, &Action::Read, petal_id.unwrap_or("*"))?;
        Ok(self.list_installed(petal_id))
    }

    /// Policy-gated search: public read (anonymous allowed by PublicReadPolicy).
    pub fn search_local_as(
        &self,
        subject: &AuthContext,
        query: &str,
        tags: &[&str],
        kind: Option<HexonType>,
    ) -> Result<Vec<HexonManifest>, RegistryError> {
        self.search_local_in_petal_as(subject, None, query, tags, kind)
    }

    /// Policy-gated search constrained to an optional petal binding.
    pub fn search_local_in_petal_as(
        &self,
        subject: &AuthContext,
        petal_id: Option<&str>,
        query: &str,
        tags: &[&str],
        kind: Option<HexonType>,
    ) -> Result<Vec<HexonManifest>, RegistryError> {
        authorize(subject, &Action::Read, petal_id.unwrap_or("*"))?;
        Ok(self.search_local_in_petal(petal_id, query, tags, kind))
    }

    /// Policy-gated lookup of the `(hexon_uri, petal_id)` install record.
    pub fn get_installed_as(
        &self,
        subject: &AuthContext,
        hexon_uri: &str,
        petal_id: &str,
    ) -> Result<Option<InstalledHexon>, RegistryError> {
        authorize(subject, &Action::Read, petal_id)?;
        Ok(self.get_in_petal(hexon_uri, petal_id).cloned())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manifest::{AssetEntry, EntryKind, License, LicenseType};
    use crate::package::{hexon_uri, HexonPackage};
    use ed25519_dalek::SigningKey;
    use fe_policy::RoleLevel;
    use rand::rngs::OsRng;
    use std::collections::HashMap;

    fn did(role: RoleLevel) -> AuthContext {
        AuthContext::Did {
            did: "did:key:z6MkTester".to_string(),
            role,
        }
    }

    fn test_package() -> HexonPackageData {
        signed_package(&SigningKey::generate(&mut OsRng), "1.0.0")
    }

    /// A signed package of the fixed `authz-model-v1` hexon at `version`.
    fn signed_package(keypair: &SigningKey, version: &str) -> HexonPackageData {
        let manifest = HexonManifest {
            schema_version: "1.0.0".to_string(),
            hexon_id: "authz-model-v1".to_string(),
            hexon_type: HexonType::Model,
            publisher_did: fe_format::verifying_key_to_did(&keypair.verifying_key()),
            publisher_name: Some("Authz Publisher".to_string()),
            version: version.to_string(),
            build_id: Some("build-authz".to_string()),
            name: "Authz Model".to_string(),
            description: Some("Policy gate test hexon".to_string()),
            tags: vec!["test".to_string()],
            created_at: "2026-01-01T00:00:00Z".to_string(),
            updated_at: "2026-01-01T00:00:00Z".to_string(),
            source_peer_did: None,
            approx_size_bytes: Some(1024),
            min_engine_version: Some("0.1.0".to_string()),
            homepage_url: None,
            dependencies: vec![],
            platforms: vec![],
            address: None,
            integrity: None,
            signature: None,
        };
        let entries = vec![AssetEntry {
            entry_id: "model_001".to_string(),
            kind: EntryKind::Model,
            asset_hash: crate::signature::asset_hash(b"fake model data"),
            format: "gltf".to_string(),
            label: "Authz Model".to_string(),
            tags: vec![],
            description: Some("test".to_string()),
            is_placeable: true,
            is_private: false,
            auto_scale: false,
            preview_image: None,
            center: None,
            extents: None,
            sub_assets: None,
            address: None,
            metadata: None,
        }];
        let license = License {
            license_type: LicenseType::Free,
            ..License::default()
        };
        let mut assets = HashMap::new();
        assets.insert(
            crate::signature::asset_hash(b"fake model data"),
            b"fake model data".to_vec(),
        );
        let zip_bytes = HexonPackage::build(
            manifest,
            entries,
            Some(license),
            assets,
            None,
            HashMap::new(),
            None,
            None,
            keypair,
        )
        .expect("build should succeed");
        HexonPackage::open(&zip_bytes).expect("open should succeed")
    }

    fn temp_registry(tag: &str) -> (HexonRegistry, std::path::PathBuf) {
        let dir = std::env::temp_dir().join(format!("hexon_authz_{tag}_{}", ulid::Ulid::new()));
        (HexonRegistry::new(dir.clone()), dir)
    }

    #[test]
    fn install_allowed_for_editor() {
        let (mut registry, dir) = temp_registry("editor");
        let package = test_package();
        let uri = hexon_uri(&package.manifest);
        let installed = registry
            .install_as(&did(RoleLevel::Editor), package, "petal-1")
            .expect("editor install must pass");
        assert_eq!(installed.hexon_uri, uri);

        // A petal install never writes the publish-scope record, whatever the role.
        let into_published = registry.install_as(&did(RoleLevel::Owner), test_package(), PUBLISHED_BINDING);
        assert!(matches!(into_published, Err(RegistryError::PermissionDenied(_))));
        assert!(registry.list_installed(Some(PUBLISHED_BINDING)).is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn install_denied_for_viewer() {
        let (mut registry, dir) = temp_registry("viewer");
        let result = registry.install_as(&did(RoleLevel::Viewer), test_package(), "petal-1");
        assert!(
            matches!(result, Err(RegistryError::PermissionDenied(_))),
            "insufficient role must be denied"
        );
        assert!(
            registry.list_installed(None).is_empty(),
            "nothing may be installed on deny"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn install_denied_for_anonymous() {
        let (mut registry, dir) = temp_registry("anon");
        let result = registry.install_as(&AuthContext::Anonymous, test_package(), "petal-1");
        assert!(
            matches!(result, Err(RegistryError::PermissionDenied(_))),
            "absent auth must be denied"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn uninstall_denied_for_viewer_allowed_for_editor() {
        let (mut registry, dir) = temp_registry("uninstall");
        let package = test_package();
        let uri = hexon_uri(&package.manifest);
        registry
            .install_as(&did(RoleLevel::Editor), package, "petal-1")
            .expect("install");

        let denied = registry.uninstall_as(&did(RoleLevel::Viewer), &uri, "petal-1");
        assert!(matches!(denied, Err(RegistryError::PermissionDenied(_))));
        assert_eq!(
            registry.list_installed(None).len(),
            1,
            "deny must not uninstall"
        );

        registry
            .uninstall_as(&did(RoleLevel::Editor), &uri, "petal-1")
            .expect("editor uninstall must pass");
        assert!(registry.list_installed(None).is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn scoped_lookups_and_uninstall_do_not_cross_petal_bindings() {
        let (mut registry, dir) = temp_registry("scope");
        let package = test_package();
        let uri = hexon_uri(&package.manifest);
        registry
            .install_as(&did(RoleLevel::Editor), package, "petal-1")
            .expect("install");

        assert!(registry
            .get_installed_as(&did(RoleLevel::Viewer), &uri, "petal-2")
            .expect("read policy")
            .is_none());
        assert!(matches!(
            registry.uninstall_as(&did(RoleLevel::Editor), &uri, "petal-2"),
            Err(RegistryError::NotFound(_))
        ));
        assert_eq!(registry.list_installed(Some("petal-1")).len(), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn scoped_search_only_returns_petal_bound_crates() {
        let (mut registry, dir) = temp_registry("search_scope");
        registry
            .install_as(&did(RoleLevel::Editor), test_package(), "petal-1")
            .expect("install");

        assert_eq!(
            registry
                .search_local_in_petal_as(&did(RoleLevel::Viewer), Some("petal-1"), "", &[], None,)
                .expect("scoped search")
                .len(),
            1
        );
        assert!(registry
            .search_local_in_petal_as(&did(RoleLevel::Viewer), Some("petal-2"), "", &[], None)
            .expect("scoped search")
            .is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn anonymous_read_is_public() {
        let (registry, dir) = temp_registry("read");
        let listed = registry
            .list_installed_as(&AuthContext::Anonymous, None)
            .expect("anonymous list is a public read");
        assert!(listed.is_empty());
        let searched = registry
            .search_local_as(&AuthContext::Anonymous, "anything", &[], None)
            .expect("anonymous search is a public read");
        assert!(searched.is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn publisher(package: &HexonPackageData) -> AuthContext {
        AuthContext::Did {
            did: package.manifest.publisher_did.clone(),
            role: RoleLevel::Owner,
        }
    }

    #[test]
    fn republish_by_a_different_signer_is_denied() {
        let (mut registry, dir) = temp_registry("republish");
        let key_a = SigningKey::generate(&mut OsRng);
        let key_b = SigningKey::generate(&mut OsRng);
        let a_v1 = signed_package(&key_a, "1.0.0");
        let a_did = a_v1.manifest.publisher_did.clone();
        registry
            .publish_as(&did(RoleLevel::Owner), a_v1)
            .expect("first publish by A");

        // Same hexon_id signed by B: denied whether same or new version.
        for version in ["1.0.0", "1.1.0"] {
            let result =
                registry.publish_as(&did(RoleLevel::Owner), signed_package(&key_b, version));
            assert!(
                matches!(result, Err(RegistryError::PermissionDenied(_))),
                "B republishing {version} must be denied, got {result:?}"
            );
        }
        let held = registry.list_installed(None);
        assert_eq!(held.len(), 1, "only A's record remains");
        assert_eq!(held[0].manifest.publisher_did, a_did);

        // B's build of A's hexon_id cannot be installed into a petal either.
        let squat = registry.install_as(&did(RoleLevel::Editor), signed_package(&key_b, "1.0.0"), "petal-1");
        assert!(matches!(squat, Err(RegistryError::PermissionDenied(_))), "got {squat:?}");
        assert!(registry.list_installed(Some("petal-1")).is_empty());

        // Installing A's published build into a petal: Editor+ like install_as, deny-by-default.
        let a_uri = "authz-model-v1@1.0.0".to_string();
        for denied_subject in [did(RoleLevel::Viewer), AuthContext::Anonymous] {
            let denied = registry.install_published_as(&denied_subject, &a_uri, "petal-1");
            assert!(matches!(denied, Err(RegistryError::PermissionDenied(_))), "got {denied:?}");
        }
        assert!(registry.list_installed(Some("petal-1")).is_empty(), "nothing installed on deny");
        let copied = registry
            .install_published_as(&did(RoleLevel::Editor), &a_uri, "petal-1")
            .expect("editor installs the published build");
        assert_eq!(copied.manifest.publisher_did, a_did);
        assert!(registry
            .get_installed_as(&did(RoleLevel::Viewer), &a_uri, "petal-1")
            .expect("read policy")
            .is_some());
        assert!(matches!(
            registry.install_published_as(&did(RoleLevel::Owner), &a_uri, PUBLISHED_BINDING),
            Err(RegistryError::PermissionDenied(_))
        ));

        // A publishing v2 of its own hexon is allowed.
        registry
            .publish_as(&did(RoleLevel::Owner), signed_package(&key_a, "2.0.0"))
            .expect("owner republish allowed");
        assert_eq!(registry.list_installed(Some(PUBLISHED_BINDING)).len(), 2);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn publish_still_requires_manager_role() {
        let (mut registry, dir) = temp_registry("publish_role");
        let result = registry.publish_as(&did(RoleLevel::Editor), test_package());
        assert!(matches!(result, Err(RegistryError::PermissionDenied(_))));
        assert!(registry.list_installed(None).is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn only_the_publisher_may_unpublish_the_global_record() {
        let (mut registry, dir) = temp_registry("unpublish");
        let a_pkg = signed_package(&SigningKey::generate(&mut OsRng), "1.0.0");
        let a = publisher(&a_pkg);
        let uri = hexon_uri(&a_pkg.manifest);
        registry
            .publish_as(&did(RoleLevel::Owner), a_pkg)
            .expect("publish");

        let b = AuthContext::Did {
            did: "did:key:z6MkSomeoneElse".to_string(),
            role: RoleLevel::Owner,
        };
        let denied = registry.uninstall_as(&b, &uri, PUBLISHED_BINDING);
        assert!(matches!(denied, Err(RegistryError::PermissionDenied(_))));
        assert_eq!(
            registry.list_installed(None).len(),
            1,
            "deny must not unpublish"
        );

        registry
            .uninstall_as(&a, &uri, PUBLISHED_BINDING)
            .expect("publisher may unpublish");
        assert!(registry.list_installed(None).is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
