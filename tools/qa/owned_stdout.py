"""Capture and audit bounded input-off OS log windows; see AGENTS.md."""

import argparse
import hashlib
import json
from pathlib import Path
import re
import sys

from owned_stdout_windows import WindowsLease, canonical

MAX_PREFIX = 64 * 1024 * 1024
MAX_WINDOW = 1024 * 1024
MAX_RECEIPT = 4 * 1024 * 1024
MESSAGE_WITHHELD = "[message withheld: stdout event scope unavailable]"
ANSI = re.compile(r"\x1b\[[0-?]*[ -/]*[@-~]")
LOG_LINE = re.compile(r"^\d{4}-\d\d-\d\dT\S+\s+(TRACE|DEBUG|INFO|WARN|ERROR)\s+[^\s:]+(?:::[^\s:]+)*:\s.*$")
OS_CHECKPOINTS = {"F114.owned-hwnd-uia", "F114.named-buttons", "F114.finite-bounds", "F114.input-debug-denied",
                  "F115.inactive-baseline", "F115.active-provider", "F115.same-cadence",
                  "F115.frame-sample-summary", "F115.incomplete-baseline-refusal"}


def require(condition, message):
    if not condition:
        raise ValueError(message)


def digest(data):
    return hashlib.sha256(data).hexdigest()


def json_object(data):
    from coverage import read_json
    try:
        value = read_json(data.decode("utf-8-sig"))
    except (ValueError, UnicodeError):
        raise ValueError("owned log receipt is not strict UTF-8 JSON") from None
    require(isinstance(value, dict), "owned log receipt must be an object")
    return value


def read_object(file):
    size = file.size()
    require(size <= MAX_RECEIPT, "owned log receipt exceeds its bound")
    return json_object(file.read(0, size))


def prefix_hash(file, size):
    require(type(size) is int and 0 <= size <= MAX_PREFIX, "stdout prefix exceeds 64 MiB or has invalid offset")
    result = hashlib.sha256()
    for offset in range(0, size, 65536):
        result.update(file.read(offset, min(65536, size - offset)))
    return result.hexdigest()


def window_projection(raw):
    require(len(raw) <= MAX_WINDOW and (not raw or raw.endswith(b"\n")),
            "stdout window exceeds 1 MiB or ends with a partial line")
    try:
        text = raw.decode("utf-8")
    except UnicodeError:
        raise ValueError("stdout window has invalid UTF-8") from None
    lines, inventory = [], []
    for index, original in enumerate(text.splitlines(), 1):
        line = ANSI.sub("", original).strip()
        if not line:
            continue
        match = LOG_LINE.fullmatch(line)
        require(match is not None, "stdout window has an unclassified line; completeness unknown")
        entry = {"line": index, "level": match[1].lower(), "raw_line_sha256": digest(original.encode("utf-8")),
                 "message": MESSAGE_WITHHELD}
        lines.append(entry)
        if entry["level"] in {"warn", "error"}:
            inventory.append(entry)
    return {"lines": lines, "warnings": inventory, "redaction": "all messages and targets withheld; scope unavailable"}


