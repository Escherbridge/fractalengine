//! Policy-gated registry operations — closes the Phase 8.4 RBAC gap (see src/AGENTS.md §authz).

use std::sync::{Arc, OnceLock};

use fe_policy::{
    Action, AuthContext, Decision, PolicyEngine, PublicReadPolicy, RoleLevelPolicy, Scope,
};

use crate::manifest::{HexonKind, HexonManifest, InstalledCrate};
use crate::package::HexonPackageData;
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

impl HexonRegistry {
    /// Policy-gated install: requires Editor+ at the petal scope.
    pub fn install_as(
        &mut self,
        subject: &AuthContext,
        package: HexonPackageData,
        petal_id: &str,
    ) -> Result<InstalledCrate, RegistryError> {
        authorize(subject, &Action::Install, petal_id)?;
        self.install(package, petal_id)
    }

    /// Policy-gated uninstall: requires Editor+ at the petal scope.
    pub fn uninstall_as(
        &mut self,
        subject: &AuthContext,
        hexon_uri: &str,
        petal_id: &str,
    ) -> Result<(), RegistryError> {
        authorize(subject, &Action::Write, petal_id)?;
        self.uninstall(hexon_uri, petal_id)
    }

    /// Policy-gated listing: public read (anonymous allowed by PublicReadPolicy).
    pub fn list_installed_as(
        &self,
        subject: &AuthContext,
        petal_id: Option<&str>,
    ) -> Result<Vec<InstalledCrate>, RegistryError> {
        authorize(subject, &Action::Read, petal_id.unwrap_or("*"))?;
        Ok(self.list_installed(petal_id))
    }

    /// Policy-gated search: public read (anonymous allowed by PublicReadPolicy).
    pub fn search_local_as(
        &self,
        subject: &AuthContext,
        query: &str,
        tags: &[&str],
        kind: Option<HexonKind>,
    ) -> Result<Vec<HexonManifest>, RegistryError> {
        authorize(subject, &Action::Read, "*")?;
        Ok(self.search_local(query, tags, kind))
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
        let keypair = SigningKey::generate(&mut OsRng);
        let pub_key = keypair.verifying_key();
        let publisher_did = {
            use multibase::Base;
            let mut bytes = vec![0xed, 0x01];
            bytes.extend_from_slice(&pub_key.to_bytes());
            format!("did:key:{}", multibase::encode(Base::Base58Btc, &bytes))
        };
        let manifest = HexonManifest {
            schema_version: 1,
            crate_id: "authz-model-v1".to_string(),
            publisher_did,
            publisher_name: "Authz Publisher".to_string(),
            version: "1.0.0".to_string(),
            build_id: "build-authz".to_string(),
            name: "Authz Model".to_string(),
            description: "Policy gate test crate".to_string(),
            tags: vec!["test".to_string()],
            hexon_type: HexonKind::Model,
            created_at: "2026-01-01T00:00:00Z".to_string(),
            updated_at: "2026-01-01T00:00:00Z".to_string(),
            approx_size_bytes: 1024,
            min_engine_version: "0.1.0".to_string(),
            homepage_url: None,
            dependencies: vec![],
            signature: String::new(),
        };
        let entries = vec![AssetEntry {
            entry_id: "model_001".to_string(),
            kind: EntryKind::Model,
            asset_hash: "abc123".to_string(),
            format: "gltf".to_string(),
            label: "Authz Model".to_string(),
            tags: vec![],
            description: "test".to_string(),
            is_placeable: true,
            is_private: false,
            preview_image: None,
            metadata: None,
            sub_assets: None,
        }];
        let license = License {
            license_type: LicenseType::Free,
            attribution: None,
            payment_provider: None,
            payment_verification_url: None,
            free_entries: vec![],
            encrypted_key: None,
        };
        let mut assets = HashMap::new();
        assets.insert("abc123".to_string(), b"fake model data".to_vec());
        let zip_bytes = HexonPackage::build(
            manifest,
            entries,
            license,
            assets,
            None,
            HashMap::new(),
            None,
            None,
            &keypair,
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
        assert!(registry.list_installed(None).is_empty(), "nothing may be installed on deny");
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
        assert_eq!(registry.list_installed(None).len(), 1, "deny must not uninstall");

        registry
            .uninstall_as(&did(RoleLevel::Editor), &uri, "petal-1")
            .expect("editor uninstall must pass");
        assert!(registry.list_installed(None).is_empty());
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
}
