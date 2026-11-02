"""Replay offline evidence-boundary fixtures; see tools/qa/AGENTS.md."""

import argparse
import copy
import hashlib
import json
from pathlib import Path
import subprocess
import sys

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument("--out", required=True, help="new retained fixture directory, separate from the app ledger")
options = parser.parse_args()
output = Path(options.out).resolve()
output.mkdir(parents=True, exist_ok=False)
tools = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(tools))
from layout_check import analyze
from owned_stdout import context, digest, prefix_hash, window_projection
from owned_stdout_windows import WindowsLease
from coverage import artifact, evidence_object

identity = {"pass_id": "fixture", "run_id": "offline-owned-fixture"}
before = {"camera": {"pose": {"position": [0, 1, 2]}}, "hierarchy": {"verses": []}}
gate_refusal = {**identity, "schema": "fe-qa-gate-off-refusal-v1",
    "request": {"method": "GET", "path": "/api/v1/debug/logs"},
    "response": {"status": 403, "body": {"ok": False, "code": "qa_gate_refused",
        "error": "QA diagnostics endpoint is disabled on this node"}}}
no_effect = {**identity, "schema": "fe-qa-gate-off-no-effect-v1",
    "request": {"method": "POST", "path": "/api/v1/debug/input", "body": {
        "commands": [{"kind": "pointer_move", "x": 0, "y": 0}], "wait_frames": 0}},
    "response": {"status": 403, "body": {"ok": False,
        "error": "input injection endpoint is disabled on this node"}},
    "before": before, "after": before}
exception = {"kind": "expected_input_disabled", "refusal_artifact": "qa/passes/fixture/refusal.json",
             "no_effect_artifact": "qa/passes/fixture/no-effect.json"}
ordinary = {"log_watermark": {"generation": 3, "seq": 100},
            "log_after": {"generation": 3, "seq": 102}, "new_logs": []}
negative = {"log_watermark": {"unavailable_reason": "input_disabled"},
            "new_logs": [], "log_exception": exception}
