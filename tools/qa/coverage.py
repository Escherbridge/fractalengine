"""Summarize one pass's checkpoint evidence; see tools/qa/AGENTS.md."""

import argparse
import hashlib
import json
from pathlib import Path
import re
import sys


OUTCOMES = {"PASS", "FAIL", "BLOCKED", "UNKNOWN", "NOT_RUN"}
BUCKETS = {"Y": "GUI", "A": "API", "O": "OS", "P": "planned", "U": "unreachable"}
LOG_RING_CAPACITY = 2000
GATE_OFF_LOG_CASES = {"F112.gate-off": "A", "F114.input-debug-denied": "O"}


def object_without_duplicates(pairs):
    result = {}
    for key, value in pairs:
        if key in result:
            raise ValueError("duplicate JSON key: " + key)
        result[key] = value
    return result


def reject_nonfinite(value):
    raise ValueError("nonfinite JSON number: " + value)


def read_json(text):
    return json.loads(text, object_pairs_hook=object_without_duplicates, parse_constant=reject_nonfinite)


def require(condition, message):
    if not condition:
        raise ValueError(message)


def nonempty(value):
    return isinstance(value, str) and bool(value.strip())


def artifact(root, evidence_dir, item):
    require(isinstance(item, dict), "artifact must be an object")
    require(nonempty(item.get("path")), "artifact needs a path")
    relative = Path(item["path"])
    require(not relative.is_absolute() and not relative.drive and ".." not in relative.parts,
            "artifact path must be relative to the evidence root")
    require(item.get("kind") in {"png", "api", "persistence", "os", "tool"}, "invalid artifact kind")
    path = (root / relative).resolve()
    require(path.is_relative_to(evidence_dir), "artifact is outside this pass's evidence directory")
    require(path.is_file(), "artifact is missing: " + item["path"])
    expected = item.get("sha256")
    require(isinstance(expected, str) and re.fullmatch(r"[a-fA-F0-9]{64}", expected),
            "artifact needs a SHA256")
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        header = handle.read(8)
        digest.update(header)
        for block in iter(lambda: handle.read(1024 * 1024), b""):
            digest.update(block)
    require(digest.hexdigest().lower() == expected.lower(), "artifact SHA256 differs: " + item["path"])
    if item.get("kind") == "png":
        require(header == b"\x89PNG\r\n\x1a\n", "visual artifact is not a PNG")
    return item.get("kind")


def validate_context(checkpoint, record, args):
    name = checkpoint["id"]
    require(record.get("pass_id") == args.pass_id, name + ": record belongs to another pass")
    require(record.get("run_id") in args.run_id, name + ": record does not belong to a declared owned run")
    for key, pattern in (("stage_commit", r"[a-fA-F0-9]{7,40}"), ("stage_fingerprint", r"[a-fA-F0-9]{64}")):
        value = record.get(key)
        if value is None:
            require(record["outcome"] != "PASS", name + ": missing " + key)
            continue
        require(isinstance(value, str) and re.fullmatch(pattern, value), name + ": invalid " + key)
        expected = getattr(args, key)
        if expected:
            require(value.lower() == expected.lower(), name + ": " + key + " differs from this pass")


def evidence_object(root, record, path, kinds):
    require(nonempty(path), "log exception needs an evidence path")
    require(any(item["path"] == path and item["kind"] in kinds for item in record["artifacts"]),
            "log exception must reference a declared, hash-validated artifact")
    source = root / path
    require(source.stat().st_size <= 65536, "log exception evidence exceeds 64 KiB")
    value = read_json(source.read_text(encoding="utf-8-sig"))
    require(isinstance(value, dict), "log exception evidence must be an object")
    for key in ("pass_id", "run_id", "checkpoint"):
        require(value.get(key) == record.get(key), "log exception evidence has foreign " + key)
    return value


