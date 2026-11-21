//! J8: one authoring journey through the real `fe-ext` binary, from `new` to a counter-signed package,
//! plus the three shipped examples (R27) through `run --sandbox`, `validate`, `pack`, `sign`, `inspect` and `index build|verify`,
//! and a WAT guest driven through fixture `commands[]`, `hooks[]` and stream capture (`sandbox_calls_leg`).
//! Registered as the fe-ext batch in conductor/test-batches.md (`cargo test -p fe-ext -- j8`).

use std::path::Path;
use std::process::{Command, Output};

use serde_json::Value;

/// Run the real binary in `dir`.
fn fe_ext(args: &[&str], dir: &Path) -> Output {
    Command::new(env!("CARGO_BIN_EXE_fe-ext")).args(args).current_dir(dir).output().expect("fe-ext runs")
}

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

fn fingerprint(output: &Output) -> Option<String> {
    stdout(output).lines().find(|line| line.starts_with("fingerprint ")).map(str::to_string)
}

fn node<'a>(trace: &'a Value, id: &str) -> &'a Value {
    trace["nodes"].as_array().and_then(|nodes| nodes.iter().find(|node| node["id"] == id)).unwrap_or_else(|| panic!("node {id}"))
}

/// fleet-sim fixture: 11 temperature (500 ms) and 5 altitude (1 s) readings over 5.5 s; the truck near x = 55 on the
/// first 100 m leg; the fleet marked running and its driver claim released when the host stopped the instance.
fn fleet_sim_outcomes(trace: &Value) {
    let readings = trace["readings"].as_array().expect("readings");
    let count = |metric: &str| readings.iter().filter(|reading| reading["metric"] == metric).count();
    assert_eq!((count("temperature_c"), count("altitude_m")), (11, 5));
    let positions = trace["streams"].as_array().expect("streams");
    assert!(positions.iter().any(|event| event["stream"] == "positions" && event["published"] == true), "positions reach the bus");
    let truck_x = node(trace, "truck-1")["position"][0].as_f64().expect("x");
    assert!((50.0..60.0).contains(&truck_x), "truck x = {truck_x}");
    let fleet = &node(trace, "fleet-1")["properties"];
    assert_eq!((fleet["fleet.status"].as_str(), fleet["fleet.driver"].as_str()), (Some("running"), Some("")));
}

/// device-adapter fixture: http acks 202, websocket and mqtt stay quiet, a malformed frame is 400 and an unknown device 404;
/// four readings land on the node carrying device.id dev-1, which the websocket frame moved.
fn device_adapter_outcomes(trace: &Value) {
    let statuses: Vec<(u64, Option<u64>)> = trace["ingress_replies"]
        .as_array()
        .expect("replies")
        .iter()
        .map(|entry| (entry["tick"].as_u64().expect("tick"), entry["reply"]["status"].as_u64()))
        .collect();
    assert_eq!(statuses, vec![(1, Some(202)), (2, None), (2, None), (3, Some(400)), (3, Some(404))]);
    let readings = trace["readings"].as_array().expect("readings");
    assert_eq!(readings.len(), 4);
    assert!(readings.iter().all(|reading| reading["node_id"] == "sensor-1"));
    let sensor = node(trace, "sensor-1");
    assert_eq!(sensor["position"], serde_json::json!([12.5, 0.0, -4.0]));
    assert_eq!(sensor["properties"]["device.last_seen"].as_u64(), Some(2_001_000));
}

/// bi-push fixture: three ticks with the connector class and zero hook faults (a faulting run exits non-zero first).
fn bi_push_outcomes(trace: &Value) {
    assert!(trace["faults"].as_array().is_some_and(Vec::is_empty), "bi-push recorded faults: {}", trace["faults"]);
    node(trace, "sensor-1");
}

const MODEL_LIBRARY: &str = include_str!("fixtures/model-pack/library.toml");
const PER_MODEL_LIMIT: u64 = 64 * 1024 * 1024;

fn crc32(data: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;
    for &byte in data {
        crc ^= u32::from(byte);
        for _ in 0..8 {
            crc = if crc & 1 != 0 { (crc >> 1) ^ 0xEDB8_8320 } else { crc >> 1 };
        }
    }
    !crc
}

fn adler32(data: &[u8]) -> u32 {
    let (mut low, mut high) = (1u32, 0u32);
    for &byte in data {
        low = (low + u32::from(byte)) % 65521;
        high = (high + low) % 65521;
    }
    (high << 16) | low
}

fn png_chunk(out: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
    out.extend_from_slice(&(data.len() as u32).to_be_bytes());
    let mut body = kind.to_vec();
    body.extend_from_slice(data);
    out.extend_from_slice(&body);
    out.extend_from_slice(&crc32(&body).to_be_bytes());
}

