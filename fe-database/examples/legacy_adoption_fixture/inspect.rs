use super::{
    files::{self, Context, Result, text},
    generator, stopped,
};
use serde_json::{Value, json};
use std::{io::Read, path::Path, time::Duration};
use surrealdb::engine::local::SurrealKv;

const HASH: &str = "encoding::base64::encode(encoding::cbor::encode($this))";

async fn select(
    db: &fe_database::repo::Db,
    sql: String,
    vars: &[(&str, Value)],
) -> Result<Vec<Value>> {
    if !sql.starts_with("SELECT ") || sql.contains(';') {
        return Err("invalid_fixture_select");
    }
    let mut query = db.query(sql);
    for (key, value) in vars {
        query = query.bind(((*key).to_owned(), value.clone()));
    }
    let mut response = query
        .await
        .and_then(|r| r.check())
        .map_err(|_| "fixture_select_failed")?;
    let rows: Vec<Value> = response
        .take(0)
        .map_err(|_| "fixture_select_decode_failed")?;
    if serde_json::to_vec(&rows)
        .map_err(|_| "fixture_select_decode_failed")?
        .len()
        > 2 * 1024 * 1024
    {
        return Err("fixture_select_overflow");
    }
    Ok(rows)
}

fn single(rows: Vec<Value>) -> Result<Value> {
    if rows.len() != 1 {
        Err("fixture_singleton_missing_or_ambiguous")
    } else {
        Ok(rows.into_iter().next().expect("one row"))
    }
}
fn digest(value: &Value) -> Result<String> {
    let witness = value["fingerprint"]
        .as_str()
        .filter(|s| !s.is_empty() && s.len() <= 64 * 1024)
        .ok_or("invalid_fixture_fingerprint")?;
    Ok(blake3::hash(witness.as_bytes()).to_hex().to_string())
}
fn ulid(value: &Value, key: &str) -> Result<String> {
    let s = text(value, key)?;
    ulid::Ulid::from_string(s).map_err(|_| "invalid_fixture_identity")?;
    Ok(s.to_owned())
}

pub async fn run(context: &Context, request: &Value, phase: &str) -> Result<Value> {
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
    let db_path = context.run.join("data/fractalengine.db");
    files::reject_redirected(&db_path)?;
    if !db_path.is_dir()
        || std::fs::read_dir(&db_path)
            .map_err(|_| "fixture_store_unavailable")?
            .next()
            .is_none()
    {
        return Err("fixture_existing_store_required");
    }
    let _store_parents = files::pin_existing(&db_path.join(".inspect-pin"))?;
    let manifest_dir = db_path.join("manifest");
    files::reject_redirected(&manifest_dir)?;
    let _manifest_parents = files::pin_existing(&manifest_dir.join(".inspect-pin"))?;
    let entries = std::fs::read_dir(&manifest_dir)
        .map_err(|_| "fixture_existing_store_required")?
        .take(33)
        .collect::<std::io::Result<Vec<_>>>()
        .map_err(|_| "fixture_existing_store_required")?;
    for entry in &entries {
        files::reject_redirected(&entry.path())?;
    }
    if entries.is_empty()
        || entries.len() > 32
        || !entries.iter().any(|entry| {
            let name = entry.file_name();
            let Some(name) = name.to_str() else {
                return false;
            };
            name.strip_suffix(".manifest")
                .is_some_and(|stem| stem.len() == 20 && stem.bytes().all(|b| b.is_ascii_digit()))
                && entry.metadata().is_ok_and(|m| m.is_file() && m.len() > 0)
        })
    {
        return Err("fixture_existing_store_required");
    }

    let db =
        surrealdb::Surreal::new::<SurrealKv>(db_path.to_str().ok_or("invalid_fixture_store_path")?)
            .await
            .map_err(|_| "fixture_store_unavailable")?;
    db.use_ns("fractalengine")
        .use_db("fractalengine")
        .await
        .map_err(|_| "fixture_store_unavailable")?;
    let result = tokio::time::timeout(
        Duration::from_secs(25),
        inspect(&db, context, &current, &preparation, phase),
    )
    .await
    .map_err(|_| "fixture_inspection_deadline")??;
    drop(db);
    stopped::verify(&current, request)?;
    Ok(result)
}

