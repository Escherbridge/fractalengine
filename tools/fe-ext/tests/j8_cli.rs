//! J8: one authoring journey through the real `fe-ext` binary, from `new` to a counter-signed package,
//! plus the two shipped examples (R27) through `run --sandbox`, `pack` and `inspect`.
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
    }

    // validate: distinct reasons from the table, non-zero exit.
    let project = root.join("gen-one");
    let manifest_path = project.join("extension.json");
    let good: serde_json::Value = serde_json::from_slice(&std::fs::read(&manifest_path).unwrap()).unwrap();
    let refusals = [
        ("empty-scope", |m: &mut serde_json::Value| m["scene"]["property_scope"] = serde_json::json!([])),
        ("cadence-below-floor", |m: &mut serde_json::Value| m["tick"]["cadence_ms"] = serde_json::json!(1)),
        ("unsupported-contract", |m: &mut serde_json::Value| m["contract"] = serde_json::json!("fractalengine:plugin@1.0.0")),
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
