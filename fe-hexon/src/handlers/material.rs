use super::InstallResult;
use crate::manifest::EntryKind;
use crate::package::HexonPackageData;
use crate::registry::{FsBlobStore, RegistryError};
use std::collections::HashMap;
use tracing::info;

/// PBR material handle with texture references for Bevy StandardMaterial construction.
///
/// Maps texture roles (albedo, normal, roughness, ao, metallic) to their
/// content-addressed blob hashes via the entry's `sub_assets` map.
#[derive(Debug, Clone)]
pub struct MaterialHandle {
    pub entry_id: String,
    pub albedo_hash: Option<String>,
    pub normal_hash: Option<String>,
    pub roughness_hash: Option<String>,
    pub ao_hash: Option<String>,
    pub metallic_hash: Option<String>,
    pub metadata: Option<serde_json::Value>,
}

impl MaterialHandle {
    /// Extract a MaterialHandle from an AssetEntry's sub_assets map.
    pub fn from_sub_assets(
        entry_id: &str,
        sub_assets: &Option<HashMap<String, String>>,
        metadata: &Option<serde_json::Value>,
    ) -> Self {
        let get = |key: &str| -> Option<String> { sub_assets.as_ref()?.get(key).cloned() };
        Self {
            entry_id: entry_id.to_string(),
            albedo_hash: get("albedo"),
            normal_hash: get("normal"),
            roughness_hash: get("roughness"),
            ao_hash: get("ao"),
            metallic_hash: get("metallic"),
            metadata: metadata.clone(),
        }
    }

    /// Collect all referenced hashes.
    pub fn all_hashes(&self) -> Vec<&str> {
        [
            self.albedo_hash.as_deref(),
            self.normal_hash.as_deref(),
            self.roughness_hash.as_deref(),
            self.ao_hash.as_deref(),
            self.metallic_hash.as_deref(),
        ]
        .iter()
        .filter_map(|h| *h)
        .collect()
    }
}

/// Handle installation of Material entries from a hexon package.
///
/// Stores all asset blobs (material bundles and their textures) in the
/// blob store, then builds `MaterialHandle` structs from each material
/// entry's `sub_assets` to verify all referenced texture blobs are present.
pub fn handle_material_install(
    package: &HexonPackageData,
    petal_id: &str,
    blob_store: &FsBlobStore,
) -> Result<InstallResult, anyhow::Error> {
    let mut result = InstallResult::default();
    let mut material_count = 0u32;

    for entry in &package.entries {
        // Store all asset blobs (materials, textures)
        if let Some(data) = package.assets.get(&entry.asset_hash) {
            blob_store.store(&entry.asset_hash, data)?;
            result.registered_assets.push(entry.asset_hash.clone());
        }

        if entry.kind != EntryKind::Material {
            continue;
        }

        let handle =
            MaterialHandle::from_sub_assets(&entry.entry_id, &entry.sub_assets, &entry.metadata);

        // Verify all sub-asset blobs exist and are stored
        for hash in handle.all_hashes() {
            if let Some(data) = package.assets.get(hash) {
                blob_store.store(hash, data)?;
                if !result.registered_assets.contains(&hash.to_string()) {
                    result.registered_assets.push(hash.to_string());
                }
            }
        }

        material_count += 1;
        info!(
            "Registered PBR material {} for petal {} (sub_assets: {:?})",
            entry.entry_id,
            petal_id,
            entry
                .sub_assets
                .as_ref()
                .map(|s| s.keys().collect::<Vec<_>>())
        );
    }

    result.summary = format!(
        "Installed {} material(s) with {} total blobs for petal {}",
        material_count,
        result.registered_assets.len(),
        petal_id
    );
    Ok(result)
}

// ============================================================================
// MaterialHandle → decoded texture loader (FR-3)
// ============================================================================
//
// This crate stays engine-agnostic (no bevy dep) — it resolves a
// `MaterialHandle`'s role→blob-hash map from `FsBlobStore` and decodes each
// blob to raw RGBA8 + dimensions. The caller (fe-ui, which owns Bevy) wraps
// the decoded bytes into `bevy::image::Image` + `StandardMaterial`. See
// `fe-ui/src/verse_manager/AGENTS.md` §primitives for the assembly side.

/// A decoded RGBA8 texture ready for the caller to wrap into a GPU image.
#[derive(Debug, Clone)]
pub struct DecodedTexture {
    pub width: u32,
    pub height: u32,
    /// Tightly-packed RGBA8 pixel data, row-major, top-to-bottom.
    pub rgba: Vec<u8>,
}

