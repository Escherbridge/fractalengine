"""Replay offline evidence-boundary fixtures; see tools/qa/AGENTS.md."""

import argparse
import copy
import hashlib
import json
from pathlib import Path
import subprocess
import struct
import sys
import zlib

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
from terminal_stdout import BEGIN_SCHEMA, END_SCHEMA

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
terminal_cases = [("terminal-clean", True), ("terminal-confirm-ack", True), ("terminal-reviewed-warning", True),
    ("terminal-nonzero-exit", False), ("terminal-wrong-pid", False), ("terminal-exit-before-arm", False),
    ("terminal-live-boolean-bypass", False), ("terminal-wrong-session", False), ("terminal-wrong-begin-hash", False),
    ("terminal-late-append", False), ("terminal-timeout-overflow", False), ("terminal-missing-warning", False),
    ("terminal-old-schema", False), ("terminal-missing-lifecycle", False), ("terminal-no-confirmation", False),
    ("terminal-already-confirming-before-close", False), ("terminal-close-not-applied", False),
    ("terminal-confirm-before-input-failed", False), ("terminal-confirm-refused", False),
    ("terminal-confirm-wrong-scope", False), ("terminal-controller-not-armed", False),
    ("terminal-controller-hash-substitution", False), ("terminal-command-wrong-run", False),
    ("terminal-other-checkpoint", False), ("terminal-not-gui", False)]
png = b"\x89PNG\r\n\x1a\n"
for kind, data in [(b"IHDR", struct.pack(">IIBBBBB", 1, 1, 8, 2, 0, 0, 0)),
                   (b"IDAT", zlib.compress(b"\0\0\0\0")), (b"IEND", b"")]:
    png += struct.pack(">I", len(data)) + kind + data + struct.pack(">I", zlib.crc32(kind + data))
