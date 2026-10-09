//! `fe-sim` — the simulation-lab scenario runner binary (F8).
//!
//! ```text
//! fe-sim                     # run the built-in default two-peer script
//! fe-sim run <script.json>   # run one declarative scenario file
//! fe-sim print <script.json> # parse-validate + print the merged plan (no run)
//! ```
//!
//! The full control surface (REST `POST /api/v1/sim/*` + MCP sim tools +
//! interactive fault injection) is F9/A20; this binary is the deterministic
//! CLI leg: same [`ScenarioScript`] document everywhere.

use std::path::PathBuf;

fn main() {
    // The harness/DB threads are generous with stack (surrealdb-core), so
    // match the workspace test invocation's headroom in the binary too.
    let args: Vec<String> = std::env::args().skip(1).collect();
    let code = match args.first().map(String::as_str) {
        None => run_builtin(),
        Some("run") if args.len() == 2 => run_file(&args[1]),
        Some("print") if args.len() == 2 => print_plan(&args[1]),
        Some(other) => {
            eprintln!("unknown command or wrong arity: '{other}'");
            eprintln!("usage: fe-sim [run <script.json> | print <script.json>]");
            2
        }
    };
    std::process::exit(code);
}

fn run_builtin() -> i32 {
    let script = fe_sim::scenario::default_script();
    println!(
        "running built-in scenario '{}' ({} peers, {} sensors, {} faults)",
        script.name,
        script.fleet.peers.len(),
        script.fleet.sensors.len(),
        script.events.len()
    );
    let dir = tempfile::tempdir().expect("scenario tempdir");
    match fe_sim::scenario::run_scenario(&script, dir.path()) {
        Ok(outcome) => {
            print_outcome(&outcome);
            0
        }
        Err(e) => {
            eprintln!("scenario failed: {e:#}");
            1
        }
    }
}

fn run_file(path: &str) -> i32 {
    let script = match load_script(PathBuf::from(path)) {
        Ok(s) => s,
        Err(code) => return code,
    };
    let dir = tempfile::tempdir().expect("scenario tempdir");
    match fe_sim::scenario::run_scenario(&script, dir.path()) {
        Ok(outcome) => {
            print_outcome(&outcome);
            0
        }
        Err(e) => {
            eprintln!("scenario failed: {e:#}");
            1
        }
    }
}

fn print_plan(path: &str) -> i32 {
    let script = match load_script(PathBuf::from(path)) {
        Ok(s) => s,
        Err(code) => return code,
    };
    let plan = fe_sim::fleet::plan_fleet(&script.fleet);
    println!(
        "scenario '{}' valid: {} peer(s), {} sensor(s), {} fault event(s), {} scheduled reading(s)",
        script.name,
        script.fleet.peers.len(),
        script.fleet.sensors.len(),
        script.events.len(),
        plan.len()
    );
    for reading in plan.iter().take(20) {
        let sensor = &script.fleet.sensors[reading.sensor];
        println!(
            "  t+{}ms  {}/{} tick {} = {:.4}",
            reading.at_ms - script.fleet.start_ms,
            sensor.anchor,
            sensor.metric,
            reading.tick,
            reading.value
        );
    }
    if plan.len() > 20 {
        println!("  … ({} more)", plan.len() - 20);
    }
    0
}

fn load_script(path: PathBuf) -> Result<fe_sim::scenario::ScenarioScript, i32> {
    let raw = match std::fs::read_to_string(&path) {
        Ok(raw) => raw,
        Err(e) => {
            eprintln!("cannot read {}: {e}", path.display());
            return Err(2);
        }
    };
    match fe_sim::scenario::ScenarioScript::parse(&raw) {
        Ok(script) => Ok(script),
        Err(e) => {
            eprintln!("invalid scenario {}: {e:#}", path.display());
            Err(2)
        }
    }
}

fn print_outcome(outcome: &fe_sim::scenario::ScenarioOutcome) {
    println!("scenario '{}' complete", outcome.name);
    println!("  readings ingested: {}", outcome.ingested);
    println!("  dropped deliveries: {}", outcome.dropped_deliveries);
    println!(
        "  iroh endpoints before/after: {}/{} (must be equal — no real network)",
        outcome.endpoints_before, outcome.endpoints_after
    );
    for (peer, readings) in outcome.fingerprint() {
        println!("  peer '{peer}': {} reading(s)", readings.len());
    }
    match serde_json::to_string_pretty(&serde_json::json!({
        "name": outcome.name,
        "ingested": outcome.ingested,
        "dropped_deliveries": outcome.dropped_deliveries,
        "endpoints_before": outcome.endpoints_before,
        "endpoints_after": outcome.endpoints_after,
        "per_peer": outcome.fingerprint(),
    })) {
        Ok(json) => println!("{json}"),
        Err(e) => eprintln!("(fingerprint not serializable: {e})"),
    }
}