def context(lease, root, pass_id, run_id, checkpoint, live):
    root = canonical(root)
    require(bool(re.fullmatch(r"[A-Za-z0-9][A-Za-z0-9_-]{0,63}", pass_id)), "invalid pass ID")
    require(bool(re.fullmatch(r"\d{8}-\d{6}-\d{3}", run_id)), "owned stdout requires an exact QA run timestamp")
    require(checkpoint in OS_CHECKPOINTS, "owned stdout is limited to current input-off OS checkpoints")
    run = root / ".qa" / "run" / run_id
    current = read_object(lease.file(run / "current.json"))
    require(isinstance(current.get("qa_options"), dict) and current["qa_options"].get("synthetic_input") is False,
            "owned stdout OS accounting requires synthetic input off")
    require(canonical(current.get("run_dir", "")) == run, "current receipt belongs to another run")
    identity = current.get("process_identity")
    require(isinstance(identity, dict) and set(identity) == {"pid", "start_ticks", "exe_path"},
            "current receipt lacks an exact process identity")
    require(type(identity["pid"]) is int and 0 < identity["pid"] <= 0xFFFFFFFF
            and isinstance(identity["start_ticks"], str) and identity["start_ticks"].isdigit(),
            "invalid owned process identity")
    stage = canonical(current.get("stage", ""))
    require(stage.parent == root / ".qa" / "stage"
            and canonical(identity["exe_path"]) == stage / "fractalengine.exe", "owned executable is not its recorded stage")
    manifest = current.get("stage_manifest")
    require(isinstance(manifest, dict) and current.get("stage_matches_source") is True
            and isinstance(manifest.get("source_commit"), str)
            and re.fullmatch(r"[a-fA-F0-9]{7,40}", manifest["source_commit"])
            and isinstance(manifest.get("source_fingerprint"), str)
            and re.fullmatch(r"[a-fA-F0-9]{64}", manifest["source_fingerprint"]),
            "owned stdout needs recorded current-stage provenance")
    source = canonical(current.get("stdout_path", ""))
    relative = source.relative_to(run)
    if relative.parts != ("stdout.log",):
        require(len(relative.parts) == 3 and relative.parts[0] == "starts" and relative.parts[2] == "stdout.log"
                and re.fullmatch(r"\d{8}-\d{6}-\d{3}-[a-f0-9]{32}", relative.parts[1]),
                "stdout is outside the owned start's canonical log location")
        require(isinstance(current.get("restart"), dict), "missing owned restart receipt")
        attempt = canonical(current["restart"].get("attempt", ""))
        require(attempt.parent == source.parent, "stdout does not belong to the current restart attempt")
    else:
        require(not current.get("restart"), "restarted app cannot use the original lifetime's stdout")
    if live:
        lease.live_process(identity)
    file = lease.file(source, active_writer=True)
    binding = {"pass_id": pass_id, "run_id": run_id, "checkpoint": checkpoint,
        "process_identity": identity, "source_path": str(source.relative_to(root)),
        "source_identity": file.identity(), "stage_commit": manifest["source_commit"],
        "stage_fingerprint": manifest["source_fingerprint"], "synthetic_input": False}
    return file, binding


def artifact_path(root, pass_id, name):
    require(isinstance(name, str) and bool(re.fullmatch(r"[A-Za-z0-9][A-Za-z0-9_.-]{0,120}\.json", name)),
            "log artifact must be a plain JSON filename")
    return canonical(root / "qa" / "passes" / pass_id / name)


def write_receipt(lease, path, value):
    data = json.dumps(value, sort_keys=True, allow_nan=False).encode("utf-8")
    require(len(data) <= MAX_RECEIPT, "redacted receipt exceeds 4 MiB")
    lease.check_live()
    lease.file(path, create=True).write(data)
    lease.check_live()
    return digest(data)


def verify_window(file, binding, begin, end):
    require(set(begin) == set(binding) | {"schema", "observed_live", "offset", "prefix_sha256"}
            and set(end) == set(binding) | {"schema", "observed_live", "offset", "prefix_sha256",
                "begin_receipt_sha256", "raw_window_sha256", "projection"}, "unknown or missing stdout receipt fields")
    require(begin.get("schema") == "fe-qa-owned-stdout-begin-v1"
            and end.get("schema") == "fe-qa-owned-stdout-window-v1", "unsupported owned stdout schema")
    for key, value in binding.items():
        require(begin.get(key) == value and end.get(key) == value, "owned stdout lifetime or provenance changed")
    require(begin.get("observed_live") is True and end.get("observed_live") is True,
            "owned stdout requires live begin and end observations")
    start, stop = begin.get("offset"), end.get("offset")
    require(type(start) is int and type(stop) is int and 0 <= start <= stop <= MAX_PREFIX
            and stop - start <= MAX_WINDOW and file.size() >= stop, "stdout window truncated, regressed or oversized")
    require(prefix_hash(file, start) == begin.get("prefix_sha256")
            and prefix_hash(file, stop) == end.get("prefix_sha256"), "stdout prefix was modified or replaced")
    require(start == 0 or file.read(start - 1, 1) == b"\n", "stdout begin offset splits a line")
    raw = file.read(start, stop - start)
    projection = window_projection(raw)
    require(digest(raw) == end.get("raw_window_sha256") and end.get("projection") == projection,
            "stdout bytes or redacted inventory disagree with the source")
    return projection["warnings"]