for label, expected in terminal_cases:
    root = output / label
    checkpoint = "F080.cancel-close" if label == "terminal-other-checkpoint" else "F080.confirm-close"
    reachability = "O" if label == "terminal-not-gui" else "Y"
    run_id, creation = "20261102-120000-000", 639000000000000000
    evidence, run, stage = root / "qa" / "passes" / "fixture", root / ".qa" / "run" / run_id, root / ".qa" / "stage" / "offline-stage"
    for directory in (evidence, run, stage):
        directory.mkdir(parents=True)
    (stage / "fractalengine.exe").write_bytes(b"offline placeholder, never executed")
    current = {"run_dir": str(run), "stage": str(stage), "stage_matches_source": True,
        "stage_manifest": {"source_commit": "7f51de19", "source_fingerprint": "a" * 64},
        "qa_options": {"synthetic_input": True}, "stdout_path": str(run / "stdout.log"),
        "api_base": "http://127.0.0.1:8882", "process_identity": {"pid": 42,
            "start_ticks": str(creation), "exe_path": str(stage / "fractalengine.exe")}}
    (run / "current.json").write_text(json.dumps(current), encoding="utf-8")
    prefix = b"2026-11-02T12:00:00.000000Z INFO qa_fixture: before close\n"
    level = "WARN" if label in {"terminal-reviewed-warning", "terminal-missing-warning"} else "INFO"
    raw = ("2026-11-02T12:00:01.000000Z " + level + " qa_fixture: synthetic shutdown\n").encode()
    source_path = run / "stdout.log"
    source_path.write_bytes(prefix + raw)
    with WindowsLease() as lease:
        file, binding = context(lease, root, "fixture", run_id, "F080.confirm-close", live=False, capture_kind="terminal_close")
        begin = {**binding, "schema": BEGIN_SCHEMA, "session_id": "a" * 32, "offset": len(prefix),
            "prefix_sha256": prefix_hash(file, len(prefix)), "armed_at_ticks": str(creation + 1), "timeout_seconds": 90}
        end = {**binding, "schema": END_SCHEMA, "session_id": "a" * 32, "offset": len(prefix + raw),
            "prefix_sha256": prefix_hash(file, len(prefix + raw)), "raw_window_sha256": digest(raw),
            "projection": window_projection(raw), "elapsed_ms": 1000,
            "exit": {"pid": 42, "creation_ticks": str(creation), "exit_ticks": str(creation + 10000001), "exit_code": 0}}
    if label == "terminal-nonzero-exit":
        end["exit"]["exit_code"] = 1
    elif label == "terminal-wrong-pid":
        end["exit"]["pid"] = 43
    elif label == "terminal-exit-before-arm":
        end["exit"]["exit_ticks"] = str(creation)
    elif label == "terminal-live-boolean-bypass":
        end["observed_live"] = True
    elif label == "terminal-wrong-session":
        end["session_id"] = "b" * 32
    elif label == "terminal-late-append":
        source_path.write_bytes(prefix + raw + b"2026-11-02T12:00:02.000000Z WARN qa_fixture: late bytes\n")
    elif label == "terminal-timeout-overflow":
        end["elapsed_ms"] = 95001
    elif label == "terminal-old-schema":
        end["schema"] = "fe-qa-owned-stdout-window-v1"
    begin_bytes = json.dumps(begin, sort_keys=True).encode()
    end["begin_receipt_sha256"] = "0" * 64 if label == "terminal-wrong-begin-hash" else digest(begin_bytes)
    nav = {"verse_id": "v", "fractal_id": "f", "petal_id": "p"}
    before_state = {"frame": 20, "navigation": nav, "open_windows": ["Inspector"]}
    confirmation = {"frame": 30, "navigation": nav, "open_windows": ["Close FractalEngine?"]}
    close_action = {"token_verse": "v", "commands": [{"kind": "pointer_click", "button": "left", "x": 20, "y": 20}],
        "locator": {"name": "Close FractalEngine", "role": "button", "geometry_source": "observed_hit_clip_window",
                    "visible_rect": {"x": 10, "y": 10, "w": 24, "h": 24}}, "outcome": {"commands_applied": 1}}
    confirm_action = {"not_running": True,
        "message": "API at http://127.0.0.1:8882/api/v1/debug/input did not answer: synthetic shutdown fixture"}
    confirm_exit = 3
    if label in {"terminal-confirm-ack", "terminal-confirm-wrong-scope"}:
        confirm_action, confirm_exit = copy.deepcopy(close_action), 0
        if label == "terminal-confirm-wrong-scope":
            confirm_action["token_verse"] = "foreign"
    elif label == "terminal-no-confirmation":
        confirmation["open_windows"] = []
    elif label == "terminal-already-confirming-before-close":
        before_state["open_windows"] = ["Close FractalEngine?"]
    elif label == "terminal-close-not-applied":
        close_action["outcome"]["commands_applied"] = 0
    elif label == "terminal-confirm-before-input-failed":
        confirm_action["message"] = "API at http://127.0.0.1:8882/api/v1/debug/camera did not answer: fixture"
    elif label == "terminal-confirm-refused":
        confirm_action, confirm_exit = {"refused": True, "code": "input_in_progress"}, 2
    lifecycle = {"schema": "fe-qa-terminal-close-lifecycle-v1", "pass_id": "fixture", "run_id": run_id,
        "checkpoint": "F080.confirm-close", "process_identity": current["process_identity"],
        "begin_receipt_sha256": digest(begin_bytes), "before_state_artifact": "qa/passes/fixture/before.json",
        "close_action_artifact": "qa/passes/fixture/close.json", "confirmation_state_artifact": "qa/passes/fixture/confirmation.json",
        "confirmation_png": "qa/passes/fixture/confirmation.png", "confirm_action_artifact": "qa/passes/fixture/confirm.json",
        "controller_artifact": "qa/passes/fixture/controller.jsonl",
        "close_command": ["--run", str(run), "click", "--name", "Close FractalEngine", "--role", "button"],
        "confirm_command": ["--run", str(run), "click", "--name", "Close FractalEngine", "--role", "button"],
        "confirm_exit_code": confirm_exit}
    controller = [{"phase": "armed", "checkpoint": "F080.confirm-close", "session_id": begin["session_id"],
        "begin_artifact": "qa/passes/fixture/begin.json", "sha256": digest(begin_bytes)},
        {"phase": "complete", "end_artifact": "qa/passes/fixture/end.json",
         "sha256": digest(json.dumps(end, sort_keys=True).encode()), "exit": end["exit"], "raw_bytes_emitted": False}]
    if label == "terminal-controller-not-armed":
        controller[0]["phase"] = "complete"
    elif label == "terminal-controller-hash-substitution":
        controller[1]["sha256"] = "0" * 64
    elif label == "terminal-command-wrong-run":
        lifecycle["confirm_command"][1] = str(root / ".qa" / "run" / "20261101-120000-000")
    artifacts = []
    for filename, value, kind in [("begin.json", begin, "tool"), ("end.json", end, "os" if reachability == "O" else "tool"),
            ("lifecycle.json", lifecycle, "tool"), ("before.json", before_state, "api"), ("close.json", close_action, "tool"),
            ("confirmation.json", confirmation, "api"), ("confirm.json", confirm_action, "tool"), ("confirmation.png", png, "png")]:
        data = value if isinstance(value, bytes) else json.dumps(value, sort_keys=True).encode()
        (evidence / filename).write_bytes(data)
        artifacts.append({"path": "qa/passes/fixture/" + filename, "kind": kind, "sha256": digest(data)})
    controller_bytes = "\n".join(json.dumps(value, sort_keys=True) for value in controller).encode()
    (evidence / "controller.jsonl").write_bytes(controller_bytes)
    artifacts.append({"path": "qa/passes/fixture/controller.jsonl", "kind": "tool", "sha256": digest(controller_bytes)})
    warnings = [{**entry, "expected": True, "reason": "synthetic offline shutdown warning"} for entry in end["projection"]["warnings"]]
    record = {"pass_id": "fixture", "run_id": run_id, "checkpoint": checkpoint, "outcome": "PASS",
        "stage_commit": "7f51de19", "stage_fingerprint": "a" * 64, "commands": ["offline validator fixture, not GUI evidence"],
        "assertions": [{"name": "fixture", "expected": "fixture", "actual": "fixture", "passed": True}],
        "visual_verdict": {"passed": True, "judged_by": "offline fixture", "notes": "synthetic one-pixel fixture; no app coverage"},
        "log_source": {"kind": "terminal_close_stdout", "begin_artifact": "qa/passes/fixture/begin.json",
            "end_artifact": "qa/passes/fixture/end.json", "lifecycle_artifact": "qa/passes/fixture/lifecycle.json"},
        "artifacts": artifacts, "new_logs": [] if label == "terminal-missing-warning" else warnings}
    if label == "terminal-missing-lifecycle":
        record["log_source"]["lifecycle_artifact"] = "qa/passes/fixture/missing.json"
    catalog = {"catalog_source_fingerprint": "a" * 64,
        "families": [{"id": "F080", "region": "R6", "requires_persistence": False}],
        "checkpoints": [{"id": checkpoint, "family": "F080", "reachability": reachability}], "gold_tasks": {"G6": [checkpoint]}}
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
        "limit": "synthetic final-audit data; retained live process/writer-release observation is not executed"})