cases = [
    ("ordinary", True, "F110.state", "A", ordinary, {}),
    ("reviewed-warning", True, "F110.state", "A", ordinary,
        {"new_logs": [{"expected": True, "reason": "fixture warning", "seq": 101}]}),
    ("capacity-boundary", True, "F110.state", "A", ordinary,
        {"log_after.seq": 2100}),
    ("consistent-retention", True, "F110.state", "A", ordinary,
        {"retention": {"possible_gap": False, "seq_delta": 2, "source_derived_capacity": 2000}}),
    ("unreviewed-warning", False, "F110.state", "A", ordinary,
        {"new_logs": [{"expected": False, "reason": "not reconciled", "seq": 101}]}),
    ("missing-after", False, "F110.state", "A", ordinary, {"log_after": "__omit__"}),
    ("missing-before", False, "F110.state", "A", ordinary, {"log_watermark": "__omit__"}),
    ("boolean-counter", False, "F110.state", "A", ordinary, {"log_after.seq": True}),
    ("new-generation", False, "F110.state", "A", ordinary, {"log_after.generation": 4}),
    ("older-generation", False, "F110.state", "A", ordinary, {"log_after.generation": 2}),
    ("regressed-sequence", False, "F110.state", "A", ordinary, {"log_after.seq": 99}),
    ("evicted-window", False, "F110.state", "A", ordinary, {"log_after.seq": 2101}),
    ("explicit-gap", False, "F110.state", "A", ordinary, {"retention": {"possible_gap": True}}),
    ("unknown-retention", False, "F110.state", "A", ordinary, {"retention": None}),
    ("post-probe-unavailable", False, "F110.state", "A", ordinary, {"new_logs_unavailable": "fixture timeout"}),
    ("dishonest-retention", False, "F110.state", "A", ordinary,
        {"retention": {"possible_gap": False, "seq_delta": 0, "source_derived_capacity": 2000}}),
    ("ordinary-with-exception", False, "F110.state", "A", ordinary, {"log_exception": exception}),
    ("gate-off-api", True, "F112.gate-off", "A", negative, {}),
    ("gate-off-os-denial", True, "F114.input-debug-denied", "O", negative, {}),
    ("exception-wrong-checkpoint", False, "F110.state", "A", negative, {}),
    ("exception-wrong-classification", False, "F114.input-debug-denied", "A", negative, {}),
    ("exception-positive-os", False, "F114.named-buttons", "O", negative, {}),
    ("exception-freeform-excuse", False, "F112.gate-off", "A", negative,
        {"log_watermark.unavailable_reason": "could not collect logs"}),
    ("exception-missing-proof", False, "F112.gate-off", "A", negative,
        {"log_exception.no_effect_artifact": "qa/passes/fixture/missing.json"}),
    ("exception-foreign-run", False, "F112.gate-off", "A", negative, {"refusal.run_id": "old-run"}),
    ("exception-foreign-checkpoint", False, "F112.gate-off", "A", negative,
        {"refusal.checkpoint": "F110.state"}),
    ("exception-masked-scope-is-not-gateoff", False, "F112.gate-off", "A", negative,
        {"refusal.response": {"status": 404, "body": {"ok": False, "error": "not found or denied"}}}),
    ("exception-success-response", False, "F112.gate-off", "A", negative,
        {"proof.response": {"status": 200, "body": {"ok": True, "commands_applied": 1}}}),
    ("exception-invalid-request", False, "F112.gate-off", "A", negative, {"proof.request": None}),
    ("exception-state-changed", False, "F112.gate-off", "A", negative,
        {"proof.after": {"camera": {"pose": {"position": [99, 1, 2]}}, "hierarchy": {"verses": []}}}),
    ("exception-null-readbacks", False, "F112.gate-off", "A", negative,
        {"proof.before": {"camera": None, "hierarchy": None}, "proof.after": {"camera": None, "hierarchy": None}}),
]
results = []
for label, expected, checkpoint, reachability, window, changes in cases:
    root = output / label
    evidence = root / "qa" / "passes" / "fixture"
    evidence.mkdir(parents=True)
    catalog = {"catalog_source_fingerprint": "a" * 64,
        "families": [{"id": checkpoint.split(".")[0], "region": "R9", "requires_persistence": False}],
        "checkpoints": [{"id": checkpoint, "family": checkpoint.split(".")[0], "reachability": reachability}],
        "gold_tasks": {"G9": [checkpoint]}}
    record = {**identity, "checkpoint": checkpoint, "outcome": "PASS", "stage_commit": "7f51de19",
        "stage_fingerprint": "a" * 64, "commands": ["offline fixture, not application coverage"],
        "assertions": [{"name": "fixture", "expected": "fixture", "actual": "fixture", "passed": True}],
        **copy.deepcopy(window)}
    documents = {"record": record, "refusal": {**copy.deepcopy(gate_refusal), "checkpoint": checkpoint},
                 "proof": {**copy.deepcopy(no_effect), "checkpoint": checkpoint}}
    for dotted, value in changes.items():
        parts = dotted.split(".")
        target = documents[parts.pop(0)] if parts[0] in {"refusal", "proof"} else record
        for key in parts[:-1]:
            target = target[key]
        if value == "__omit__":
            target.pop(parts[-1], None)
        else:
            target[parts[-1]] = value
    artifacts = []
    for filename, document, kind in [("refusal.json", documents["refusal"], "api"),
            ("no-effect.json", documents["proof"], "os" if reachability == "O" else "tool")]:
        data = json.dumps(document, sort_keys=True).encode("utf-8")
        (evidence / filename).write_bytes(data)
        artifacts.append({"path": "qa/passes/fixture/" + filename, "kind": kind,
                          "sha256": hashlib.sha256(data).hexdigest()})
    record["artifacts"] = artifacts
    (root / "catalog.json").write_text(json.dumps(catalog), encoding="utf-8")
    ledger = evidence / "checkpoints.jsonl"
    ledger.write_text(json.dumps(record) + "\n", encoding="utf-8")
    command = [sys.executable, str(tools / "coverage.py"), "--root", str(root), "--catalog", str(root / "catalog.json"),
        "--ledger", str(ledger), "--pass-id", "fixture", "--run-id", identity["run_id"],
        "--stage-commit", "7f51de19", "--stage-fingerprint", "a" * 64, "--require-complete"]
    completed = subprocess.run(command, capture_output=True, text=True, timeout=15, check=False)
    result = {"case": label, "expected_accepted": expected, "exit_code": completed.returncode,
        "passed": completed.returncode == (0 if expected else 2), "command": command,
        "stdout": completed.stdout, "stderr": completed.stderr}
    results.append(result)