/// A valid RGBA PNG of `width` x `height` with stored (uncompressed) zlib blocks, so its size is predictable.
fn png_bytes(width: u32, height: u32) -> Vec<u8> {
    let mut raw = Vec::new();
    for row in 0..height {
        raw.push(0);
        for column in 0..width {
            raw.extend_from_slice(&[(row % 6 * 40) as u8, (column % 6 * 40) as u8, 128, 255]);
        }
    }
    let mut zlib = vec![0x78, 0x01];
    let mut blocks = raw.chunks(65535).peekable();
    while let Some(block) = blocks.next() {
        zlib.push(u8::from(blocks.peek().is_none()));
        zlib.extend_from_slice(&(block.len() as u16).to_le_bytes());
        zlib.extend_from_slice(&(!(block.len() as u16)).to_le_bytes());
        zlib.extend_from_slice(block);
    }
    zlib.extend_from_slice(&adler32(&raw).to_be_bytes());
    let mut header = Vec::new();
    header.extend_from_slice(&width.to_be_bytes());
    header.extend_from_slice(&height.to_be_bytes());
    header.extend_from_slice(&[8, 6, 0, 0, 0]);
    let mut png = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
    png_chunk(&mut png, b"IHDR", &header);
    png_chunk(&mut png, b"IDAT", &zlib);
    png_chunk(&mut png, b"IEND", &[]);
    png
}

/// A structurally valid GLB whose BIN chunk is `bin_len` pseudo-random bytes (incompressible, so member data dominates the zip).
fn glb_bytes(seed: u32, bin_len: usize) -> Vec<u8> {
    let mut json = br#"{"asset":{"version":"2.0"},"meshes":[{}],"nodes":[{}]}"#.to_vec();
    while json.len() % 4 != 0 {
        json.push(b' ');
    }
    let mut state = seed.wrapping_mul(2_654_435_761).max(1);
    let bin: Vec<u8> = (0..bin_len)
        .map(|_| {
            state ^= state << 13;
            state ^= state >> 17;
            state ^= state << 5;
            state as u8
        })
        .collect();
    let total = 12 + 8 + json.len() + 8 + bin.len();
    let mut out = b"glTF".to_vec();
    out.extend_from_slice(&2u32.to_le_bytes());
    out.extend_from_slice(&(total as u32).to_le_bytes());
    out.extend_from_slice(&(json.len() as u32).to_le_bytes());
    out.extend_from_slice(b"JSON");
    out.extend_from_slice(&json);
    out.extend_from_slice(&(bin.len() as u32).to_le_bytes());
    out.extend_from_slice(b"BIN\0");
    out.extend_from_slice(&bin);
    out
}

/// A two-model pack folder: `library.toml`, `chair.glb`/`table.glb` and their thumbnails.
fn model_folder(dir: &Path, library: &str) {
    std::fs::create_dir_all(dir).unwrap();
    std::fs::write(dir.join("library.toml"), library).unwrap();
    for (name, seed) in [("chair", 1u32), ("table", 2)] {
        std::fs::write(dir.join(format!("{name}.glb")), glb_bytes(seed, 48 * 1024)).unwrap();
        std::fs::write(dir.join(format!("{name}.png")), png_bytes(4, 4)).unwrap();
    }
}

/// Create a symlink; false when the platform or the account may not (Windows needs a privilege).
fn make_symlink(target: &Path, link: &Path) -> bool {
    #[cfg(unix)]
    let made = std::os::unix::fs::symlink(target, link);
    #[cfg(windows)]
    let made = std::os::windows::fs::symlink_file(target, link);
    #[cfg(not(any(unix, windows)))]
    let made: std::io::Result<()> = Err(std::io::ErrorKind::Unsupported.into());
    made.is_ok()
}

