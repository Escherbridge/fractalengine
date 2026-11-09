//! Journey J4 (docs/extensions/bus.md §10) through the public `Bus` seam with fake host sources.

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use crossbeam::channel::Sender;
use fe_policy::RoleLevel;
use fe_runtime::messages::SceneChange;
use fe_runtime::plugin_host::{PluginReply, PluginResult};
use fe_sdk::manifest_v2::{ConsumeEntry, ExtensionManifest};
use fe_sdk::plugin_host::{PendingOp, TransactionReceipt};
use serde_json::{json, Value};

use super::*;
use crate::tick::{Clock, ManualClock};

const LOCAL: &str = "did:key:z6MkLocalHost";
const PETAL: &str = "01JPETAL";
const FLEET: &str = "org.example.fleet-sim";
const CONSOLE: &str = "org.example.console";

/// A petal holding one node more than REST's row cap; each node carries an in-scope and an out-of-scope key.
struct OversizedPetal;

impl SceneSnapshotSource for OversizedPetal {
    fn snapshot(&self, petal_id: &str, reply: Sender<Result<Vec<Value>, HostError>>) {
        let nodes = (0..=REST_ROW_CAP)
            .map(|i| json!({ "node_id": format!("n{i}"), "petal_id": petal_id, "properties": { "fleet.speed": i, "secret": "hidden" } }))
            .collect();
        let _ = reply.send(Ok(nodes));
    }
}

/// The write path stand-in: records each write and commits it, or holds the reply while `hold` is set.
#[derive(Default)]
struct Writes {
    seen: Mutex<Vec<PortalWrite>>,
    held: Mutex<Vec<Sender<PluginResult>>>,
    hold: AtomicBool,
}

impl PortalWriteSink for Writes {
    fn commit(&self, write: &PortalWrite, reply: Sender<PluginResult>) {
        self.seen.lock().unwrap().push(write.clone());
        if self.hold.load(Ordering::SeqCst) {
            self.held.lock().unwrap().push(reply);
        } else {
            let _ = reply.send(Ok(committed()));
        }
    }
}

fn committed() -> PluginReply {
    PluginReply::Committed(serde_json::from_value::<TransactionReceipt>(json!({ "op_count": 1, "created_node_ids": [] })).unwrap())
}

/// The console bundle's host-minted binding at `PETAL` (ipc-v1.md §6.5).
fn console_grants() -> PortalGrants {
    PortalGrants {
        package_id: CONSOLE.into(),
        content_hash: "cd".repeat(32),
        installation_id: Some("inst-console".into()),
        verse_id: "01JVERSE".into(),
        petal_id: PETAL.into(),
        consume: serde_json::from_value::<Vec<ConsumeEntry>>(json!([{ "source": "host/node-changes" }, { "source": format!("{FLEET}/positions") }])).unwrap(),
        property_scope: Some(vec!["fleet.*".into()]),
        streams: vec![PortalStream { id: "selection".into(), max_rate_hz: 4.0, max_event_bytes: 2048, consumers: None }],
        invoke: vec![format!("{FLEET}/start")],
        set_property_keys: vec!["fleet.*".into(), "console.note".into()],
    }
}

/// The guarded core stand-in: FROM target = the word after FROM; every run returns `rows`.
struct FakeQuery {
    rows: Mutex<Vec<Value>>,
    runs: AtomicUsize,
    requests: Mutex<Vec<QueryRequest>>,
}

impl QuerySource for FakeQuery {
    fn prepare(&self, request: &QueryRequest) -> Result<PreparedQuery, HostError> {
        let mut words = request.sql.split_whitespace().skip_while(|word| !word.eq_ignore_ascii_case("from"));
        let table = words.nth(1).ok_or_else(|| HostError::InvalidInput("no FROM".into()))?;
        Ok(PreparedQuery { tables: vec![table.to_ascii_uppercase()], guarded_sql: request.sql.clone() })
    }

    fn run(&self, request: &QueryRequest, _prepared: &PreparedQuery, reply: Sender<Result<QueryRows, HostError>>) {
        self.runs.fetch_add(1, Ordering::SeqCst);
        self.requests.lock().unwrap().push(request.clone());
        let _ = reply.send(Ok(QueryRows { rows: self.rows.lock().unwrap().clone(), truncated: false }));
    }