tree = {"frame": 1, "window": {"width": 1280, "height": 720}, "nodes": [
    {"id": "outer", "parent": None, "interactive": False},
    {"id": "inner", "parent": "outer", "interactive": False},
    {"id": "action", "parent": "inner", "interactive": True, "enabled": True, "hidden": False,
     "name": "Edit property fixture", "bounds": {"x": 400, "y": 50, "w": 30, "h": 24}}],
    "layout": {"scroll_geometry_version": 2, "scrolls": [
        {"content_id": "inner", "parent_id": "outer", "label": "Custom property table", "layer": "L",
         "axes": [True, False], "viewport": {"x": 100, "y": 0, "w": 270, "h": 321},
         "clip": {"x": 100, "y": 0, "w": 270, "h": 492},
         "content_rect": {"x": 100, "y": 0, "w": 496, "h": 321},
         "content_size": [496, 321], "offset": [0, 0]},
        {"content_id": "outer", "parent_id": None, "label": "Inspector", "layer": "L",
         "axes": [False, True], "viewport": {"x": 100, "y": 0, "w": 316, "h": 492},
         "clip": {"x": 100, "y": 0, "w": 316, "h": 492},
         "content_rect": {"x": 100, "y": 0, "w": 304, "h": 1231.25},
         "content_size": [304, 1231.25], "offset": [0, 739.25]}],
        "controls": [{"id": "action", "allowed": True, "clickable": True, "draggable": False, "layer": "L",
            "rect": {"x": 400, "y": 50, "w": 30, "h": 24},
            "hit": {"x": 400, "y": 50, "w": -30, "h": 24},
            "clip": {"x": 400, "y": 50, "w": -30, "h": 24}}], "texts": []}}
layout_cases = [("horizontal-range", "intentional", None),
    ("disabled-axis-zero", "unknown", ("h", 0)),
    ("enabled-axis-zero", "unknown", ("w", 0)),
    ("nonfinite-axis", "unknown", ("h", float("nan"))),
    ("genuine-no-scroll-range", "violation", None),
    ("small-hit-still-fails", "small_hit_target", None),
    ("masked-background", "modal_background", ("h", 0))]
for label, expected, viewport_change in layout_cases:
    sample = copy.deepcopy(tree)
    if viewport_change:
        sample["layout"]["scrolls"][0]["viewport"][viewport_change[0]] = viewport_change[1]
    if label == "genuine-no-scroll-range":
        sample["layout"]["scrolls"][0]["axes"] = [False, False]
    if label == "small-hit-still-fails":
        small = {"x": 110, "y": 50, "w": 10, "h": 10}
        sample["nodes"][2]["bounds"] = small
        sample["layout"]["controls"][0].update(rect=small, clip=small, hit=small)
    if label == "masked-background":
        sample["layout"]["controls"][0]["allowed"] = False
    state = {"frame": 1, "viewport": {"w": 800}}
    actual = analyze(sample, state)
    violations = [entry["reason"] for entry in actual["violations"]]
    unknown = [entry["reason"] for entry in actual["unknown"]]
    intentional = [entry["reason"] for entry in actual["intentional_scroll_content"]]
    passed = ((expected == "intentional" and "scroll_content" in intentional and not violations and not unknown)
        or (expected == "unknown" and bool(unknown) and not violations and not intentional)
        or (expected == "violation" and "clipped_without_scroll_range" in violations)
        or expected in violations or expected in intentional)
    passed &= actual["coverage_complete"] is False
    fixture = {"tree": sample, "state": state, "actual": actual}
    (output / (label + ".json")).write_text(json.dumps(fixture, indent=2), encoding="utf-8")
    results.append({"case": label, "passed": passed, "expected": expected,
                    "violations": violations, "unknown": unknown, "intentional": intentional})
stdout_cases = [("stdout-info", True), ("stdout-reviewed-warning", True), ("stdout-later-append", True),
    ("stdout-missing-warning", False), ("stdout-unreviewed-warning", False), ("stdout-truncated", False),
    ("stdout-replaced", False), ("stdout-prefix-modified", False), ("stdout-restarted", False),
    ("stdout-forged-liveness", False), ("stdout-forged-file-id", False), ("stdout-forged-projection", False),
    ("stdout-projection-invalid-shape", False), ("stdout-projection-omits-warning", False),
    ("stdout-unclassified-line", False), ("stdout-partial-line", False), ("stdout-oversized", False),
    ("stdout-input-on", False), ("stdout-wrong-path", False), ("stdout-wrong-executable", False),
    ("stdout-mixed-ring", False), ("stdout-not-os", False), ("stdout-foreign-begin", False)]