/// All decoded textures resolved from a [`MaterialHandle`]'s roles.
/// Any role whose blob is missing or fails to decode is simply absent —
/// callers assemble a material with whichever roles resolved.
#[derive(Debug, Clone, Default)]
pub struct ResolvedMaterial {
    pub albedo: Option<DecodedTexture>,
    pub normal: Option<DecodedTexture>,
    pub roughness: Option<DecodedTexture>,
    pub ao: Option<DecodedTexture>,
    pub metallic: Option<DecodedTexture>,
}

/// Load and decode a blob hash from the blob store into RGBA8 pixels.
fn load_decoded(blob_store: &FsBlobStore, hash: &str) -> Result<DecodedTexture, RegistryError> {
    let bytes = blob_store.load(hash)?;
    let img = image::load_from_memory(&bytes)
        .map_err(|e| {
            RegistryError::Package(anyhow::anyhow!("texture decode failed for {hash}: {e}"))
        })?
        .to_rgba8();
    Ok(DecodedTexture {
        width: img.width(),
        height: img.height(),
        rgba: img.into_raw(),
    })
}

/// Resolve a [`MaterialHandle`]'s role→blob-hash map into decoded textures.
///
/// Missing or undecodable blobs are skipped (logged) rather than failing the
/// whole material — a primitive with a partially-resolved material still
/// renders with whichever roles loaded (falling back to defaults for the rest).
pub fn resolve_material_textures(
    handle: &MaterialHandle,
    blob_store: &FsBlobStore,
) -> ResolvedMaterial {
    let try_load = |hash: &Option<String>, role: &str| -> Option<DecodedTexture> {
        let hash = hash.as_ref()?;
        match load_decoded(blob_store, hash) {
            Ok(tex) => Some(tex),
            Err(e) => {
                tracing::warn!(
                    "MaterialHandle {} role={} hash={}: {}",
                    handle.entry_id,
                    role,
                    hash,
                    e
                );
                None
            }
        }
    };

    ResolvedMaterial {
        albedo: try_load(&handle.albedo_hash, "albedo"),
        normal: try_load(&handle.normal_hash, "normal"),
        roughness: try_load(&handle.roughness_hash, "roughness"),
        ao: try_load(&handle.ao_hash, "ao"),
        metallic: try_load(&handle.metallic_hash, "metallic"),
    }
}

#[cfg(test)]
mod loader_tests {
    use super::*;
    use tempfile::tempdir;

    fn make_png_bytes(w: u32, h: u32) -> Vec<u8> {
        let img = image::RgbaImage::from_pixel(w, h, image::Rgba([200, 100, 50, 255]));
        let mut buf = Vec::new();
        image::DynamicImage::ImageRgba8(img)
            .write_to(&mut std::io::Cursor::new(&mut buf), image::ImageFormat::Png)
            .expect("encode test png");
        buf
    }

    #[test]
    fn resolves_albedo_only_when_only_albedo_hash_present() {
        let dir = tempdir().expect("tempdir");
        let store = FsBlobStore::new(dir.path().to_path_buf());
        let png = make_png_bytes(4, 4);
        store.store("albedo-hash", &png).expect("store blob");

        let handle = MaterialHandle {
            entry_id: "mat-1".to_string(),
            albedo_hash: Some("albedo-hash".to_string()),
            normal_hash: None,
            roughness_hash: None,
            ao_hash: None,
            metallic_hash: None,
            metadata: None,
        };

        let resolved = resolve_material_textures(&handle, &store);
        let albedo = resolved.albedo.expect("albedo resolved");
        assert_eq!((albedo.width, albedo.height), (4, 4));
        assert_eq!(albedo.rgba.len(), 4 * 4 * 4);
        assert!(resolved.normal.is_none());
    }

    #[test]
    fn missing_blob_resolves_to_none_without_erroring() {
        let dir = tempdir().expect("tempdir");
        let store = FsBlobStore::new(dir.path().to_path_buf());

        let handle = MaterialHandle {
            entry_id: "mat-2".to_string(),
            albedo_hash: Some("does-not-exist".to_string()),
            normal_hash: None,
            roughness_hash: None,
            ao_hash: None,
            metallic_hash: None,
            metadata: None,
        };

        let resolved = resolve_material_textures(&handle, &store);
        assert!(resolved.albedo.is_none());
    }
}
