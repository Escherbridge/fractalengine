use fe_database::{DbHandle, PetalId};

pub fn is_petal_available_offline(petal_id: &PetalId) -> bool {
    local_namespace_path(petal_id).exists()
}

fn local_namespace_path(petal_id: &PetalId) -> std::path::PathBuf {
    dirs::data_local_dir()
        .unwrap_or_else(|| std::path::PathBuf::from("."))
        .join("fractalengine")
        .join("replicas")
        .join(petal_id.0.to_string())
}

pub async fn load_petal_offline(
    petal_id: &PetalId,
    _db_handle: &DbHandle,
) -> anyhow::Result<serde_json::Value> {
    Ok(serde_json::json!({
        "data": {},
        "stale": true,
        "petal_id": petal_id.0.to_string(),
    }))
}