/// Model pack leg of J8: pack, inspect, tamper, then one table of refusals that each write nothing.
fn model_leg(root: &Path, key: &Path) {
    let key_arg = key.to_str().unwrap();
    let folder = root.join("model-pack");
    model_folder(&folder, MODEL_LIBRARY);
    let out_dir = root.join("model-out");
    let out_arg = format!("{}/", out_dir.display());
    let packed = fe_ext(&["model", "pack", folder.to_str().unwrap(), "--key", key_arg, "--out", &out_arg], root);
    assert!(packed.status.success(), "model pack: {}", stderr(&packed));
    let line = |name: &str| stdout(&packed).lines().find_map(|line| line.strip_prefix(&format!("{name} ")).map(str::to_string));
    let package = out_dir.join("acme.furniture-1.0.0.hexon");
    assert!(package.is_file(), "pack names <id>-<version>.hexon: {}", stdout(&packed));
    assert!(!stdout(&packed).contains(&std::fs::read_to_string(key).unwrap().trim().to_string()), "the key is never printed");

    let inspected = fe_ext(&["inspect", package.to_str().unwrap(), "--json"], root);
    assert!(inspected.status.success(), "inspect model: {}", stderr(&inspected));
    let facts: Value = serde_json::from_slice(&inspected.stdout).unwrap();
    assert_eq!((facts["package_id"].as_str(), facts["kind"].as_str()), (Some("acme.furniture"), Some("model")));
    assert_eq!(facts["models"], serde_json::json!(["chair", "table"]));
    assert_eq!((facts["license"].as_str(), facts["spdx"].as_str()), (Some("attribution"), Some("CC-BY-4.0")));
    assert_eq!(facts["content_hash"].as_str(), line("content_hash").as_deref(), "inspect reads the packed build");
    assert_eq!(facts["publisher_did"].as_str(), line("publisher_did").as_deref());

    let mut tampered = std::fs::read(&package).unwrap();
    let middle = tampered.len() / 2;
    tampered[middle] ^= 0xff;
    let tampered_path = root.join("tampered-model.hexon");
    std::fs::write(&tampered_path, tampered).unwrap();
    assert!(!fe_ext(&["inspect", tampered_path.to_str().unwrap()], root).status.success(), "a flipped byte is refused");

    // Refusals: each names its own reason and leaves no output file, no output folder and no temp file behind.
    type Edit = fn(&Path) -> bool;
    let refusals: [(&str, &str, Edit); 9] = [
        ("unlisted-glb", "is not listed in library.toml", |dir| {
            std::fs::write(dir.join("spare.glb"), glb_bytes(3, 64)).unwrap();
            true
        }),
        ("symlink", "is a symlink", |dir| make_symlink(&dir.join("chair.glb"), &dir.join("spare.glb"))),
        ("bad-glb", "is not a valid GLB", |dir| {
            std::fs::write(dir.join("chair.glb"), b"this is not a glb").unwrap();
            true
        }),
        ("oversize", "exceeds the 64 MiB", |dir| {
            let file = std::fs::OpenOptions::new().write(true).open(dir.join("chair.glb")).unwrap();
            file.set_len(PER_MODEL_LIMIT + 1).unwrap();
            true
        }),
        ("missing-licence", "no [license] table", |dir| {
            let (start, end) = (MODEL_LIBRARY.find("[license]").unwrap(), MODEL_LIBRARY.find("[[model]]").unwrap());
            std::fs::write(dir.join("library.toml"), format!("{}{}", &MODEL_LIBRARY[..start], &MODEL_LIBRARY[end..])).unwrap();
            true
        }),
        ("empty-attribution", "attribution is required", |dir| {
            let edited = MODEL_LIBRARY.replace("attribution = \"Chair and table by Acme Models, CC BY 4.0\"", "attribution = \"  \"");
            std::fs::write(dir.join("library.toml"), edited).unwrap();
            true
        }),
        // Zero-width, joiner, isolate and BOM characters only: shows as nothing, so it is no attribution.
        ("invisible-attribution", "attribution is required", |dir| {
            let edited = MODEL_LIBRARY.replace(
                "attribution = \"Chair and table by Acme Models, CC BY 4.0\"",
                "attribution = \"\\u200B\\u200D\\u2066\\uFEFF\"",
            );
            std::fs::write(dir.join("library.toml"), edited).unwrap();
            true
        }),
        ("oversize-thumbnail", "larger than", |dir| {
            std::fs::write(dir.join("chair.png"), png_bytes(300, 300)).unwrap();
            true
        }),
        ("non-png-thumbnail", "not a PNG", |dir| {
            std::fs::write(dir.join("chair.png"), b"GIF89a not a png").unwrap();
            true
        }),
    ];
    let mut refused_reasons = std::collections::BTreeSet::new();
    for (name, reason, edit) in refusals {
        let case = root.join(format!("refuse-{name}"));
        model_folder(&case, MODEL_LIBRARY);
        if !edit(&case) {
            eprintln!("skipping the {name} row: this platform cannot create it here");
            continue;
        }
        let case_out = root.join(format!("refuse-out-{name}"));
        let case_out_arg = format!("{}/", case_out.display());
        let refused = fe_ext(&["model", "pack", case.to_str().unwrap(), "--key", key_arg, "--out", &case_out_arg], root);
        assert!(!refused.status.success(), "{name} should be refused");
        assert!(stderr(&refused).contains(reason), "{name}: {}", stderr(&refused));
        assert!(!case_out.exists(), "{name}: a refused pack writes nothing");
        refused_reasons.insert(reason);
    }
    assert!(refused_reasons.len() >= 7, "each refusal carries its own reason: {refused_reasons:?}");
}

/// Every byte as a WAT `\hh` escape, so data segments need no quoting rules.
fn wat_bytes(text: &str) -> String {
    text.bytes().map(|byte| format!("\\{byte:02x}")).collect()
}

const PONG: &str = r#"{"pong":true}"#;
const ALLOW: &str = r#"{"verdict":"allow","augment":[{"key":"sbx.judged","value":true}]}"#;
const REFUSE: &str = r#"{"verdict":"refuse","reason":"node is locked"}"#;
const STREAM: &str = "beats";
const BEAT: &str = r#"{"beat":1}"#;

