//! Disposable legacy fixture preparation and stopped-store SELECTs; see examples/AGENTS.md.

#[path = "legacy_adoption_fixture/files.rs"]
mod files;
#[path = "legacy_adoption_fixture/generator.rs"]
mod generator;
#[path = "legacy_adoption_fixture/inspect.rs"]
mod inspect;
#[path = "legacy_adoption_fixture/stopped.rs"]
mod stopped;

use files::{Context, Result};
use serde_json::{Value, json};
use std::{io::Read, time::Duration};

fn input() -> Result<Value> {
    let mut bytes = Vec::new();
    std::io::stdin()
        .take(files::INPUT_LIMIT + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| "fixture_request_unavailable")?;
    if bytes.len() as u64 > files::INPUT_LIMIT {
        return Err("fixture_request_overflow");
    }
    serde_json::from_slice(&bytes).map_err(|_| "invalid_fixture_request")
}

async fn run() -> Result<Value> {
    let mode = std::env::args().nth(1).ok_or("invalid_fixture_mode")?;
    if std::env::args().len() != 2
        || ![
            "generate",
            "seal",
            "discovery",
            "baseline",
            "adopted",
            "resume",
        ]
        .contains(&mode.as_str())
    {
        return Err("invalid_fixture_mode");
    }
    let request = input()?;
    // Process-lifetime pins: only OS exit releases the final diagnostic lease.
    let context = Box::leak(Box::new(Context::open(&request, mode == "generate")?));
    if mode == "generate" {
        for relative in ["data", "fractalengine", "assets", "assets/models"] {
            let path = context.run.join(relative);
            let _parents = files::pin_existing(&path)?;
            std::fs::create_dir(path).map_err(|_| "fixture_directory_not_fresh")?;
        }
        let source = generator::generate(&context.instance)?;
        let geometry = generator::verify(&source, &context.instance)?;
        let mut prepared = context.identity();
        prepared["content_hash"] = json!(blake3::hash(&source).to_hex().to_string());
        prepared["size_bytes"] = json!(source.len());
        prepared["geometry"] = geometry;
        files::create_json(&context.run.join("legacy-preparing.json"), &prepared)?;
        files::create_bytes(
            &context
                .run
                .join(format!("assets/models/{}.glb", generator::NAME)),
            &source,
        )?;
        return Ok(prepared);
    }
    if mode == "seal" {
        if context.run.join("current.json").exists()
            || context.run.join("data/fractalengine.db").exists()
            || context.run.join("identity").exists()
        {
            return Err("fixture_seal_requires_unopened_store");
        }
        let mut prepared = files::read_json(
            &context.run.join("legacy-preparing.json"),
            files::INPUT_LIMIT,
        )?;
        for (key, value) in context.identity().as_object().expect("identity") {
            if prepared.get(key) != Some(value) {
                return Err("fixture_preparation_changed");
            }
        }
        let source = generator::generate(&context.instance)?;
        let mut actual = Vec::new();
        files::read_file(
            &context
                .run
                .join(format!("assets/models/{}.glb", generator::NAME)),
        )?
        .take(1024 * 1024 + 1)
        .read_to_end(&mut actual)
        .map_err(|_| "fixture_source_unavailable")?;
        if actual != source {
            return Err("fixture_source_changed");
        }
        let sha = files::text(&request, "input_sha256")?;
        if sha.len() != 64 || !sha.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err("invalid_fixture_sha256");
        }
        prepared["input_sha256"] = json!(sha);
        prepared["setup_only"] = json!(true);
        files::create_json(&context.run.join("legacy-fixture.json"), &prepared)?;
        return Ok(prepared);
    }
    let report = inspect::run(&context, &request, &mode).await?;
    if mode != "resume" {
        files::create_json(&context.run.join(format!("legacy-{mode}.json")), &report)?;
    }
    Ok(report)
}

#[tokio::main]
async fn main() {
    let result = tokio::time::timeout(Duration::from_secs(30), run())
        .await
        .unwrap_or(Err("fixture_helper_deadline"));
    let (value, failed) = match result {
        Ok(data) => (json!({"ok":true,"data":data}), false),
        Err(code) => (json!({"ok":false,"code":code}), true),
    };
    match serde_json::to_vec(&value) {
        Ok(bytes) if bytes.len() <= files::OUTPUT_LIMIT => {
            println!("{}", String::from_utf8_lossy(&bytes))
        }
        _ => {
            println!("{{\"ok\":false,\"code\":\"fixture_output_overflow\"}}");
            std::process::exit(1);
        }
    }
    if failed {
        std::process::exit(1);
    }
}
