use super::{
    files::{self, Context, Result},
    generator, stopped,
};
use serde_json::Value;
use std::io::Read;

pub async fn run(context: &Context, request: &Value, _phase: &str) -> Result<Value> {
    let current_path = context.run.join("current.json");
    let _current_file = files::read_file(&current_path)?;
    let current = files::read_json(&current_path, files::INPUT_LIMIT)?;
    if current["fixture"] != "legacy-adoption"
        || current["fixture_detail"]["preparation_id"] != context.instance
    {
        return Err("not_a_legacy_fixture");
    }
    for key in [
        "source_commit",
        "source_fingerprint",
        "exe_sha256",
        "legacy_fixture_sha256",
    ] {
        if current["stage_manifest"][key] != context.stage[key] {
            return Err("fixture_stage_changed");
        }
    }
    stopped::verify(&current, request)?;
    let _preparation_file = files::read_file(&context.run.join("legacy-fixture.json"))?;
    let preparation =
        files::read_json(&context.run.join("legacy-fixture.json"), files::INPUT_LIMIT)?;
    let expected = context.identity();
    for (key, value) in expected.as_object().expect("identity object") {
        if preparation.get(key) != Some(value) {
            return Err("fixture_preparation_changed");
        }
    }
    let glb_path = context
        .run
        .join(format!("assets/models/{}.glb", generator::NAME));
    let mut glb = Vec::new();
    files::read_file(&glb_path)?
        .take(1024 * 1024 + 1)
        .read_to_end(&mut glb)
        .map_err(|_| "fixture_source_unavailable")?;
    generator::verify(&glb, &context.instance)?;
    if preparation["content_hash"] != blake3::hash(&glb).to_hex().as_str()
        || preparation["size_bytes"] != glb.len()
    {
        return Err("fixture_source_changed");
    }
    Err("fixture_store_confinement_unavailable")
}