def validate_gate_off_logs(root, checkpoint, record):
    name = checkpoint["id"]
    require(GATE_OFF_LOG_CASES.get(name) == checkpoint["reachability"],
            name + ": this checkpoint cannot use a gate-off log exception")
    require(record.get("log_watermark") == {"unavailable_reason": "input_disabled"}
            and "log_after" not in record and "retention" not in record and record.get("new_logs") == [],
            name + ": gate-off exception cannot replace an incomplete ordinary log window")
    exception = record.get("log_exception")
    require(isinstance(exception, dict) and set(exception) == {"kind", "refusal_artifact", "no_effect_artifact"}
            and exception["kind"] == "expected_input_disabled",
            name + ": structured gate-off refusal and no-effect evidence required")
    refusal = evidence_object(root, record, exception["refusal_artifact"], {"api", "tool"})
    require(refusal.get("schema") == "fe-qa-gate-off-refusal-v1"
            and refusal.get("request") == {"method": "GET", "path": "/api/v1/debug/logs"}
            and refusal.get("response") == {"status": 403, "body": {
                "ok": False, "code": "qa_gate_refused",
                "error": "QA diagnostics endpoint is disabled on this node"}},
            name + ": diagnostic refusal does not match the gate-off REST contract")
    proof = evidence_object(root, record, exception["no_effect_artifact"], {"api", "tool", "os"})
    require(proof.get("schema") == "fe-qa-gate-off-no-effect-v1"
            and isinstance(proof.get("request"), dict)
            and proof.get("request", {}).get("method") == "POST"
            and proof.get("request", {}).get("path") == "/api/v1/debug/input"
            and proof["request"].get("body") == {"commands": [
                {"kind": "pointer_move", "x": 0, "y": 0}], "wait_frames": 0}
            and proof.get("response") == {"status": 403, "body": {
                "ok": False, "error": "input injection endpoint is disabled on this node"}},
            name + ": input must be refused by the pre-admission gate")
    before, after = proof.get("before"), proof.get("after")
    require(isinstance(before, dict) and before == after
            and all(isinstance(before.get(key), dict) and before[key] for key in ("camera", "hierarchy")),
            name + ": independent unchanged camera/hierarchy readbacks required")


def validate_logs(root, checkpoint, record):
    if "log_source" in record:
        from owned_stdout import validate_record
        validate_record(root, checkpoint, record)
        return
    name = checkpoint["id"]
    require("new_logs_unavailable" not in record,
            name + ": unavailable post-action logs cannot PASS")
    require(record.get("generation_changed", False) is False,
            name + ": log generation changed")
    retention = record.get("retention")
    if "retention" in record:
        require(isinstance(retention, dict) and retention.get("possible_gap") is False,
                name + ": explicit or unknown retention gap cannot PASS")
    watermark = record.get("log_watermark")
    if isinstance(watermark, dict) and "unavailable_reason" in watermark:
        validate_gate_off_logs(root, checkpoint, record)
        return
    require("log_exception" not in record, name + ": ordinary log window cannot use an exception")
    after = record.get("log_after")
    for label, counters in (("before", watermark), ("after", after)):
        require(isinstance(counters, dict)
                and all(type(counters.get(key)) is int and counters[key] >= 0
                        for key in ("generation", "seq")),
                name + ": missing or invalid " + label + " log generation/sequence")
        require("unavailable_reason" not in counters, name + ": unavailable log window")
    delta = after["seq"] - watermark["seq"]
    require(after["generation"] == watermark["generation"], name + ": log generation changed")
    require(0 <= delta <= LOG_RING_CAPACITY, name + ": log window regressed or may have evicted entries")
    if retention is not None:
        require(type(retention.get("seq_delta")) is int and retention["seq_delta"] == delta
                and retention.get("source_derived_capacity") == LOG_RING_CAPACITY,
                name + ": retention counters disagree with the actual window")
    require(isinstance(record.get("new_logs"), list), name + ": missing new-log inventory")
    for entry in record["new_logs"]:
        require(isinstance(entry, dict) and entry.get("expected") is True and nonempty(entry.get("reason")),
                name + ": unreviewed warning/error must not PASS")


