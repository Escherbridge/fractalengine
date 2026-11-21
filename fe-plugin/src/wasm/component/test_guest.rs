//! Test guests: a hand-written core module encoded against world `extension`; see
//! src/wasm/AGENTS.md §test-guests for the core signatures and why they are flat.

use std::sync::{Arc, Mutex};

use fe_plugin_test::mock_host::MockHostEnv;

use super::kv::{KvSlot, MemoryKv};
use super::{window_for_token, Binding, InstanceSetup};
use crate::capability::{CapabilityToken, V2Binding};
use crate::context::PluginContext;
use crate::tick::ManualClock;

/// The shipped fleet-sim manifest (scene read/write in `fleet.*`, readings, routes, kv 64 KiB).
const FLEET: &str = include_str!("../../../../fe-sdk/schema/examples/fleet-sim.extension.json");

/// The fleet manifest bound to petal `p1` of the mock host: v2 token, reader, shared window,
/// a 64-byte kv budget and the unfiltered geometry reader.
pub(crate) fn fleet_setup(host: &Arc<Mutex<MockHostEnv>>, clock: &ManualClock) -> InstanceSetup {
    setup_for(host, clock, FLEET.as_bytes())
}

/// [`fleet_setup`] with the manifest's `terrain: true` grant (no terrain source attached yet).
pub(crate) fn terrain_setup(host: &Arc<Mutex<MockHostEnv>>, clock: &ManualClock) -> InstanceSetup {
    let mut manifest: serde_json::Value = serde_json::from_str(FLEET).expect("fleet manifest is JSON");
    manifest["terrain"] = serde_json::Value::Bool(true);
    setup_for(host, clock, &serde_json::to_vec(&manifest).expect("manifest serializes"))
}

fn setup_for(host: &Arc<Mutex<MockHostEnv>>, clock: &ManualClock, manifest: &[u8]) -> InstanceSetup {
    let validated = fe_sdk::manifest_v2::parse_and_validate(manifest).expect("fleet manifest validates");
    let scopes = V2Binding {
        verse_scope: "VERSE#mock".into(),
        petal_scope: "VERSE#mock-FRACTAL#mock-PETAL#p1".into(),
        source_class: None,
    };
    let token = CapabilityToken::mint_v2("fleet", &validated.manifest, &scopes).expect("fleet token mints");
    let window = window_for_token(&token);
    let (commands, _receiver) = crossbeam::channel::bounded(1);
    let context = PluginContext::new("fleet".into(), "p1".into(), token, commands).with_node_reader(host.clone());
    let binding = Binding {
        installation_id: "inst-1".into(),
        package_id: validated.manifest.id.clone(),
        content_hash: "0".repeat(64),
        verse_id: "mock".into(),
        fractal_id: "mock".into(),
        petal_id: "p1".into(),
        identity_did: "did:key:local".into(),
        source_class: Some("simulation".into()),
    };
    InstanceSetup::new(binding, context, window, Arc::new(clock.clone()))
        .with_kv(KvSlot::new(Box::new(MemoryKv::new()), 64))
        .with_geometry_reader(host.clone())
}

/// `on-tick` body: stage `set-transform(n1, position [tick, 0, 0])`, then `log.write(info, "tick")`.
pub(crate) const STAGE_ONE_TRANSFORM: &str = r#"
    (call $stage_set_transform
      (i32.const 16) (i32.const 2)
      (i32.const 1) (f64.convert_i64_u (local.get $tick)) (f64.const 0) (f64.const 0)
      (i32.const 0) (f64.const 0) (f64.const 0) (f64.const 0) (f64.const 0)
      (i32.const 0) (f64.const 0) (f64.const 0) (f64.const 0)
      (i32.const 2048))
    (call $log_write (i32.const 1) (i32.const 32) (i32.const 4))
"#;

/// `on-tick` body that never returns: any fuel budget runs out.
pub(crate) const BURN_FOREVER: &str = "(loop $spin (br $spin))";

/// `hooks.on-hook` by input: byte 2 `t` (`["trap"]`) traps, `s` (`["spin"]`) loops until interrupted, anything
/// else answers `ok({"verdict":"allow"})`. Result area at 1536: `ok` tag, output ptr/len, empty body.
const ON_HOOK: &str = r#"
  (data (i32.const 48) "{\"verdict\":\"allow\"}")
  (func (export "fractalengine:plugin/hooks@2.0.0#on-hook")
    (param i32 i32 i32 i32 i32 i32 i32 i32) (param $input i32) (param $input_len i32) (param i32 i32) (result i32)
    (local $mode i32)
    (if (i32.gt_u (local.get $input_len) (i32.const 2))
      (then (local.set $mode (i32.load8_u offset=2 (local.get $input)))))
    (if (i32.eq (local.get $mode) (i32.const 116)) (then unreachable))
    (if (i32.eq (local.get $mode) (i32.const 115)) (then (loop $spin (br $spin))))
    (i32.store8 (i32.const 1536) (i32.const 0))
    (i32.store (i32.const 1540) (i32.const 48))
    (i32.store (i32.const 1544) (i32.const 19))
    (i32.store (i32.const 1548) (i32.const 0))
    (i32.store (i32.const 1552) (i32.const 0))
    (i32.const 1536))
