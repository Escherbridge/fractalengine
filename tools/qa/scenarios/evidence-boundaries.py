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
summary = {"scope": "offline synthetic validator fixtures; zero application checkpoint coverage",
           "passed": all(result["passed"] for result in results), "cases": results}
(output / "results.json").write_text(json.dumps(summary, indent=2), encoding="utf-8")
print(json.dumps(summary, indent=2))
sys.exit(0 if summary["passed"] else 2)