for label, expected in stdout_cases:
    root = output / label
    checkpoint, run_id = "F114.named-buttons", "20261102-120000-000"
    evidence = root / "qa" / "passes" / "fixture"
    run = root / ".qa" / "run" / run_id
    stage = root / ".qa" / "stage" / "offline-validator-fixture"
    for directory in (evidence, run, stage):
        directory.mkdir(parents=True)
    executable = stage / "fractalengine.exe"
    executable.write_bytes(b"offline validator placeholder; not an application")
    current = {"run_dir": str(run), "stage": str(stage), "stage_matches_source": True,
        "stage_manifest": {"source_commit": "7f51de19", "source_fingerprint": "a" * 64},
        "qa_options": {"synthetic_input": False}, "stdout_path": str(run / "stdout.log"),
        "process_identity": {"pid": 42, "start_ticks": "639000000000000000", "exe_path": str(executable)}}
    current_path = run / "current.json"
    current_path.write_text(json.dumps(current), encoding="utf-8")
    prefix = b"2026-11-02T12:00:00.000000Z INFO qa_fixture: baseline\n"
    level = "WARN" if label in {"stdout-reviewed-warning", "stdout-missing-warning", "stdout-unreviewed-warning",
                                "stdout-projection-omits-warning"} else "INFO"
    raw = ("2026-11-02T12:00:01.000000Z " + level + " qa_fixture: synthetic private-message sentinel\n").encode()
    source_path = run / "stdout.log"
    source_path.write_bytes(prefix + raw)
    # This exercises final offline verification, not the CLI's live-process admission.
    with WindowsLease() as lease:
        pinned, binding = context(lease, root, "fixture", run_id, checkpoint, live=False)
        begin = {**binding, "schema": "fe-qa-owned-stdout-begin-v1", "observed_live": True,
                 "offset": len(prefix), "prefix_sha256": prefix_hash(pinned, len(prefix))}
        end = {**binding, "schema": "fe-qa-owned-stdout-window-v1", "observed_live": True,
            "offset": len(prefix + raw), "prefix_sha256": prefix_hash(pinned, len(prefix + raw)),
            "raw_window_sha256": digest(raw), "projection": window_projection(raw)}
    original_warnings = copy.deepcopy(end["projection"]["warnings"])
    if label == "stdout-truncated":
        source_path.write_bytes(prefix)
    elif label == "stdout-replaced":
        source_path.rename(run / "original-stdout.log")
        source_path.write_bytes(prefix + raw)
    elif label == "stdout-prefix-modified":
        source_path.write_bytes(prefix.replace(b"baseline", b"modified") + raw)
    elif label == "stdout-restarted":
        current["process_identity"]["start_ticks"] = "639000000000000001"
    elif label == "stdout-forged-liveness":
        begin["observed_live"] = False
    elif label == "stdout-forged-file-id":
        end["source_identity"] = {"volume": "00000000", "file_id": "0000000000000000"}
    elif label == "stdout-forged-projection":
        end["projection"]["lines"][0]["message"] = "unredacted synthetic message"
    elif label == "stdout-projection-invalid-shape":
        end["projection"] = {"lines": "invalid", "warnings": []}
    elif label == "stdout-projection-omits-warning":
        end["projection"]["warnings"] = []
    elif label in {"stdout-unclassified-line", "stdout-partial-line"}:
        raw = b"unclassified fixture output\n" if label.endswith("unclassified-line") else raw[:-1]
        source_path.write_bytes(prefix + raw)
        end.update(offset=len(prefix + raw), prefix_sha256=digest(prefix + raw), raw_window_sha256=digest(raw))
    elif label == "stdout-oversized":
        end["offset"] = len(prefix) + 1024 * 1024 + 1
    elif label == "stdout-input-on":
        current["qa_options"]["synthetic_input"] = True
    elif label == "stdout-wrong-path":
        current["stdout_path"] = str(run / "unowned.log")
    elif label == "stdout-wrong-executable":
        current["process_identity"]["exe_path"] = str(run / "unowned.exe")
    elif label == "stdout-later-append":
        with source_path.open("ab") as handle:
            handle.write(b"2026-11-02T12:00:02.000000Z INFO qa_fixture: later independent window\n")
    elif label == "stdout-foreign-begin":
        begin["run_id"] = "20261101-120000-000"
    current_path.write_text(json.dumps(current), encoding="utf-8")
    begin_bytes = json.dumps(begin, sort_keys=True).encode()
    end["begin_receipt_sha256"] = digest(begin_bytes)
    end_bytes = json.dumps(end, sort_keys=True).encode()
    (evidence / "begin.json").write_bytes(begin_bytes)
    (evidence / "end.json").write_bytes(end_bytes)
    reachability = "A" if label == "stdout-not-os" else "O"
    catalog = {"catalog_source_fingerprint": "a" * 64,
        "families": [{"id": "F114", "region": "R9", "requires_persistence": False}],
        "checkpoints": [{"id": checkpoint, "family": "F114", "reachability": reachability}],
        "gold_tasks": {"G9": [checkpoint]}}
    warnings = [{**entry, "expected": True, "reason": "known synthetic offline warning"}
                for entry in original_warnings]
    if label == "stdout-missing-warning":
        warnings = []
    elif label == "stdout-unreviewed-warning":
        warnings[0]["expected"] = False
    record = {"pass_id": "fixture", "run_id": run_id, "checkpoint": checkpoint, "outcome": "PASS",
        "stage_commit": "7f51de19", "stage_fingerprint": "a" * 64,
        "commands": ["offline validator fixture; no app or live-process capture"],
        "assertions": [{"name": "fixture", "expected": "fixture", "actual": "fixture", "passed": True}],
        "log_source": {"kind": "owned_stdout", "begin_artifact": "qa/passes/fixture/begin.json",
                       "end_artifact": "qa/passes/fixture/end.json"}, "new_logs": warnings,
        "artifacts": [{"path": "qa/passes/fixture/begin.json", "kind": "tool", "sha256": digest(begin_bytes)},
                      {"path": "qa/passes/fixture/end.json", "kind": "os", "sha256": digest(end_bytes)}]}
    if label == "stdout-not-os":
        record["artifacts"][0]["kind"] = "api"
    if label == "stdout-mixed-ring":
        record["log_watermark"] = {"generation": 1, "seq": 1}
    (root / "catalog.json").write_text(json.dumps(catalog), encoding="utf-8")
    ledger = evidence / "checkpoints.jsonl"
    ledger.write_text(json.dumps(record) + "\n", encoding="utf-8")
    command = [sys.executable, str(tools / "coverage.py"), "--root", str(root), "--catalog", str(root / "catalog.json"),
        "--ledger", str(ledger), "--pass-id", "fixture", "--run-id", run_id,
        "--stage-commit", "7f51de19", "--stage-fingerprint", "a" * 64, "--require-complete"]
    completed = subprocess.run(command, capture_output=True, text=True, timeout=15, check=False)
    results.append({"case": label, "expected_accepted": expected, "exit_code": completed.returncode,
        "passed": completed.returncode == (0 if expected else 2), "command": command,
        "stdout": completed.stdout, "stderr": completed.stderr,
        "limit": "synthetic final-audit fixtures; live begin/end process checks remain separately required"})