def validate_pass(root, evidence_dir, checkpoint, record):
    name = checkpoint["id"]
    require(checkpoint["reachability"] in "YAO", name + ": excluded checkpoint cannot PASS")
    commands = record.get("commands")
    require(isinstance(commands, list) and commands and all(nonempty(x) for x in commands),
            name + ": commands must record actual actions")
    assertions = record.get("assertions")
    require(isinstance(assertions, list) and assertions, name + ": missing assertions")
    for assertion in assertions:
        require(isinstance(assertion, dict) and nonempty(assertion.get("name"))
                and "expected" in assertion and "actual" in assertion and assertion.get("passed") is True,
                name + ": every assertion needs name/expected/actual/passed=true")
    artifacts = record.get("artifacts")
    require(isinstance(artifacts, list) and artifacts, name + ": missing captured artifacts")
    kinds = {artifact(root, evidence_dir, item) for item in artifacts}
    if checkpoint["reachability"] == "Y":
        require("png" in kinds, name + ": missing captured PNG")
        judgment = record.get("visual_verdict")
        require(isinstance(judgment, dict) and judgment.get("passed") is True
                and nonempty(judgment.get("judged_by")) and nonempty(judgment.get("notes")),
                name + ": missing explicit visual judgment")
    if checkpoint.get("requires_persistence"):
        require("persistence" in kinds, name + ": missing durable readback artifact")
    if checkpoint["reachability"] == "A":
        require(bool(kinds & {"api", "tool"}), name + ": missing API/tool response artifact")
    if checkpoint["reachability"] == "O":
        require("os" in kinds, name + ": missing OS/timing report artifact")
    validate_logs(root, checkpoint, record)