/// The sandbox-calls guest (world `extension-with-hooks`, hand-written WAT as fe-plugin's test guests): `on-tick`
/// publishes `{"beat":1}` on `beats`, `on-command` answers `{"pong":true}`, and `on-hook` refuses ("node is locked")
/// when the input contains `~`, else allows with the augment `sbx.judged = true`.
fn sandbox_calls_component() -> Vec<u8> {
    let wat = format!(
        r#"(module
  (import "fractalengine:plugin/bus@2.0.0" "publish" (func $publish (param i32 i32 i32 i32 i32)))
  (memory (export "memory") 1)
  (global $heap (mut i32) (i32.const 4096))
  (data (i32.const 64) "{pong}")
  (data (i32.const 128) "{allow}")
  (data (i32.const 256) "{refuse}")
  (data (i32.const 352) "{stream}")
  (data (i32.const 368) "{beat}")
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
    (param i64 i64 i64 i32 i32 i32 i64 i32 i32 i32 i32)
    (call $publish (i32.const 352) (i32.const {stream_len}) (i32.const 368) (i32.const {beat_len}) (i32.const 1664)))
  (func (export "fractalengine:plugin/ingress-hooks@2.0.0#on-ingress") (param i32) (result i32) (i32.const 1024))
  (func (export "fractalengine:plugin/ingress-hooks@2.0.0#on-connection") (param i32 i32 i64 i32))
  (func (export "fractalengine:plugin/egress-hooks@2.0.0#on-egress-result") (param i64 i32 i32 i32 i32 i32 i32 i32 i32 i32))
  (func (export "fractalengine:plugin/bus-hooks@2.0.0#on-events") (param i32 i32))
  (func (export "fractalengine:plugin/bus-hooks@2.0.0#on-command") (param i32 i32 i32 i32 i32 i32 i32 i32 i32 i32 i32) (result i32)
    (i32.store8 (i32.const 1600) (i32.const 0))
    (i32.store (i32.const 1604) (i32.const 64))
    (i32.store (i32.const 1608) (i32.const {pong_len}))
    (i32.const 1600))
  (func (export "fractalengine:plugin/bus-hooks@2.0.0#on-call-result") (param i64 i32 i32 i32 i32))
  (func (export "fractalengine:plugin/hooks@2.0.0#on-hook")
    (param i32 i32 i32 i32 i32 i32 i32 i32) (param $input i32) (param $input_len i32) (param i32 i32) (result i32)
    (local $at i32) (local $locked i32)
    (block $done
      (loop $scan
        (br_if $done (i32.ge_u (local.get $at) (local.get $input_len)))
        (if (i32.eq (i32.load8_u (i32.add (local.get $input) (local.get $at))) (i32.const 126))
          (then (local.set $locked (i32.const 1)) (br $done)))
        (local.set $at (i32.add (local.get $at) (i32.const 1)))
        (br $scan)))
    (i32.store8 (i32.const 1536) (i32.const 0))
    (if (local.get $locked)
      (then
        (i32.store (i32.const 1540) (i32.const 256))
        (i32.store (i32.const 1544) (i32.const {refuse_len})))
      (else
        (i32.store (i32.const 1540) (i32.const 128))
        (i32.store (i32.const 1544) (i32.const {allow_len}))))
    (i32.store (i32.const 1548) (i32.const 0))
    (i32.store (i32.const 1552) (i32.const 0))
    (i32.const 1536))
)"#,
        pong = wat_bytes(PONG),
        allow = wat_bytes(ALLOW),
        refuse = wat_bytes(REFUSE),
        stream = wat_bytes(STREAM),
        beat = wat_bytes(BEAT),
        pong_len = PONG.len(),
        allow_len = ALLOW.len(),
        refuse_len = REFUSE.len(),
        stream_len = STREAM.len(),
        beat_len = BEAT.len(),
    );
    let mut module = wat::parse_str(&wat).expect("sandbox-calls WAT parses");
    let mut resolve = wit_parser::Resolve::default();
    let package = resolve
        .push_file(concat!(env!("CARGO_MANIFEST_DIR"), "/../../fe-plugin/wit/fractal-plugin-2.wit"))
        .expect("the 2.0 WIT parses");
    let world = resolve.select_world(&[package], Some("extension-with-hooks")).expect("the hooks world exists");
    wit_component::embed_component_metadata(&mut module, &resolve, world, wit_component::StringEncoding::UTF8).expect("metadata embeds");
    wit_component::ComponentEncoder::default().module(&module).expect("module matches the world").validate(true).encode().expect("component encodes")
}