def validate_record(root, checkpoint, record):
    require(checkpoint["reachability"] == "O", "owned stdout cannot replace ordinary GUI/API ring accounting")
    source = record.get("log_source")
    require(isinstance(source, dict) and set(source) == {"kind", "begin_artifact", "end_artifact"}
            and source["kind"] == "owned_stdout", "invalid owned stdout evidence references")
    require(not any(key in record for key in ("log_watermark", "log_after", "generation_changed", "retention",
            "log_exception", "new_logs_unavailable")), "owned stdout cannot conceal a failed or mixed ring window")
    with WindowsLease() as lease:
        file, binding = context(lease, root, record["pass_id"], record["run_id"], checkpoint["id"], live=False)
        receipts, contents = [], []
        for key in ("begin_artifact", "end_artifact"):
            relative = Path(source[key])
            require(relative.parent == Path("qa") / "passes" / record["pass_id"], "stdout artifact is outside the exact pass")
            path = artifact_path(Path(root), record["pass_id"], relative.name)
            declared = [item for item in record["artifacts"] if item["path"] == source[key]
                        and item["kind"] in {"tool", "os"}]
            require(len(declared) == 1, "stdout receipts need unique declared tool/OS artifacts")
            pinned = lease.file(path)
            require(pinned.size() <= MAX_RECEIPT, "stdout receipt is oversized")
            raw = pinned.read(0, pinned.size())
            require(digest(raw).lower() == declared[0]["sha256"].lower(), "stdout receipt hash changed")
            receipts.append(json_object(raw))
            contents.append(raw)
        begin, end = receipts
        require(end.get("begin_receipt_sha256") == digest(contents[0]), "stdout end does not bind its begin receipt")
        require(binding["stage_commit"].lower() == record["stage_commit"].lower()
                and binding["stage_fingerprint"].lower() == record["stage_fingerprint"].lower(),
                "stdout evidence belongs to another stage")
        warnings = verify_window(file, binding, begin, end)
        reported = record.get("new_logs")
        require(isinstance(reported, list) and len(reported) == len(warnings), "stdout warning inventory is incomplete")
        for expected, actual in zip(warnings, reported):
            require(isinstance(actual, dict) and set(actual) == set(expected) | {"expected", "reason"}
                    and all(actual.get(key) == value for key, value in expected.items())
                    and actual.get("expected") is True and isinstance(actual.get("reason"), str) and actual["reason"].strip(),
                    "stdout warning/error needs exact inventory and independent reconciliation")


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("operation", choices=("begin", "end"))
    parser.add_argument("--root", default=str(Path(__file__).absolute().parents[2]))
    parser.add_argument("--run-id", required=True)
    parser.add_argument("--pass-id", required=True)
    parser.add_argument("--checkpoint", required=True)
    parser.add_argument("--out", required=True, help="new JSON filename in the existing pass directory")
    parser.add_argument("--begin", help="begin receipt filename, required for end")
    args = parser.parse_args(argv)
    try:
        require(bool(args.begin) == (args.operation == "end"), "only end requires a begin receipt")
        root = canonical(args.root)
        with WindowsLease() as lease:
            file, binding = context(lease, root, args.pass_id, args.run_id, args.checkpoint, live=True)
            output = artifact_path(root, args.pass_id, args.out)
            size = file.size()
            receipt = {**binding, "observed_live": True, "offset": size, "prefix_sha256": prefix_hash(file, size)}
            if args.operation == "begin":
                require(size == 0 or file.read(size - 1, 1) == b"\n", "stdout begin would split a line")
                receipt["schema"] = "fe-qa-owned-stdout-begin-v1"
            else:
                begin_file = lease.file(artifact_path(root, args.pass_id, args.begin))
                require(begin_file.size() <= MAX_RECEIPT, "stdout begin receipt is oversized")
                begin_bytes = begin_file.read(0, begin_file.size())
                begin = json_object(begin_bytes)
                start = begin.get("offset")
                require(type(start) is int and 0 <= start <= size and size - start <= MAX_WINDOW,
                        "stdout end window regressed or exceeds its bound")
                raw = file.read(start, size - start)
                receipt.update(schema="fe-qa-owned-stdout-window-v1", begin_receipt_sha256=digest(begin_bytes),
                    raw_window_sha256=digest(raw), projection=window_projection(raw))
                verify_window(file, binding, begin, receipt)
            lease.check_live()
            artifact_hash = write_receipt(lease, output, receipt)
        print(json.dumps({"artifact": str(output.relative_to(root)), "sha256": artifact_hash,
                          "operation": args.operation, "raw_bytes_emitted": False}))
        return 0
    except (OSError, ValueError, TypeError, KeyError, AttributeError):
        print(json.dumps({"error": "owned_stdout_refused", "reason": "owned lifetime, path, byte window or evidence is unavailable/invalid"}))
        return 2


if __name__ == "__main__":
    sys.exit(main())