"#;

/// A component whose `on-tick` runs `on_tick_body`; every other export is a no-op that returns
/// a zeroed result area (`ok`, `none`).
pub(crate) fn component(on_tick_body: &str) -> Vec<u8> {
    encode(on_tick_body, "", "extension")
}

/// A component of world `extension-with-hooks`: a no-op `on-tick` and the [`ON_HOOK`] judge.
pub(crate) fn hooks_component() -> Vec<u8> {
    encode("", ON_HOOK, "extension-with-hooks")
}

fn encode(on_tick_body: &str, hooks_export: &str, world_name: &str) -> Vec<u8> {
    let wat = format!(
        r#"(module
  (import "fractalengine:plugin/txn@2.0.0" "stage-set-transform"
    (func $stage_set_transform (param i32 i32 i32 f64 f64 f64 i32 f64 f64 f64 f64 i32 f64 f64 f64 i32)))
  (import "fractalengine:plugin/log@2.0.0" "write" (func $log_write (param i32 i32 i32)))
  (memory (export "memory") 1)
  (global $heap (mut i32) (i32.const 4096))
  (data (i32.const 16) "n1")
  (data (i32.const 32) "tick")
  (func (export "cabi_realloc") (param $old i32) (param $old_size i32) (param $align i32) (param $size i32) (result i32)
    (local $ptr i32)
    (local.set $ptr
      (i32.and
        (i32.add (global.get $heap) (i32.sub (local.get $align) (i32.const 1)))
        (i32.sub (i32.const 0) (local.get $align))))
    (global.set $heap (i32.add (local.get $ptr) (local.get $size)))
    (local.get $ptr))
  (func (export "fractalengine:plugin/lifecycle@2.0.0#on-install") (param i32) (result i32) (i32.const 1024))
  (func (export "fractalengine:plugin/lifecycle@2.0.0#on-activate") (param i32) (result i32) (i32.const 1024))
  (func (export "fractalengine:plugin/lifecycle@2.0.0#on-deactivate") (param i32))
  (func (export "fractalengine:plugin/lifecycle@2.0.0#on-uninstall") (param i32))
  (func (export "fractalengine:plugin/lifecycle@2.0.0#on-tick")
    (param $tick i64) (param i64 i64 i32 i32 i32 i64 i32 i32 i32 i32)
    {on_tick_body})
  (func (export "fractalengine:plugin/ingress-hooks@2.0.0#on-ingress") (param i32) (result i32) (i32.const 1024))
  (func (export "fractalengine:plugin/ingress-hooks@2.0.0#on-connection") (param i32 i32 i64 i32))
  (func (export "fractalengine:plugin/egress-hooks@2.0.0#on-egress-result") (param i64 i32 i32 i32 i32 i32 i32 i32 i32 i32))
  (func (export "fractalengine:plugin/bus-hooks@2.0.0#on-events") (param i32 i32))
  (func (export "fractalengine:plugin/bus-hooks@2.0.0#on-command") (param i32 i32 i32 i32 i32 i32 i32 i32 i32 i32 i32) (result i32) (i32.const 1024))
  (func (export "fractalengine:plugin/bus-hooks@2.0.0#on-call-result") (param i64 i32 i32 i32 i32))
  {hooks_export}
)"#
    );
    let mut module = wat::parse_str(&wat).expect("test guest WAT parses");
    let mut resolve = wit_parser::Resolve::default();
    let package = resolve
        .push_file(concat!(env!("CARGO_MANIFEST_DIR"), "/wit/fractal-plugin-2.wit"))
        .expect("the 2.0 WIT parses");
    let world = resolve
        .select_world(&[package], Some(world_name))
        .expect("the test world exists");
    wit_component::embed_component_metadata(&mut module, &resolve, world, wit_component::StringEncoding::UTF8)
        .expect("world metadata embeds");
    wit_component::ComponentEncoder::default()
        .module(&module)
        .expect("core module matches the world's lowered signatures")
        .validate(true)
        .encode()
        .expect("component encodes")
}