    fn try_acquire_rate(&self, _identity_did: &str) -> bool {
        true
    }
}

struct Roles;

impl RoleSource for Roles {
    fn role_at(&self, did: &str, _petal_id: &str) -> RoleLevel {
        match did {
            LOCAL => RoleLevel::Owner,
            "did:key:z6MkRemoteEditor" => RoleLevel::Editor,
            _ => RoleLevel::None,
        }
    }
}

struct OpenPolicy;

impl BusPolicy for OpenPolicy {
    fn external_access_allowed(&self, _verse_id: &str) -> bool {
        true
    }
}

#[derive(Default)]
struct Audit(Mutex<Vec<AuditEntry>>);

impl AuditSink for Audit {
    fn record(&self, entry: AuditEntry) {
        self.0.lock().unwrap().push(entry);
    }
}

impl Audit {
    fn count(&self, action: AuditAction) -> usize {
        self.0.lock().unwrap().iter().filter(|entry| entry.action == action).count()
    }

    fn outcomes(&self, action: AuditAction) -> Vec<String> {
        self.0.lock().unwrap().iter().filter(|entry| entry.action == action).map(|entry| entry.outcome.clone()).collect()
    }
}

/// Records deliveries and answers `start` with `{"started":1}`, anything else with `{}`.
#[derive(Default)]
struct Guest {
    events: Vec<BusEvent>,
    results: Vec<(u64, Result<String, HostError>)>,
}

impl BusHooks for Guest {
    fn on_events(&mut self, events: Vec<BusEvent>) {
        self.events.extend(events);
    }

