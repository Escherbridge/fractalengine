//! `new <kind> <name>`: scaffold a guest crate, a manifest v2 file, a fixture and a journey test.

use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use serde_json::{json, Value};

const FLEET_SIM: &str = include_str!("../../../fe-sdk/schema/examples/fleet-sim.extension.json");
const DEVICE_ADAPTER: &str = include_str!("../../../fe-sdk/schema/examples/device-adapter.extension.json");
const BI_PUSH: &str = include_str!("../../../fe-sdk/schema/examples/bi-push.extension.json");

/// The extension kinds `new` can scaffold.
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum Kind {
    Simulation,
    DeviceAdapter,
    BiConnector,
    General,
}

/// Where the scaffold's path dependencies point: this checkout's `tools/fe-ext`.
fn tool_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

/// `my-ext` -> `my_ext`: a property-scope-safe prefix.
fn property_prefix(name: &str) -> String {
    name.rsplit('.').next().unwrap_or(name).chars().map(|c| if c.is_ascii_alphanumeric() { c.to_ascii_lowercase() } else { '_' }).collect()
}

/// The manifest for `kind`: a shipped example with the identity swapped, or a minimal general one.
fn manifest_for(kind: Kind, id: &str, display_name: &str, prefix: &str) -> Result<Value> {
    let mut manifest: Value = match kind {
        Kind::Simulation => serde_json::from_str(FLEET_SIM)?,
        Kind::DeviceAdapter => serde_json::from_str(DEVICE_ADAPTER)?,
        Kind::BiConnector => serde_json::from_str(BI_PUSH)?,
        Kind::General => json!({
            "manifest_version": 2,
            "id": id,
            "version": "0.1.0",
            "name": display_name,
            "kind": "general",
            "contract": "fractalengine:plugin@2.0.0",
            "binding": "petal",
            "scene": { "read": true, "write": ["set_property"], "property_scope": [format!("{prefix}.*")] },
            "tick": { "cadence_ms": 1000, "max_ops": 16, "max_readings": 16, "fuel": 200000 }
        }),
    };
    manifest["id"] = json!(id);
    manifest["name"] = json!(display_name);
    manifest["version"] = json!("0.1.0");
    Ok(manifest)
}

/// A property key inside the manifest's first `<prefix>.*` scope, so the sample `on_tick` write is admitted.
fn stamp_key(manifest: &Value, fallback_prefix: &str) -> String {
    manifest["scene"]["property_scope"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .find_map(|pattern| pattern.strip_suffix('*').filter(|stem| stem.ends_with('.')))
        .map_or_else(|| format!("{fallback_prefix}.last_tick"), |stem| format!("{stem}last_tick"))
}

/// The starter fixture; a manifest that declares a source-class SET needs the bind-time pick, or the sandbox refuses the binding.
fn fixture_for(manifest: &Value) -> Result<String> {
    let mut fixture = json!({
        "petal_id": "p1",
        "start_ms": 1_000_000,
        "ticks": 5,
        "nodes": [ { "id": "n1", "name": "Node 1" }, { "id": "n2", "name": "Node 2" } ]
    });
    if let Some(first) = manifest["readings"]["source_class"].as_array().and_then(|classes| classes.first()) {
        fixture["source_class"] = first.clone();
    }
    Ok(serde_json::to_string_pretty(&fixture)? + "\n")
}

fn write(path: &Path, text: &str) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, text).with_context(|| format!("writing {}", path.display()))
}

/// Create `<parent>/<slug>/`; returns the project directory. Fails if it already exists.
pub fn scaffold(kind: Kind, name: &str, parent: &Path) -> Result<PathBuf> {
    let id = if name.contains('.') { name.to_string() } else { format!("local.{name}") };
    let slug = id.rsplit('.').next().unwrap_or(&id).to_string();
    let crate_name = slug.replace('_', "-");
    let prefix = property_prefix(&id);
    let dir = parent.join(&crate_name);
    if dir.exists() {
        bail!("{} already exists", dir.display());
    }

    let manifest = manifest_for(kind, &id, &slug, &prefix)?;
    let manifest_text = serde_json::to_string_pretty(&manifest)? + "\n";
    // The scaffold must itself pass the same table `validate` runs.
    crate::validate::validate_manifest(manifest_text.as_bytes()).context("the scaffolded manifest does not validate")?;

    let stamp = stamp_key(&manifest, &prefix);
    let root = tool_root();
    let guest = root.join("guest");
    write(&dir.join("extension.json"), &manifest_text)?;
    write(
        &dir.join("Cargo.toml"),
        &format!(
            "[package]\nname = \"{crate_name}\"\nversion = \"0.1.0\"\nedition = \"2021\"\npublish = false\n\n\
             [lib]\ncrate-type = [\"cdylib\", \"rlib\"]\n\n\
             [dependencies]\nfe-ext-guest = {{ path = \"{}\" }}\nserde_json = \"1\"\n\n\
             [dev-dependencies]\nfe-ext = {{ path = \"{}\" }}\n\n\
             # Standalone: built for wasm32-wasip2, never part of a surrounding workspace.\n[workspace]\n",
            guest.display().to_string().replace('\\', "/"),
            root.display().to_string().replace('\\', "/"),
        ),
    )?;
    write(
        &dir.join("src/lib.rs"),
        &format!(
            "//! {slug}: a FractalEngine extension. Contract: docs/extensions/authoring-guide.md.\n\n\
             use fe_ext_guest::{{extension, scene, window, Extension, TickInfo}};\n\n\
             struct App;\n\n\
             impl Extension for App {{\n    \
                 /// Stamp the tick number on the first few nodes of the bound petal.\n    \
                 fn on_tick(info: TickInfo) {{\n        \
                     let nodes = scene::query_nodes(r#\"{{\"limit\":4}}\"#).unwrap_or_default();\n        \
                     for node in nodes {{\n            \
                         let _ = window::set_property(&node.node_id, \"{stamp}\", &serde_json::json!(info.tick));\n        \
                     }}\n    \
                 }}\n}}\n\nextension!(App);\n"
        ),
    )?;
    write(&dir.join("fixtures/basic.json"), &fixture_for(&manifest)?)?;
    write(
        &dir.join("tests/journey.rs"),
        "//! Journey: the built component runs in the sandbox host and reproduces its fingerprint.\n\
         //! Needs `fe-ext build` first; skips quietly when dist/component.wasm is absent.\n\n\
         #![cfg(not(target_arch = \"wasm32\"))]\n\n\
         use std::path::Path;\n\n\
         #[test]\nfn sandbox_run_is_reproducible() {\n    \
             let project = Path::new(env!(\"CARGO_MANIFEST_DIR\"));\n    \
             if !project.join(\"dist/component.wasm\").is_file() {\n        return;\n    }\n    \
             let fixture = project.join(\"fixtures/basic.json\");\n    \
             let first = fe_ext::sandbox::run_project(project, &fixture, None).expect(\"first run\");\n    \
             let second = fe_ext::sandbox::run_project(project, &fixture, None).expect(\"second run\");\n    \
             assert_eq!(first.fingerprint, second.fingerprint);\n}\n",
    )?;
    write(&dir.join(".gitignore"), "target/\ndist/\n")?;
    Ok(dir)
}