/// Sandbox calls leg of J8: bus commands, a scene.transform hook and stream capture through the real bus and gate,
/// fuel per guest call, two byte-identical traces; then the fixture refusals an author meets.
fn sandbox_calls_leg(root: &Path) {
    let project = root.join("sandbox-calls");
    std::fs::create_dir_all(project.join("dist")).unwrap();
    let manifest = serde_json::json!({
        "manifest_version": 2,
        "id": "local.sandbox-calls",
        "version": "0.1.0",
        "name": "Sandbox calls",
        "kind": "general",
        "contract": "fractalengine:plugin@2.0.0",
        "binding": "petal",
        "scene": { "read": true, "write": ["set_property"], "property_scope": ["sbx.*"] },
        "tick": { "cadence_ms": 1000, "max_ops": 16, "max_readings": 16, "fuel": 200000 },
        "bus": { "expose": {
            "commands": [{ "id": "ping", "role_floor": "editor", "rate_hz": 5, "args_schema": {
                "type": "object", "properties": { "n": { "type": "integer" } }, "required": ["n"], "additionalProperties": false } }],
            "streams": [{ "id": "beats", "max_rate_hz": 1, "max_event_bytes": 64 }]
        } },
        "provides": [{ "hook": "scene.transform" }]
    });
    std::fs::write(project.join("extension.json"), serde_json::to_vec_pretty(&manifest).unwrap()).unwrap();
    std::fs::write(project.join("dist/component.wasm"), sandbox_calls_component()).unwrap();
    let pose = serde_json::json!({ "position": [1.0, 2.0, 3.0], "rotation": [0.0, 0.1, 0.0], "scale": [1.0, 1.0, 1.0] });
    let fixture = serde_json::json!({
        "fixture_version": 2,
        "petal_id": "p1",
        "start_ms": 1_000_000,
        "ticks": 3,
        "cadence_ms": 500,
        "nodes": [{ "id": "n1" }, { "id": "locked~1" }],
        "commands": [
            { "at_tick": 1, "command": "ping", "args": { "n": 1 }, "role": "editor" },
            { "at_tick": 1, "command": "ping", "args": { "n": "one" } },
            { "at_tick": 2, "command": "ping", "args": { "n": 2 }, "role": "viewer" }
        ],
        "hooks": [
            { "at_tick": 2, "hook": "scene.transform", "input": { "node": "n1", "proposed": pose.clone() } },
            { "after_tick": 2, "hook": "scene.transform", "input": { "node": "locked~1", "proposed": pose } }
        ]
    });
    let fixture_path = project.join("fixture.json");
    std::fs::write(&fixture_path, serde_json::to_vec_pretty(&fixture).unwrap()).unwrap();
    let (project_arg, fixture_arg) = (project.to_str().unwrap(), fixture_path.to_str().unwrap());
    let traces = [root.join("sandbox-calls-1.json"), root.join("sandbox-calls-2.json")];
    let runs: Vec<Output> = traces
        .iter()
        .map(|trace| fe_ext(&["run", "--sandbox", "--project", project_arg, "--fixture", fixture_arg, "--out", trace.to_str().unwrap()], root))
        .collect();
    assert!(runs[0].status.success(), "sandbox-calls run: {}", stderr(&runs[0]));
    assert_eq!(fingerprint(&runs[0]), fingerprint(&runs[1]), "commands, hooks and streams keep a run deterministic");
    assert_eq!(std::fs::read(&traces[0]).unwrap(), std::fs::read(&traces[1]).unwrap());
    let trace: Value = serde_json::from_slice(&std::fs::read(&traces[0]).unwrap()).unwrap();

    // Commands: the guest's answer, then the bus's own refusals (schema = invalid-input 400, role floor = 403).
    let commands = trace["commands"].as_array().expect("commands");
    assert_eq!(commands[0]["reply"]["ok"], serde_json::json!({ "pong": true }), "{}", commands[0]);
    assert_eq!((commands[1]["host_refused"]["code"].as_str(), commands[1]["host_refused"]["status"].as_u64()), (Some("invalid-input"), Some(400)));
    assert_eq!((commands[2]["host_refused"]["code"].as_str(), commands[2]["host_refused"]["status"].as_u64()), (Some("capability-denied"), Some(403)));

    // Hooks: the gate's input shape (Euler radians through f32, caller), an allow whose augment staged, a refusal's reason.
    let hooks = trace["hooks"].as_array().expect("hooks");
    assert_eq!(hooks[0]["input"]["proposed"]["rotation"][1].as_f64(), Some(f64::from(0.1f32)), "rotation travels as the f32 the DB stores");
    assert_eq!(hooks[0]["input"]["caller"], serde_json::json!({ "did": "did:key:sandbox", "kind": "ui", "local": true }));
    assert_eq!((hooks[0]["verdict"]["allow"].as_bool(), hooks[0]["verdict"]["staged"].as_bool()), (Some(true), Some(true)), "{}", hooks[0]);
    assert_eq!((hooks[1]["verdict"]["refused"].as_str(), hooks[1]["verdict"]["reason"].as_str()), (Some("refused"), Some("node is locked")));
    assert_eq!(node(&trace, "n1")["properties"]["sbx.judged"], serde_json::json!(true), "the augment flushed with tick 2");

    // Streams: 1 Hz on a 500 ms cadence publishes, is rate-limited by the bus, then publishes again.
    let streams: Vec<(Option<&str>, bool)> = trace["streams"]
        .as_array()
        .expect("streams")
        .iter()
        .map(|event| (event["refused"]["code"].as_str(), event["published"].as_bool() == Some(true)))
        .collect();
    assert_eq!(streams, vec![(None, true), (Some("rate-limited"), false), (None, true)]);
    assert_eq!(trace["streams"][0]["payload"], serde_json::json!({ "beat": 1 }));

    // Fuel: every guest call records its limit and what it burned.
    let fuel = trace["fuel"]["calls"].as_array().expect("fuel calls");
    assert_eq!(trace["fuel"]["tick_limit"].as_u64(), Some(200_000));
    for call in ["on-tick", "on-command ping", "hook scene.transform"] {
        let entry = fuel.iter().find(|entry| entry["call"] == call).unwrap_or_else(|| panic!("fuel for {call}: {}", trace["fuel"]));
        assert!(entry["consumed"].as_u64().is_some_and(|burned| burned > 0 && burned <= entry["limit"].as_u64().unwrap()), "{entry}");
    }

    // Fixture refusals: a hook the manifest does not provide, and what an older fe-ext says about a newer fixture.
    type Edit = fn(&mut Value);
    let refusals: [(&str, Edit); 3] = [
        ("does not provide scene.property", |f| f["hooks"][0]["hook"] = serde_json::json!("scene.property")),
        ("needs fe-ext >= fixture_version 99", |f| f["fixture_version"] = serde_json::json!(99)),
        ("needs a newer fe-ext", |f| f["replay_from"] = serde_json::json!("trace.json")),
    ];
    for (expected, edit) in refusals {
        let mut edited = fixture.clone();
        edit(&mut edited);
        std::fs::write(&fixture_path, serde_json::to_vec(&edited).unwrap()).unwrap();
        let refused = fe_ext(&["run", "--sandbox", "--project", project_arg, "--fixture", fixture_arg], root);
        assert!(!refused.status.success() && stderr(&refused).contains(expected), "{expected}: {}", stderr(&refused));
    }
}