for label, replacement, expected in [("exception-snapshot-control", False, True),
        ("exception-substituted-after-artifact-hash", True, False)]:
    root = output / label
    evidence = root / "qa" / "passes" / "fixture"
    evidence.mkdir(parents=True)
    record = {**identity, "checkpoint": "F112.gate-off"}
    valid = {**copy.deepcopy(gate_refusal), "checkpoint": "F112.gate-off"}
    initial = copy.deepcopy(valid)
    if replacement:
        initial["response"] = {"status": 200, "body": {"ok": True}}
    original = json.dumps(initial, sort_keys=True).encode()
    source = evidence / "refusal.json"
    source.write_bytes(original)
    declaration = {"path": "qa/passes/fixture/refusal.json", "kind": "api", "sha256": digest(original)}
    record["artifacts"] = [declaration]
    artifact(root, evidence, declaration)
    # Deterministic interleaving: replace bytes after the old hash step, before parsing.
    if replacement:
        source.write_text(json.dumps(valid, sort_keys=True), encoding="utf-8")
    accepted, detail = True, "parsed the declared snapshot"
    try:
        evidence_object(root, record, declaration["path"], {"api"})
    except ValueError as error:
        accepted, detail = False, str(error)
    (root / "initial.json").write_bytes(original)
    (root / "record.json").write_text(json.dumps(record), encoding="utf-8")
    results.append({"case": label, "expected_accepted": expected, "accepted": accepted,
                    "passed": accepted == expected, "detail": detail,
                    "boundary": "actual artifact hash then actual evidence_object; no timing-dependent race"})
summary = {"scope": "offline synthetic validator fixtures; zero application checkpoint coverage",
           "passed": all(result["passed"] for result in results), "cases": results}
(output / "results.json").write_text(json.dumps(summary, indent=2), encoding="utf-8")
print(json.dumps(summary, indent=2))
sys.exit(0 if summary["passed"] else 2)