    fn on_command(&mut self, command: &str, _args: &str, _caller: &Caller) -> Result<String, CommandError> {
        Ok(if command == "start" { r#"{"started":1}"#.into() } else { "{}".into() })
    }

    fn on_call_result(&mut self, call: u64, outcome: Result<String, HostError>) {
        self.results.push((call, outcome));
    }
}

fn spec(installation_id: &str, manifest: Value) -> InstanceSpec {
    let manifest: ExtensionManifest = serde_json::from_value(manifest).expect("test manifest");
    InstanceSpec {
        installation_id: installation_id.into(),
        content_hash: "ab".repeat(32),
        verse_id: "01JVERSE".into(),
        petal_id: PETAL.into(),
        identity_did: LOCAL.into(),
        manifest: Arc::new(manifest),
    }
}

fn fleet_manifest() -> Value {
    json!({
        "manifest_version": 2, "id": FLEET, "version": "0.1.0", "name": "Fleet", "kind": "simulation",
        "contract": "fractalengine:plugin@2.0.0", "binding": "petal",
        "bus": { "expose": {
            "streams": [{ "id": "positions", "max_rate_hz": 4, "max_event_bytes": 4096 }],
            "commands": [
                { "id": "start", "role_floor": "editor", "rate_hz": 1,
                  "args_schema": { "type": "object", "properties": { "node_id": { "type": "string", "maxLength": 128 } }, "additionalProperties": false } },
                { "id": "ping", "role_floor": "viewer", "rate_hz": 10 }
            ]
        } }
    })
}

fn bi_manifest() -> Value {
    json!({
        "manifest_version": 2, "id": "org.example.bi-push", "version": "0.1.0", "name": "BI push", "kind": "bi-connector",
        "contract": "fractalengine:plugin@2.0.0", "binding": "petal",
        "bus": {
            "consume": [
                { "source": "host/query", "id": "latest", "sql": "SELECT * FROM iot_reading", "min_interval_ms": 1000 },
                { "source": "host/query", "id": "petals", "sql": "SELECT * FROM petal" },
                { "source": "org.example.fleet-sim/positions" },
                { "source": "org.example.console/selection" }
            ],
            "call": ["org.example.fleet-sim/start", "org.example.absent/ping"]
        }
    })
}

fn reading(node_id: &str) -> ReadingEvent {
    ReadingEvent {
        petal_id: PETAL.into(),
        row: ReadingRow {
            reading_id: "01JREADING".into(),
            node_id: node_id.into(),
            metric: "altitude_m".into(),
            value: 112.5,
            units: "m".into(),
            recorded_at: "2026-11-08T19:50:00+00:00".into(),
            recorded_at_ms: 1_762_631_400_000,
            source_did: LOCAL.into(),
            origin: ReadingOrigin::Local,
        },
    }
}

fn deliver(bus: &Bus, key: &InstanceKey) -> Guest {
    let mut guest = Guest::default();
    bus.dispatch(key, &mut guest, 64);
    guest
}

fn advance(clock: &ManualClock, bus: &Bus, ms: u64) {
    for _ in 0..ms / 100 {
        clock.set(clock.now_ms() + 100);
        bus.poll();
    }
}

/// J4: guarded query parity and change-triggered pacing, stream ceilings, command floors and queues, audit; then a
/// mounted portal's capped snapshot, emit, invoke, one-op writes and revocation (bus/AGENTS.md §portal-clients).
#[test]
fn bus_journey_queries_streams_commands_and_audit() {
    let clock = ManualClock::default();
    clock.set(1_000);
    let query = Arc::new(FakeQuery {
        rows: Mutex::new(vec![json!({ "reading_id": "r1", "value": 1.0 })]),
        runs: AtomicUsize::new(0),
        requests: Mutex::new(Vec::new()),
    });
    let audit = Arc::new(Audit::default());
    let sources = BusSources {
        query: Some(query.clone()),
        scene: Some(Arc::new(OversizedPetal)),
        roles: Arc::new(Roles),
        policy: Arc::new(OpenPolicy),
        audit: audit.clone(),
    };
    let bus = Bus::new(Arc::new(clock.clone()), sources, LOCAL);
    let fleet = bus.register_instance(spec("inst-fleet", fleet_manifest())).unwrap();
    let bi = bus.register_instance(spec("inst-bi", bi_manifest())).unwrap();
    assert_eq!(audit.count(AuditAction::Grant), 2, "one grant per admitted bus surface");
    let runs = || query.runs.load(Ordering::SeqCst);

    // 1. host/query delivers what the guarded core returns, judged as the local DID at the binding petal.
    let latest = bus.subscribe(&bi, "host/query", r#"{"query":"latest"}"#).unwrap();
    bus.poll();
    let events = deliver(&bus, &bi).events;
    assert_eq!(events.len(), 1);
    assert_eq!((events[0].subscription, events[0].kind, events[0].seq), (latest, DeliveryKind::Snapshot, 1));
    let data: Value = serde_json::from_str(&events[0].data).unwrap();
    assert_eq!(data, json!({ "data": [{ "reading_id": "r1", "value": 1.0 }], "row_count": 1, "truncated": false }));
    let request = query.requests.lock().unwrap()[0].clone();
    assert_eq!((request.identity_did.as_str(), request.petal_id.as_str(), request.sql.as_str()), (LOCAL, PETAL, "SELECT * FROM iot_reading"));

    // 2. An idle petal costs zero reads.
    advance(&clock, &bus, 10_000);
    assert_eq!(runs(), 1);

    // 3. A write to a table the query does not read: none. Twenty writes in one window: one re-run. Same rows: no event.
    bus.feed_node_changes(&[SceneChange::NodeRenamed { node_id: "n1".into(), petal_id: PETAL.into(), new_name: "Mover".into() }]);
    advance(&clock, &bus, 2_000);
    assert_eq!(runs(), 1);
    for _ in 0..20 {
        bus.feed_readings(&[reading("n1")]);
    }
    advance(&clock, &bus, 3_000);
    assert_eq!(runs(), 2, "twenty writes, one re-run");
    assert_eq!(bus.counters(&bi).unwrap().query_unchanged, 1);
    assert!(deliver(&bus, &bi).events.is_empty(), "an identical result delivers no event");
    *query.rows.lock().unwrap() = vec![json!({ "reading_id": "r2", "value": 2.0 })];
    bus.feed_readings(&[reading("n1")]);
    advance(&clock, &bus, 1_100);
    assert_eq!(runs(), 3);
    assert_eq!(deliver(&bus, &bi).events.len(), 1, "a changed result is one snapshot");

    // 4. A query over a table with no change feed is refused at subscribe.
    assert!(matches!(bus.subscribe(&bi, "host/query", r#"{"query":"petals"}"#), Err(HostError::NotAvailable(_))));

    // 5. A stream pushed past its ceiling: the excess is rate-limited and counted, the consumer gets the rest.
    let positions = bus.subscribe(&bi, "org.example.fleet-sim/positions", "{}").unwrap();
    let published: Vec<_> = (0..10).map(|i| bus.publish(&fleet, "positions", &format!(r#"{{"count":{i}}}"#))).collect();
    assert_eq!(published.iter().filter(|outcome| outcome.is_ok()).count(), 4);
    assert!(published[4..].iter().all(|outcome| matches!(outcome, Err(HostError::RateLimited(_)))));
    assert_eq!(bus.counters(&fleet).unwrap().dropped_rate, 6);
    let streamed = deliver(&bus, &bi).events;
    assert_eq!(streamed.iter().filter(|event| event.subscription == positions && event.kind == DeliveryKind::Delta).count(), 4);

    // 7. An undeclared consume is refused, and the denial is one audit entry.
    let denials = audit.count(AuditAction::Denial);
    assert!(matches!(bus.subscribe(&fleet, "host/readings", "{}"), Err(HostError::CapabilityDenied(_))));
    assert_eq!(audit.count(AuditAction::Denial), denials + 1);

    // 8. A call is one audit entry, written when its outcome is known.
    let ui = Caller { did: LOCAL.into(), kind: CallerKind::Ui, package_id: None, local: true, role_ceiling: None };
    let calls = audit.count(AuditAction::CommandCall);
    let mut ticket = bus.call_command(&ui, PETAL, FLEET, "start", &json!({})).unwrap();
    assert_eq!(audit.count(AuditAction::CommandCall), calls, "not yet answered");
    assert_eq!(bus.dispatch(&fleet, &mut Guest::default(), 8), 1);
    assert_eq!(ticket.reply.try_recv().unwrap(), CommandOutcome::Ok(r#"{"started":1}"#.into()));
    assert_eq!(audit.count(AuditAction::CommandCall), calls + 1);

    // 6. A full command queue answers rate-limited (ui and local REST are separate caller keys).
    clock.set(clock.now_ms() + 2_000);
    let rest = Caller { did: LOCAL.into(), kind: CallerKind::Rest, package_id: None, local: true, role_ceiling: None };
    let mut queued = Vec::new();
    for caller in std::iter::repeat_n(&ui, 10).chain(std::iter::repeat_n(&rest, 6)) {
        queued.push(bus.call_command(caller, PETAL, FLEET, "ping", &json!({})).unwrap());
    }
    match bus.call_command(&rest, PETAL, FLEET, "ping", &json!({})) {
        Err(BusRefusal::RateLimited(reason)) => assert!(reason.contains("queue"), "{reason}"),
        other => panic!("expected a full queue, got {other:?}"),
    }
    assert_eq!(bus.dispatch(&fleet, &mut Guest::default(), 64), 16);

    // 9. Per-caller keying: the extension spends start's budget while a ui call in the same second succeeds;
    //    a package with no instance at the petal answers the extension caller not-available.
    clock.set(clock.now_ms() + 2_000);
    let first = bus.call(&bi, "org.example.fleet-sim/start", "{}").unwrap();
    assert!(matches!(bus.call(&bi, "org.example.fleet-sim/start", "{}"), Err(HostError::RateLimited(_))));
    assert!(bus.call_command(&ui, PETAL, FLEET, "start", &json!({})).is_ok());
    assert!(matches!(bus.call(&bi, "org.example.absent/ping", "{}"), Err(HostError::NotAvailable(_))));
    assert_eq!(bus.dispatch(&fleet, &mut Guest::default(), 8), 2);
    assert_eq!(deliver(&bus, &bi).results, vec![(first, Ok(r#"{"started":1}"#.to_string()))]);

    // Remote callers: an `external: false` command and an absent package get the same not-found, each one denial.
    let remote = Caller { did: "did:key:z6MkRemoteEditor".into(), kind: CallerKind::Rest, package_id: None, local: true, role_ceiling: None };
    let denials = audit.count(AuditAction::Denial);
    assert_eq!(bus.call_command(&remote, PETAL, FLEET, "start", &json!({})).unwrap_err(), BusRefusal::NotFound);
    assert_eq!(bus.call_command(&remote, PETAL, "org.example.absent", "start", &json!({})).unwrap_err(), BusRefusal::NotFound);
    assert_eq!(audit.count(AuditAction::Denial), denials + 2);

    // G1: a delegated token of this node carries the local DID under a viewer ceiling; it is not local (same not-found).
    let delegated = Caller { role_ceiling: Some(fe_policy::RoleLevel::Viewer), ..rest.clone() };
    assert_eq!(bus.call_command(&delegated, PETAL, FLEET, "start", &json!({})).unwrap_err(), BusRefusal::NotFound);

    // ---- A mounted portal bundle (portal ipc-v1.md §5-§6, A-13) -----------------------------------------------

    // Its stream has no producer until the bundle mounts; mounted, the consuming extension may subscribe.
    let selection = format!("{CONSOLE}/selection");
    assert!(matches!(bus.subscribe(&bi, &selection, "{}"), Err(HostError::NotAvailable(_))));
    let console = bus.open_portal_client(LOCAL, RoleLevel::Editor, console_grants());
    let consumed = bus.subscribe(&bi, &selection, "{}").unwrap();

    // A portal consumes its declared hooks at its mount petal only.
    let elsewhere = bus.client_subscribe(console.id, "01JOTHERPETAL", &format!("{FLEET}/positions"), &json!({}));
    assert!(matches!(elsewhere, Err(BusRefusal::CapabilityDenied(_))));

    // G1-C6: a node snapshot stops at REST's row cap and says so; properties are cut to the bundle's scope.
    bus.client_subscribe(console.id, PETAL, "host/node-changes", &json!({})).unwrap();
    bus.poll();
    let parts = bus.take_client_events(console.id, 64);
    assert!(parts.iter().all(|event| event.kind == DeliveryKind::Snapshot));
    let parts: Vec<Value> = parts.iter().map(|event| serde_json::from_str(&event.data).unwrap()).collect();
    let nodes: usize = parts.iter().map(|part| part["nodes"].as_array().unwrap().len()).sum();
    assert_eq!(nodes, REST_ROW_CAP);
    assert_eq!((parts.last().unwrap()["last"].clone(), parts.last().unwrap()["truncated"].clone()), (json!(true), json!(true)));
    assert_eq!(parts[0]["nodes"][0]["properties"], json!({ "fleet.speed": 0 }));

    // emit: a declared stream under its own ceiling (4/s) reaches the consuming extension; an undeclared one is a denial.
    let emitted: Vec<_> = (0..6).map(|i| bus.client_publish(console.id, "selection", &json!({ "node_id": format!("n{i}") }))).collect();
    assert_eq!(emitted.iter().filter(|outcome| outcome.is_ok()).count(), 4);
    assert!(matches!(emitted[5], Err(BusRefusal::RateLimited(_))));
    let denials = audit.count(AuditAction::Denial);
    assert!(matches!(bus.client_publish(console.id, "undeclared", &json!({})), Err(BusRefusal::CapabilityDenied(_))));
    assert_eq!(audit.count(AuditAction::Denial), denials + 1);
    let received = deliver(&bus, &bi).events;
    assert_eq!(received.iter().filter(|event| event.subscription == consumed && event.kind == DeliveryKind::Delta).count(), 4);

    // invoke: declared targets only, judged as the binding (never a Caller literal), its ceiling capping the role.
    let mut invoked = bus.client_call_command(console.id, FLEET, "start", &json!({})).unwrap();
    assert_eq!(bus.dispatch(&fleet, &mut Guest::default(), 8), 1);
    assert_eq!(invoked.reply.try_recv().unwrap(), CommandOutcome::Ok(r#"{"started":1}"#.into()));
    assert!(matches!(bus.client_call_command(console.id, FLEET, "ping", &json!({})), Err(BusRefusal::CapabilityDenied(_))));
    let literal = Caller { did: LOCAL.into(), kind: CallerKind::Portal, package_id: Some(CONSOLE.into()), local: true, role_ceiling: None };
    assert!(matches!(bus.call_command(&literal, PETAL, FLEET, "start", &json!({})), Err(BusRefusal::InvalidInput(_))));
    let viewer = bus.open_portal_client(LOCAL, RoleLevel::Viewer, console_grants());
    assert!(matches!(bus.client_call_command(viewer.id, FLEET, "start", &json!({})), Err(BusRefusal::CapabilityDenied(_))));

    // set_property: one op on the write path with the binding's provenance, after the host has a write path.
    assert!(matches!(bus.client_set_property(console.id, "n1", "fleet.speed", &json!(3)), Err(BusRefusal::NotAvailable(_))));
    let writes = Arc::new(Writes::default());
    assert!(bus.install_portal_writes(writes.clone()));
    let mut written = bus.client_set_property(console.id, "n1", "fleet.speed", &json!(3)).unwrap();
    assert!(matches!(written.reply.try_recv().unwrap(), WriteOutcome::Committed(_)));
    let sent = writes.seen.lock().unwrap()[0].clone();
    assert_eq!((sent.package_id.as_str(), sent.content_hash.as_str(), sent.petal_id.as_str()), (CONSOLE, "cd".repeat(32).as_str(), PETAL));
    assert_eq!(sent.op, PendingOp::SetProperty { node_id: "n1".into(), key: "fleet.speed".into(), value: json!(3) });

    // Reserved and undeclared keys never reach the DB; a viewer-capped binding is role-denied before it.
    for key in ["portal.bundle_asset", "node_kind", "secret"] {
        assert!(matches!(bus.client_set_property(console.id, "n1", key, &json!(1)), Err(BusRefusal::CapabilityDenied(_))));
    }
    let mut denied = bus.client_set_property(viewer.id, "n1", "fleet.speed", &json!(4)).unwrap();
    assert!(matches!(denied.reply.try_recv().unwrap(), WriteOutcome::RoleDenied(_)));
    assert_eq!(writes.seen.lock().unwrap().len(), 1, "nothing but the one accepted write reached the write path");
    assert_eq!(audit.outcomes(AuditAction::PropertyWrite), ["ok", "role-denied"]);

    // The package budget holds across clients and instants: twenty calls at once, exactly the burst lands.
    clock.set(clock.now_ms() + 3_000);
    let accepted = (0..20).filter(|_| bus.client_set_property(console.id, "n1", "fleet.speed", &json!(5)).is_ok()).count();
    assert_eq!(accepted, PORTAL_WRITE_BURST as usize);

    // OD-E1: revoking the bundle by hash under another id stops its clients and its stream at once; a write already
    // in flight still settles and its row stays.
    clock.set(clock.now_ms() + 3_000);
    writes.hold.store(true, Ordering::SeqCst);
    let mut in_flight = bus.client_set_property(console.id, "n2", "console.note", &json!("hi")).unwrap();
    bus.feed_admission(&AdmissionEvent {
        package_id: "org.example.renamed".into(),
        content_hash: "cd".repeat(32),
        kind: AdmissionKind::WebBundle,
        reason: AdmissionReason::RevocationList,
        verse_id: None,
    });
    let ended = bus.take_client_events(console.id, 64);
    assert_eq!(ended.last().map(|event| (event.kind, event.data.to_string())), Some((DeliveryKind::Ended, r#"{"reason":"revoked"}"#.into())));
    let producer_gone = deliver(&bus, &bi).events;
    assert!(producer_gone.iter().any(|event| event.subscription == consumed && event.kind == DeliveryKind::Ended && event.data.contains("producer-inactive")));
    assert!(matches!(bus.client_set_property(console.id, "n2", "console.note", &json!("again")), Err(BusRefusal::NotAvailable(_))));
    assert!(matches!(bus.client_publish(console.id, "selection", &json!({})), Err(BusRefusal::NotAvailable(_))));
    for reply in writes.held.lock().unwrap().drain(..) {
        let _ = reply.send(Ok(committed()));
    }
    bus.poll();
    assert!(matches!(in_flight.reply.try_recv().unwrap(), WriteOutcome::Committed(_)));
    assert_eq!(audit.outcomes(AuditAction::PropertyWrite).last().map(String::as_str), Some("ok"));
}