#[test]
fn j8_new_validate_pack_inspect_countersign_and_sandbox() {
    let temp = tempfile::tempdir().expect("temp dir");
    let root = temp.path();

    // new: every kind scaffolds a manifest that passes the validation table.
    for (kind, name) in [("simulation", "sim-one"), ("device-adapter", "dev-one"), ("bi-connector", "bi-one"), ("general", "gen-one")] {
        let created = fe_ext(&["new", kind, name], root);
        assert!(created.status.success(), "new {kind}: {}", stderr(&created));
        let validated = fe_ext(&["validate", "--project", name], root);
        assert!(validated.status.success(), "validate {kind}: {}", stderr(&validated));
        assert!(stdout(&validated).starts_with("ok local."), "{}", stdout(&validated));
        let scaffold: Value = serde_json::from_slice(&std::fs::read(root.join(name).join("fixtures/basic.json")).unwrap()).unwrap();
        assert_eq!(scaffold["fixture_version"].as_u64(), Some(u64::from(fe_ext::sandbox::FIXTURE_VERSION)), "new stamps the fixture format");
    }

    // validate: distinct reasons from the table, non-zero exit.
    let project = root.join("gen-one");
    let manifest_path = project.join("extension.json");
    let good: serde_json::Value = serde_json::from_slice(&std::fs::read(&manifest_path).unwrap()).unwrap();
    type Edit = fn(&mut serde_json::Value);
    let refusals: [(&str, Edit); 3] = [
        ("empty-scope", |m| m["scene"]["property_scope"] = serde_json::json!([])),
        ("cadence-below-floor", |m| m["tick"]["cadence_ms"] = serde_json::json!(1)),
        ("unsupported-contract", |m| m["contract"] = serde_json::json!("fractalengine:plugin@1.0.0")),
    ];
    let mut reasons = std::collections::BTreeSet::new();
    for (expected, edit) in refusals {
        let mut bad = good.clone();
        edit(&mut bad);
        std::fs::write(&manifest_path, serde_json::to_vec(&bad).unwrap()).unwrap();
        let refused = fe_ext(&["validate", "--project", "gen-one"], root);
        assert!(!refused.status.success(), "{expected} should be refused");
        assert!(stderr(&refused).contains(expected), "{expected}: {}", stderr(&refused));
        reasons.insert(expected);
    }
    assert_eq!(reasons.len(), 3, "each refusal carries its own reason");
    std::fs::write(&manifest_path, serde_json::to_vec_pretty(&good).unwrap()).unwrap();

    let key = root.join("publisher.key");
    let counter_key = root.join("reviewer.key");
    assert!(fe_ext(&["keygen", key.to_str().unwrap()], root).status.success());
    let reviewer = fe_ext(&["keygen", counter_key.to_str().unwrap()], root);
    assert!(reviewer.status.success());
    let reviewer_did = stdout(&reviewer).trim().to_string();

    // The shipped examples (R27), each once `fe-ext build --project extensions/<name>` has left dist/component.wasm:
    // two sandbox runs print one fingerprint and write identical traces, the trace shows the example's outcomes,
    // and the example packs into a sealed package that inspects under its own id.
    let examples = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../extensions");
    let legs: [(&str, &str, fn(&Value)); 3] = [
        ("fleet-sim", "org.example.fleet-sim", fleet_sim_outcomes),
        ("device-adapter", "org.example.device-adapter", device_adapter_outcomes),
        ("bi-push", "org.example.bi-push", bi_push_outcomes),
    ];
    let packages = root.join("packages");
    std::fs::create_dir_all(&packages).unwrap();
    let mut packed_ids = Vec::new();
    for (example, package_id, outcomes) in legs {
        let example_dir = examples.join(example);
        if !example_dir.join("dist/component.wasm").is_file() {
            continue;
        }
        let (example_path, fixture) = (example_dir.to_str().unwrap(), example_dir.join("fixtures/basic.json"));
        let traces = [root.join(format!("{example}-1.json")), root.join(format!("{example}-2.json"))];
        let runs: Vec<Output> = traces
            .iter()
            .map(|trace| {
                fe_ext(&["run", "--sandbox", "--project", example_path, "--fixture", fixture.to_str().unwrap(), "--out", trace.to_str().unwrap()], root)
            })
            .collect();
        assert!(runs[0].status.success(), "{example} run: {}", stderr(&runs[0]));
        assert!(fingerprint(&runs[0]).is_some());
        assert_eq!(fingerprint(&runs[0]), fingerprint(&runs[1]), "{example}: a sandbox run is a pure function of its inputs");
        assert_eq!(std::fs::read(&traces[0]).unwrap(), std::fs::read(&traces[1]).unwrap());
        outcomes(&serde_json::from_slice(&std::fs::read(&traces[0]).unwrap()).unwrap());

        // validate with the built component runs check_provides (bi-push declares `provides`, so it exports `hooks`).
        let validated = fe_ext(&["validate", "--project", example_path], root);
        assert!(validated.status.success(), "{example} validate: {}", stderr(&validated));
        assert!(stdout(&validated).contains("component ok"), "{example}: the built component was checked: {}", stdout(&validated));

        let package = root.join(format!("{example}.hexon"));
        let packed = fe_ext(&["pack", "--project", example_path, "--key", key.to_str().unwrap(), "--out", package.to_str().unwrap()], root);
        assert!(packed.status.success(), "{example} pack: {}", stderr(&packed));
        let inspected = fe_ext(&["inspect", package.to_str().unwrap(), "--json"], root);
        assert!(inspected.status.success(), "{example} inspect: {}", stderr(&inspected));
        let facts: Value = serde_json::from_slice(&inspected.stdout).unwrap();
        assert_eq!(facts["package_id"], package_id);
        // The sealed package is the one pack announced (same content hash on both sides of the CLI).
        let announced = stdout(&packed).lines().find_map(|line| line.strip_prefix("content_hash ").map(str::to_string));
        assert_eq!(facts["content_hash"].as_str(), announced.as_deref(), "{example}: inspect reads the packed build");

        // sign: re-sealing under the reviewer key moves the content hash; the original stays the one that is indexed.
        let resigned = root.join(format!("{example}-resigned.hexon"));
        let signed = fe_ext(&["sign", package.to_str().unwrap(), "--key", counter_key.to_str().unwrap(), "--out", resigned.to_str().unwrap()], root);
        assert!(signed.status.success(), "{example} sign: {}", stderr(&signed));
        let resigned_facts: Value = serde_json::from_slice(&fe_ext(&["inspect", resigned.to_str().unwrap(), "--json"], root).stdout).unwrap();
        assert_eq!(resigned_facts["package_id"], package_id);
        assert_ne!(resigned_facts["content_hash"], facts["content_hash"], "{example}: a new publisher key is a new seal");

        std::fs::copy(&package, packages.join(format!("{example}.hexon"))).unwrap();
        packed_ids.push(package_id);
    }

    // A `provides` manifest over a component without the `hooks` export is refused by validate and by pack.
    let (bi_push, fleet_component) = (examples.join("bi-push"), examples.join("fleet-sim/dist/component.wasm"));
    if bi_push.join("dist/component.wasm").is_file() && fleet_component.is_file() {
        let (project_arg, component_arg) = (bi_push.to_str().unwrap(), fleet_component.to_str().unwrap());
        let refused = fe_ext(&["validate", "--project", project_arg, "--component", component_arg], root);
        assert!(!refused.status.success() && stderr(&refused).contains("missing-export"), "validate: {}", stderr(&refused));
        let unsealed = root.join("no-hooks.hexon");
        let pack_args = ["pack", "--project", project_arg, "--component", component_arg, "--key", key.to_str().unwrap(), "--out", unsealed.to_str().unwrap()];
        let refused = fe_ext(&pack_args, root);
        assert!(!refused.status.success() && stderr(&refused).contains("missing-export"), "pack: {}", stderr(&refused));
        assert!(!unsealed.exists(), "a refused pack writes nothing");
    }

    // index build + verify: every packed example is listed, and StaticIndexSource re-verifies each one end to end
    // from the written tree against the pinned index key (a different key from every publisher).
    if !packed_ids.is_empty() {
        let index_key = root.join("index.key");
        let keygen = fe_ext(&["keygen", index_key.to_str().unwrap()], root);
        assert!(keygen.status.success(), "index keygen: {}", stderr(&keygen));
        let index_did = stdout(&keygen).trim().to_string();
        let tree = root.join("catalog-tree");
        let built = fe_ext(
            &["index", "build", "--packages", packages.to_str().unwrap(), "--base-url", "https://store.example.test", "--key", index_key.to_str().unwrap(), "--out", tree.to_str().unwrap()],
            root,
        );
        assert!(built.status.success(), "index build: {}", stderr(&built));
        assert!(stdout(&built).contains(&format!("index: {} package(s)", packed_ids.len())), "{}", stdout(&built));
        let verified = fe_ext(&["index", "verify", tree.to_str().unwrap(), "--index-key", &index_did], root);
        assert!(verified.status.success(), "index verify: {}{}", stdout(&verified), stderr(&verified));
        for package_id in &packed_ids {
            assert!(stdout(&verified).contains(*package_id), "verify names {package_id}: {}", stdout(&verified));
        }
        let wrong_pin = stdout(&fe_ext(&["keygen", root.join("other.key").to_str().unwrap()], root)).trim().to_string();
        assert!(!fe_ext(&["index", "verify", tree.to_str().unwrap(), "--index-key", &wrong_pin], root).status.success(), "a foreign index key is refused");
    }

    model_leg(root, &key);
    sandbox_calls_leg(root);

    // The scaffold's packaging half needs a built component: FE_EXT_TEST_COMPONENT points at the compiled
    // generated project (`fe-ext build` output). Without it the journey ends here.
    let Ok(component) = std::env::var("FE_EXT_TEST_COMPONENT") else { return };
    std::fs::create_dir_all(project.join("dist")).unwrap();
    std::fs::copy(&component, project.join("dist/component.wasm")).unwrap();

    let packed = fe_ext(&["pack", "--project", "gen-one", "--key", key.to_str().unwrap()], root);
    assert!(packed.status.success(), "pack: {}", stderr(&packed));
    let package = project.join("dist/local.gen-one-0.1.0.hexon");
    assert!(package.is_file());

    let before = fe_ext(&["inspect", package.to_str().unwrap(), "--json"], root);
    assert!(before.status.success(), "inspect: {}", stderr(&before));
    let facts: serde_json::Value = serde_json::from_slice(&before.stdout).unwrap();

    // countersign leaves the content hash alone.
    let countersigned = fe_ext(&["countersign", package.to_str().unwrap(), "--key", counter_key.to_str().unwrap()], root);
    assert!(countersigned.status.success(), "countersign: {}", stderr(&countersigned));
    let after: serde_json::Value = serde_json::from_slice(&fe_ext(&["inspect", package.to_str().unwrap(), "--json"], root).stdout).unwrap();
    assert_eq!(facts["content_hash"], after["content_hash"]);
    assert_eq!(after["countersigners"].as_str(), Some(reviewer_did.as_str()), "inspect names the counter-signer");

    // A flipped component byte is refused by inspect with a typed reason.
    let mut tampered = std::fs::read(&package).unwrap();
    let flipped = tampered.len() / 2;
    tampered[flipped] ^= 0xff;
    let tampered_path = root.join("tampered.hexon");
    std::fs::write(&tampered_path, tampered).unwrap();
    assert!(!fe_ext(&["inspect", tampered_path.to_str().unwrap()], root).status.success());

    // run --sandbox: two executions print the same fingerprint.
    let fixture = project.join("fixtures/basic.json");
    let args = ["run", "--sandbox", "--project", "gen-one", "--fixture", fixture.to_str().unwrap()];
    let first = fe_ext(&args, root);
    let second = fe_ext(&args, root);
    assert!(first.status.success(), "run: {}", stderr(&first));
    let fingerprint = |output: &Output| stdout(output).lines().find(|line| line.starts_with("fingerprint ")).map(str::to_string);
    assert!(fingerprint(&first).is_some());
    assert_eq!(fingerprint(&first), fingerprint(&second));
}