async fn inspect(
    db: &fe_database::repo::Db,
    context: &Context,
    current: &Value,
    preparation: &Value,
    phase: &str,
) -> Result<Value> {
    let vars = [
        ("name", json!(generator::NAME)),
        ("hash", preparation["content_hash"].clone()),
    ];
    let size=single(select(db,format!("SELECT asset_id, string::len({HASH}) AS encoded_size FROM asset WHERE name=$name AND content_hash=$hash LIMIT 2 TIMEOUT 2s"),&vars).await?)?;
    if size["encoded_size"]
        .as_u64()
        .filter(|n| *n <= 64 * 1024)
        .is_none()
    {
        return Err("fixture_asset_metadata_overflow");
    }
    let asset=single(select(db,format!("SELECT asset_id,name,content_type,size_bytes,content_hash,created_at,owner_did,petal_id,publication_provenance,object::keys($this) AS fields,{HASH} AS fingerprint FROM asset WHERE name=$name AND content_hash=$hash LIMIT 2 TIMEOUT 2s"),&vars).await?)?;
    let asset_id = ulid(&asset, "asset_id")?;
    chrono::DateTime::parse_from_rfc3339(text(&asset, "created_at")?)
        .map_err(|_| "invalid_fixture_asset_time")?;
    if asset["content_type"] != "model/gltf-binary"
        || asset["size_bytes"] != preparation["size_bytes"]
    {
        return Err("fixture_asset_metadata_changed");
    }
    let fields = asset["fields"]
        .as_array()
        .ok_or("fixture_asset_decode_failed")?;
    if fields.len() > 32
        || fields
            .iter()
            .any(|v| v.as_str().is_none_or(|s| s.len() > 128))
    {
        return Err("fixture_asset_decode_failed");
    }
    let presence = |key: &str| fields.iter().any(|field| field == key);
    if presence("owner_did") || (!asset["owner_did"].is_null()) {
        return Err("fixture_is_not_ownerless");
    }
    let provenance = &asset["publication_provenance"];
    let adopted = presence("publication_provenance");
    if (!presence("petal_id") && !asset["petal_id"].is_null())
        || (!adopted && !provenance.is_null())
    {
        return Err("fixture_presence_decode_failed");
    }
    if presence("petal_id") != adopted {
        return Err("fixture_partial_adoption_state");
    }
    if adopted {
        if provenance.as_object().map(|o| o.len()) != Some(4)
            || provenance["kind"] != "explicit_local_copy_adoption"
        {
            return Err("unexpected_fixture_provenance");
        }
        ulid(provenance, "operation_id")?;
        chrono::DateTime::parse_from_rfc3339(text(provenance, "adopted_at")?)
            .map_err(|_| "invalid_fixture_adoption_time")?;
    }
    if (phase == "adopted") != adopted && phase != "resume" {
        return Err("unexpected_fixture_adoption_phase");
    }
    let vars = [("asset", json!(asset_id))];
    let refs=select(db,"SELECT node_id,petal_id,tombstone FROM node WHERE asset_id=$asset ORDER BY node_id LIMIT 257 TIMEOUT 2s".into(),&vars).await?;
    if refs.len() > 256 {
        return Err("fixture_reference_overflow");
    }
    if (phase == "discovery" && !refs.is_empty())
        || (matches!(phase, "baseline" | "adopted") && refs.len() != 2)
        || (phase == "resume" && ![0, 2].contains(&refs.len()))
    {
        return Err("unexpected_fixture_reference_count");
    }
    let models = select(
        db,
        "SELECT asset_id,petal_id FROM model WHERE asset_id=$asset LIMIT 2 TIMEOUT 2s".into(),
        &vars,
    )
    .await?;
    let payloads = select(
        db,
        "SELECT payload_id FROM device_payload WHERE asset_ids CONTAINS $asset LIMIT 2 TIMEOUT 2s"
            .into(),
        &vars,
    )
    .await?;
    if !models.is_empty() || !payloads.is_empty() {
        return Err("unexpected_fixture_model_or_payload_reference");
    }
    let genesis=single(select(db,"SELECT verse_id,created_by FROM verse WHERE verse_id=$bootstrap AND name='Genesis Verse' LIMIT 2 TIMEOUT 2s".into(),&[("bootstrap",current["bootstrap_verse_id"].clone())]).await?)?;
    let runtime_did = text(&current["fixture_detail"], "runtime_did")?;
    fe_identity::api_token::validate_client_did(runtime_did)
        .map_err(|_| "invalid_fixture_runtime_identity")?;
    if genesis["created_by"] != runtime_did || (adopted && provenance["adopter_did"] != runtime_did)
    {
        return Err("fixture_runtime_owner_changed");
    }
    let mut hierarchy = Value::Null;
    if !refs.is_empty() {
        if refs
            .iter()
            .any(|r| r["petal_id"] != refs[0]["petal_id"] || !r["tombstone"].is_null())
        {
            return Err("fixture_foreign_or_tombstoned_reference");
        }
        let petal = single(
            select(
                db,
                "SELECT petal_id,fractal_id FROM petal WHERE petal_id=$petal LIMIT 2 TIMEOUT 2s"
                    .into(),
                &[("petal", refs[0]["petal_id"].clone())],
            )
            .await?,
        )?;
        let fractal=single(select(db,"SELECT fractal_id,verse_id,owner_did FROM fractal WHERE fractal_id=$fractal LIMIT 2 TIMEOUT 2s".into(),&[("fractal",petal["fractal_id"].clone())]).await?)?;
        let verse=single(select(db,"SELECT verse_id,created_by,namespace_id FROM verse WHERE verse_id=$verse LIMIT 2 TIMEOUT 2s".into(),&[("verse",fractal["verse_id"].clone())]).await?)?;
        if verse["created_by"] != runtime_did
            || fractal["owner_did"] != runtime_did
            || verse["namespace_id"]
                .as_str()
                .filter(|s| s.len() == 64 && s.bytes().all(|b| b.is_ascii_hexdigit()))
                .is_none()
        {
            return Err("fixture_actual_owner_or_namespace_missing");
        }
        if adopted && asset["petal_id"] != petal["petal_id"] {
            return Err("fixture_adoption_anchor_changed");
        }
        hierarchy =
            json!({"petal":petal,"fractal":fractal,"verse":verse,"actual_owner_by_creator":true});
    }
    let mut nodes = Vec::new();
    for reference in &refs {
        let node_id = ulid(reference, "node_id")?;
        let vars = [("node", json!(node_id))];
        let sized=single(select(db,format!("SELECT string::len({HASH}) AS encoded_size FROM node WHERE node_id=$node LIMIT 2 TIMEOUT 2s"),&vars).await?)?;
        if sized["encoded_size"]
            .as_u64()
            .filter(|n| *n <= 64 * 1024)
            .is_none()
        {
            return Err("fixture_node_overflow");
        }
        let node=single(select(db,format!("SELECT node_id,asset_id,petal_id,display_name,position,elevation,rotation,scale,edit_seq,interactive,properties,{HASH} AS fingerprint FROM node WHERE node_id=$node LIMIT 2 TIMEOUT 2s"),&vars).await?)?;
        if node["properties"].as_object().is_none_or(|o| !o.is_empty()) {
            return Err("unexpected_fixture_node_properties");
        }
        if !["QA Legacy A", "QA Legacy B"].contains(&text(&node, "display_name")?)
            || node["asset_id"] != asset_id
            || node["petal_id"] != reference["petal_id"]
            || node["interactive"].as_bool() != Some(true)
            || node["edit_seq"].as_u64().is_none()
        {
            return Err("unexpected_fixture_node_state");
        }
        for (values, counts) in [
            (&node["position"]["coordinates"], &[2_usize][..]),
            (&node["rotation"], &[3_usize, 4][..]),
            (&node["scale"], &[3_usize][..]),
        ] {
            if values.as_array().is_none_or(|a| {
                !counts.contains(&a.len())
                    || a.iter()
                        .any(|v| v.as_f64().is_none_or(|n| !n.is_finite() || n.abs() > 1e6))
            }) {
                return Err("invalid_fixture_numeric_state");
            }
        }
        if node["elevation"]
            .as_f64()
            .is_none_or(|n| !n.is_finite() || n.abs() > 1e6)
        {
            return Err("invalid_fixture_numeric_state");
        }
        let sizes=select(db,format!("SELECT row_version, string::len({HASH}) AS encoded_size FROM node_log WHERE node_id=$node ORDER BY row_version LIMIT 33 TIMEOUT 2s"),&vars).await?;
        if sizes.is_empty()
            || sizes.len() > 32
            || sizes.iter().enumerate().any(|(i, r)| {
                r["row_version"].as_u64() != Some(i as u64 + 1)
                    || r["encoded_size"]
                        .as_u64()
                        .filter(|n| *n <= 32 * 1024)
                        .is_none()
            })
        {
            return Err("fixture_history_missing_gap_or_overflow");
        }
        let facts=select(db,format!("SELECT node_id,row_version,op,{HASH} AS fingerprint FROM node_log WHERE node_id=$node ORDER BY row_version LIMIT 33 TIMEOUT 2s"),&vars).await?;
        if facts.len() != sizes.len() {
            return Err("fixture_history_changed");
        }
        if facts.iter().enumerate().any(|(i, r)| {
            r["node_id"] != node_id
                || r["row_version"].as_u64() != Some(i as u64 + 1)
                || ![
                    "baseline",
                    "full_transform",
                    "node_renamed",
                    "property_set",
                    "property_deleted",
                ]
                .contains(&r["op"].as_str().unwrap_or(""))
        }) {
            return Err("invalid_fixture_history_fact");
        }
        let facts=facts.iter().map(|r|Ok(json!({"row_version":r["row_version"],"op":r["op"],"complete_fact_digest":digest(r)?}))).collect::<Result<Vec<_>>>()?;
        let mut public = node
            .as_object()
            .ok_or("fixture_node_decode_failed")?
            .clone();
        public.remove("fingerprint");
        nodes.push(json!({"current":public,"complete_row_digest":digest(&node)?,"history":facts}));
    }
    let op_sizes=select(db,format!("SELECT string::len({HASH}) AS encoded_size FROM op_log WHERE op_type='SceneAssetAdopted' AND payload.asset_id=$asset LIMIT 3 TIMEOUT 2s"),&[("asset",json!(asset_id))]).await?;
    if op_sizes.len() > 1
        || op_sizes.iter().any(|r| {
            r["encoded_size"]
                .as_u64()
                .filter(|n| *n <= 64 * 1024)
                .is_none()
        })
    {
        return Err("fixture_adoption_operation_overflow");
    }
    let ops=select(db,"SELECT lamport_clock,node_id,payload.operation_id AS operation_id,payload.request_id AS request_id,payload.scope AS scope,payload.content_hash AS content_hash,payload.size_bytes AS size_bytes,payload.content_type AS content_type,payload.provenance AS provenance FROM op_log WHERE op_type='SceneAssetAdopted' AND payload.asset_id=$asset LIMIT 3 TIMEOUT 2s".into(),&[("asset",json!(asset_id))]).await?;
    if ops.len() != (if adopted { 1 } else { 0 })
        || ops.iter().any(|r| {
            r["operation_id"] != provenance["operation_id"]
                || r["provenance"] != *provenance
                || r["content_hash"] != asset["content_hash"]
                || r["size_bytes"] != asset["size_bytes"]
                || r["content_type"] != asset["content_type"]
        })
    {
        return Err("fixture_adoption_operation_missing_or_ambiguous");
    }
    if nodes.len() == 2
        && nodes[0]["current"]["display_name"] == nodes[1]["current"]["display_name"]
    {
        return Err("unexpected_fixture_node_names");
    }
    for operation in &ops {
        let id = text(operation, "request_id")?
            .strip_prefix("ui-scene-asset-")
            .ok_or("not_a_native_fixture_adoption")?;
        ulid::Ulid::from_string(id).map_err(|_| "not_a_native_fixture_adoption")?;
        let expected_scope = format!(
            "VERSE#{}-FRACTAL#{}-PETAL#{}",
            text(&hierarchy["verse"], "verse_id")?,
            text(&hierarchy["fractal"], "fractal_id")?,
            text(&hierarchy["petal"], "petal_id")?
        );
        if operation["scope"] != expected_scope
            || !refs.iter().any(|r| r["node_id"] == operation["node_id"])
            || operation["lamport_clock"]
                .as_u64()
                .filter(|n| *n > 0)
                .is_none()
        {
            return Err("fixture_adoption_operation_changed");
        }
    }
    let hash = fe_runtime::blob_store::hash_from_hex(text(&asset, "content_hash")?)
        .map_err(|_| "invalid_fixture_hash")?;
    let blob = fe_runtime::blob_store::canonical_blob_path(&context.run.join("data/blobs"), &hash);
    let local_blob = canonical_blob(&blob, &asset)?;
    let report = json!({"schema":"fe-qa-legacy-inspection-v1","phase":phase,"preparation_id":context.instance,"runtime_did":runtime_did,
        "asset":{"asset_id":asset_id,"name":asset["name"],"content_hash":asset["content_hash"],"content_type":asset["content_type"],"size_bytes":asset["size_bytes"],"created_at":asset["created_at"],
                 "owner_present":presence("owner_did"),"petal_present":presence("petal_id"),"provenance_present":adopted,"petal_id":asset["petal_id"],"publication_provenance":provenance,"complete_row_digest":digest(&asset)?},
        "reference_count":refs.len(),"references":refs,"model_reference_count":models.len(),"payload_reference_count":payloads.len(),"hierarchy":hierarchy,"nodes":nodes,"adoption_operations":ops,
        "local_canonical_blob":local_blob,"fingerprint_encoding":"blake3(UTF8(base64(CBOR(DB-row))))","logical_database_writes":false,
        "cold_native_iroh_cache_proven":false,"adoption_or_transfer_executed_by_inspector":false});
    if phase == "adopted" {
        let baseline = files::read_json(
            &context.run.join("legacy-baseline.json"),
            files::OUTPUT_LIMIT as u64,
        )?;
        for key in [
            "asset_id",
            "name",
            "content_hash",
            "content_type",
            "size_bytes",
            "created_at",
        ] {
            if baseline["asset"][key] != report["asset"][key] {
                return Err("fixture_adoption_changed_asset_identity");
            }
        }
        if baseline["preparation_id"] != context.instance
            || baseline["nodes"] != report["nodes"]
            || baseline["references"] != report["references"]
            || baseline["local_canonical_blob"] != report["local_canonical_blob"]
        {
            return Err("fixture_adoption_changed_nodes_or_bytes");
        }
    }
    Ok(report)
}

fn canonical_blob(path: &Path, asset: &Value) -> Result<Value> {
    let _parents = files::pin_existing(path)?;
    let mut bytes = Vec::new();
    files::read_file(path)?
        .take(1024 * 1024 + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| "fixture_blob_unavailable")?;
    let hash = blake3::hash(&bytes).to_hex().to_string();
    if asset["content_hash"] != hash || asset["size_bytes"] != bytes.len() {
        return Err("fixture_blob_changed");
    }
    Ok(json!({"content_hash":hash,"size_bytes":bytes.len(),"regular_file":true}))
}