# Controlled socket-owner interleavings exercise the production pre-send guard without networking.
from types import SimpleNamespace, MethodType
from owned_run_http import OwnedRun, PinnedConnection
for label, rows_before, rows_at_send, expected in [
        ("matching", [("127.0.0.1",8765,"127.0.0.1",51000,42)], None, True),
        ("pid-mismatch", [("127.0.0.1",8765,"127.0.0.1",51000,99)], None, False),
        ("rebind-before-auth", [("127.0.0.1",8765,"127.0.0.1",51000,42)], [("127.0.0.1",8765,"127.0.0.1",51000,99)], False),
        ("tuple-mismatch", [("127.0.0.1",8765,"127.0.0.1",51001,42)], None, False),
        ("ambiguous-owner", [("127.0.0.1",8765,"127.0.0.1",51000,42),("127.0.0.1",8765,"127.0.0.1",51000,99)], None, False)]:
    sent, reported = [], list(rows_before)
    owner=SimpleNamespace(host="127.0.0.1",port=8765,identity={"pid":42},files=SimpleNamespace(check_live=lambda:None),owners=SimpleNamespace(rows=lambda:reported))
    owner.check_live=MethodType(OwnedRun.check_live,owner)
    owner.prove_socket=MethodType(OwnedRun.prove_socket,owner)
    stream=SimpleNamespace(getsockname=lambda:("127.0.0.1",51000),getpeername=lambda:("127.0.0.1",8765),sendall=lambda data:sent.append(bytes(data)))
    connection=PinnedConnection(owner,1.0);connection.sock=stream
    accepted=True
    try:
        if rows_at_send is not None:
            owner.prove_socket(stream)
            reported=rows_at_send
        connection.send(b"Authorization: Bearer offline-fixture-only\r\n")
    except ValueError:
        accepted=False
    results.append({"case":"owned-api-"+label,"expected_accepted":expected,"accepted":accepted,
                    "passed":accepted==expected and len(sent)==(1 if expected else 0),
                    "authenticated_writes":len(sent),
                    "boundary":"production retained-stream pre-send owner guard; controlled OS rows, no network/runtime acceptance"})