def summarize(args):
    root = Path(args.root).resolve()
    require(bool(re.fullmatch(r"[a-zA-Z0-9][a-zA-Z0-9_-]{0,63}", args.pass_id)), "invalid pass ID")
    require(all(nonempty(run) for run in args.run_id) and len(set(args.run_id)) == len(args.run_id),
            "owned run IDs must be nonempty and unique")
    pass_path = root / "qa" / "passes" / args.pass_id
    evidence_dir = pass_path.resolve()
    require(evidence_dir.is_relative_to(root) and evidence_dir == pass_path.absolute(),
            "pass evidence directory must not be redirected")
    ledger = Path(args.ledger).resolve()
    require(ledger.is_relative_to(evidence_dir), "ledger is outside this pass's evidence directory")
    catalog = read_json(Path(args.catalog).read_text(encoding="utf-8-sig"))
    require(isinstance(catalog, dict) and isinstance(catalog.get("checkpoints"), list), "invalid catalog")
    require(isinstance(catalog.get("families"), list), "missing source families")
    families = {}
    for family in catalog["families"]:
        require(isinstance(family, dict) and nonempty(family.get("id")), "invalid source family")
        require(family["id"] not in families, "duplicate source family")
        require(nonempty(family.get("region")) and type(family.get("requires_persistence")) is bool,
                "invalid family classification")
        families[family["id"]] = family
    if args.require_complete:
        reviewed = catalog.get("catalog_source_fingerprint")
        require(isinstance(reviewed, str) and re.fullmatch(r"[a-fA-F0-9]{64}", reviewed)
                and reviewed.lower() == args.stage_fingerprint.lower(),
                "catalog needs source reconciliation for the expected stage fingerprint")
    checkpoints = {}
    for item in catalog["checkpoints"]:
        require(isinstance(item, dict) and nonempty(item.get("id")), "invalid checkpoint")
        require(set(item) == {"id", "family", "reachability"}, "checkpoint may not override source-family fields")
        require(item["id"] not in checkpoints, "duplicate catalog ID: " + item["id"])
        require(item.get("family") in families and item.get("reachability") in BUCKETS, "invalid classification")
        require(item["id"].startswith(item["family"] + "."), "checkpoint ID does not match its family")
        checkpoints[item["id"]] = {**families[item["family"]], **item}
    records = {}
    for line_number, line in enumerate(ledger.read_text(encoding="utf-8-sig").splitlines(), 1):
        if not line.strip():
            continue
        record = read_json(line)
        require(isinstance(record, dict), "ledger line must be an object")
        name = record.get("checkpoint")
        require(isinstance(name, str) and name in checkpoints, "unknown checkpoint at line " + str(line_number))
        require(name not in records, "duplicate ledger checkpoint: " + name)
        require(record.get("outcome") in OUTCOMES, name + ": invalid outcome")
        validate_context(checkpoints[name], record, args)
        if record["outcome"] == "PASS":
            validate_pass(root, evidence_dir, checkpoints[name], record)
        else:
            require(nonempty(record.get("reason")), name + ": non-PASS needs a reason")
            if "artifacts" in record:
                require(isinstance(record["artifacts"], list), name + ": artifacts must be an array")
                for item in record["artifacts"]:
                    artifact(root, evidence_dir, item)
        records[name] = record
    for key in ("stage_commit", "stage_fingerprint"):
        require(len({r[key].lower() for r in records.values() if r.get(key) is not None}) <= 1,
                "one pass cannot combine different " + key)
    provenances = {(r["stage_commit"].lower(), r["stage_fingerprint"].lower())
                   for r in records.values() if r.get("stage_commit") and r.get("stage_fingerprint")}
    require(len(provenances) <= 1, "one pass cannot combine different stage provenance")
    regions = {}
    uncovered = []
    for name, item in checkpoints.items():
        bucket = BUCKETS[item["reachability"]]
        counts = regions.setdefault(item["region"], {}).setdefault(bucket, {"passed": 0, "total": 0})
        counts["total"] += 1
        passed = records.get(name, {}).get("outcome") == "PASS"
        counts["passed"] += int(passed)
        if item["reachability"] in "YAO" and not passed:
            uncovered.append({"checkpoint": name, "outcome": records.get(name, {}).get("outcome", "NOT_RUN")})
    gold = {}
    require(isinstance(catalog.get("gold_tasks"), dict) and catalog["gold_tasks"], "missing gold task map")
    for task, names in catalog["gold_tasks"].items():
        require(isinstance(names, list) and names and all(name in checkpoints for name in names), "invalid gold task")
        gold[task] = all(records.get(name, {}).get("outcome") == "PASS" for name in names)
    evidence_complete = not uncovered and all(gold.values())
    reviewed = catalog.get("catalog_source_fingerprint")
    catalog_matches = (isinstance(reviewed, str) and bool(re.fullmatch(r"[a-fA-F0-9]{64}", reviewed))
                       and bool(provenances) and all(fp == reviewed.lower() for _, fp in provenances))
    complete = evidence_complete and catalog_matches
    return {"pass_id": args.pass_id, "owned_run_ids": args.run_id,
            "complete": complete, "evidence_complete": evidence_complete, "catalog_matches_evidence": catalog_matches,
            "expected_provenance_checked": bool(args.stage_commit and args.stage_fingerprint),
            "regions": regions, "gold_tasks": gold, "uncovered": uncovered,
            "catalog_source_fingerprint": catalog.get("catalog_source_fingerprint"),
            "provenance": [dict(zip(("stage_commit", "stage_fingerprint"), values)) for values in sorted(provenances)],
            "limits": "Validates reported evidence and artifact integrity; does not judge pixels or prove assertions true."}


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--catalog", default="qa/feature-checkpoints.json")
    parser.add_argument("--ledger", required=True)
    parser.add_argument("--pass-id", required=True)
    parser.add_argument("--run-id", action="append", required=True)
    parser.add_argument("--root", default=str(Path(__file__).resolve().parents[2]))
    parser.add_argument("--stage-commit")
    parser.add_argument("--stage-fingerprint")
    parser.add_argument("--require-complete", action="store_true")
    args = parser.parse_args(argv)
    try:
        require(not args.require_complete or (args.stage_commit and args.stage_fingerprint),
                "--require-complete needs the expected stage commit and source fingerprint")
        result = summarize(args)
        print(json.dumps(result, indent=2))
        return 2 if args.require_complete and not result["complete"] else 0
    except (OSError, ValueError, TypeError, KeyError) as error:
        print(json.dumps({"error": "invalid_coverage_evidence", "message": str(error)}))
        return 2


if __name__ == "__main__":
    sys.exit(main())
