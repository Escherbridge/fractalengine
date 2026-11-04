//! DB-local Petal view proof through stored ancestry; see src/AGENTS.md.

use crate::repo::Db;
use fe_runtime::{messages::EntityType, native_navigation::{PetalTarget, PetalView}};
use serde_json::json;

pub(crate) async fn read(
    db: &Db,
    target: &PetalTarget,
    local_did: &str,
) -> anyhow::Result<PetalView> {
    anyhow::ensure!(target.is_bounded(), "native navigation target unavailable");
    let access = super::entity::prepare_parent_access(
        db, EntityType::Petal, &target.petal_id, local_did, fe_policy::RoleLevel::Viewer,
    ).await?;
    let scope = access.scope().to_owned();
    let parts = crate::parse_scope(&scope)?;
    anyhow::ensure!(
        parts.verse_id == target.verse_id
            && parts.fractal_id.as_deref() == Some(target.fractal_id.as_str())
            && parts.petal_id.as_deref() == Some(target.petal_id.as_str()),
        "native navigation ancestry unavailable"
    );
    let effective_role = access.role().to_string();
    let rows = access.read_rows(
        db,
        "SELECT petal_id, fractal_id, name FROM petal WHERE petal_id = $native_petal LIMIT 2 TIMEOUT 500ms",
        vec![("native_petal".into(), json!(target.petal_id))],
    ).await?;
    anyhow::ensure!(
        rows.len() == 1
            && rows[0]["petal_id"].as_str() == Some(target.petal_id.as_str())
            && rows[0]["fractal_id"].as_str() == Some(target.fractal_id.as_str())
            && rows[0]["name"].as_str() == Some(target.name.as_str()),
        "native navigation source unavailable"
    );
    let source = serde_json::to_vec(&json!({
        "scope": scope, "target": rows[0], "role": effective_role, "actor": local_did,
    }))?;
    Ok(PetalView {
        target: target.clone(),
        source_fence: blake3::hash(&source).to_hex().to_string(),
    })
}