import owned_run_http as snapshot_module
from unittest.mock import patch
for label, raw, expected in [
        ("bounded-version", b'{"tokens":{},"roles":{}}', True),
        ("growth-after-size", b'{"tokens":{},"roles":{}}', True),
        ("inplace-writer-held", b'{"tokens":{},"roles":{}}', False),
        ("inplace-mutation-attempt", b'{"tokens":{},"roles":{}}', True),
        ("oversized", b'{"tokens":{},"roles":{}}', False),
        ("duplicate-key", b'{"tokens":{},"tokens":{},"roles":{}}', False),
        ("nonfinite", b'{"tokens":{},"roles":{},"extra":NaN}', False),
        ("short-read", b'{"tokens":{},"roles":{}}', False)]:
    snapshot_path = output / "unwritten-token-snapshot.json"
    observed = {"shares": [], "sizes": 0, "reads": [], "closed": 0, "writer_denied": False}

    def open_snapshot(path, access, share, security, disposition, flags, template):
        observed["shares"].append(share)
        return None if label == "inplace-writer-held" and not share & 2 else 1

    def resolve_snapshot(handle, buffer, size, flags):
        buffer.value = str(snapshot_path)
        return len(buffer.value)

    def snapshot_size():
        observed["sizes"] += 1
        if label == "oversized" or label == "growth-after-size" and observed["sizes"] > 1:
            return snapshot_module.MAX_TOKEN_BYTES + 1
        return len(raw)

    def snapshot_read(offset, length):
        observed["reads"].append([offset, length])
        if label == "inplace-mutation-attempt":
            observed["writer_denied"] = not observed["shares"][0] & 2
            if not observed["writer_denied"]:
                raise ValueError("controlled concurrent writer was not excluded")
        return raw[:-1] if label == "short-read" else raw

    files = SimpleNamespace(api=SimpleNamespace(
        CreateFileW=open_snapshot, GetFinalPathNameByHandleW=resolve_snapshot,
        CloseHandle=lambda handle: observed.__setitem__("closed", observed["closed"] + 1)))
    pinned = SimpleNamespace(information=lambda: SimpleNamespace(attributes=0),
                             size=snapshot_size, read=snapshot_read)
    accepted = True
    with patch.object(snapshot_module, "PinnedFile", lambda files, handle: pinned), \
            patch.object(snapshot_module, "canonical", lambda value: Path(value)):
        try:
            value = snapshot_module.token_snapshot(files, snapshot_path)
            accepted = value == {"tokens": {}, "roles": {}}
        except ValueError:
            accepted = False
    contract = observed["shares"] == [5] and observed["sizes"] <= 1
    contract = contract and all(length <= snapshot_module.MAX_TOKEN_BYTES for _, length in observed["reads"])
    contract = contract and observed["closed"] == (0 if label == "inplace-writer-held" else 1)
    if label == "growth-after-size":
        contract = contract and observed["reads"] == [[0, len(raw)]]
    if label == "inplace-mutation-attempt":
        contract = contract and observed["writer_denied"]
    results.append({"case": "owned-token-" + label, "expected_accepted": expected, "accepted": accepted,
                    "passed": accepted == expected and contract, "contract": observed,
                    "boundary": "production snapshot with controlled kernel sharing/read seams; no token file or native API execution"})
summary = {"scope": "offline synthetic validator fixtures; zero application checkpoint coverage",
           "passed": all(result["passed"] for result in results), "cases": results}
(output / "results.json").write_text(json.dumps(summary, indent=2), encoding="utf-8")
print(json.dumps(summary, indent=2))
sys.exit(0 if summary["passed"] else 2)
