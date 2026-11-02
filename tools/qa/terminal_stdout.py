"""Retain the owned root through one F080 close observation; see AGENTS.md."""

import argparse
import json
import math
from pathlib import Path
import re
import secrets
import sys
import time

from coverage import read_json
from owned_stdout import (MAX_PREFIX, MAX_RECEIPT, MAX_WINDOW, TERMINAL_CHECKPOINT, artifact_path, context,
    declared_snapshot, digest, json_object, prefix_hash, reconcile_warnings,
    record_receipts, require, verify_byte_window, window_projection, write_receipt)
from owned_stdout_windows import WindowsLease, canonical

MAX_WAIT_SECONDS = 180
DRAIN_SECONDS = 5
BEGIN_SCHEMA = "fe-qa-terminal-stdout-begin-v1"
END_SCHEMA = "fe-qa-terminal-stdout-window-v1"


def verify_terminal(file, binding, begin, end):
    require(set(begin) == set(binding) | {"schema", "session_id", "offset", "prefix_sha256",
                "armed_at_ticks", "timeout_seconds"}
            and set(end) == set(binding) | {"schema", "session_id", "offset", "prefix_sha256",
                "begin_receipt_sha256", "raw_window_sha256", "projection", "exit", "elapsed_ms"},
            "unknown or missing terminal receipt fields")
    require(begin["schema"] == BEGIN_SCHEMA and end["schema"] == END_SCHEMA,
            "terminal close requires its distinct capture schema")
    for key, value in binding.items():
        require(begin.get(key) == value and end.get(key) == value, "terminal stdout lifetime or stage changed")
    session = begin["session_id"]
    require(isinstance(session, str) and re.fullmatch(r"[a-f0-9]{32}", session)
            and end["session_id"] == session, "terminal capture sessions do not match")
    timeout, elapsed = begin["timeout_seconds"], end["elapsed_ms"]
    require(type(timeout) is int and 1 <= timeout <= MAX_WAIT_SECONDS
            and type(elapsed) is int and 0 <= elapsed <= (timeout + DRAIN_SECONDS) * 1000,
            "terminal observation exceeded its bounded deadline")
    exit_info = end["exit"]
    require(isinstance(exit_info, dict) and set(exit_info) == {"pid", "creation_ticks", "exit_ticks", "exit_code"}
            and type(exit_info["pid"]) is int and exit_info["pid"] == binding["process_identity"]["pid"]
            and exit_info["creation_ticks"] == binding["process_identity"]["start_ticks"]
            and type(exit_info["exit_code"]) is int and exit_info["exit_code"] == 0,
            "terminal proof requires native clean exit facts for the original root")
    for ticks in (begin["armed_at_ticks"], exit_info["creation_ticks"], exit_info["exit_ticks"]):
        require(isinstance(ticks, str) and re.fullmatch(r"[0-9]{1,19}", ticks), "invalid terminal timestamp")
    require(int(exit_info["creation_ticks"]) <= int(begin["armed_at_ticks"]) < int(exit_info["exit_ticks"]),
            "root exit must follow the live capture arm point")
    require(type(end["offset"]) is int and file.size() == end["offset"],
            "terminal stdout has additional bytes beyond its final EOF")
    return verify_byte_window(file, begin, end)


def jsonl_artifact(lease, root, record, path):
    raw = declared_snapshot(lease, root, record, path, {"api", "tool"})
    lines = [line for line in raw.decode("utf-8-sig").splitlines() if line.strip()]
    require(1 <= len(lines) <= 8, "terminal action evidence must contain bounded actual JSON objects")
    values = [read_json(line) for line in lines]
    require(all(isinstance(value, dict) for value in values), "terminal action evidence is not object-shaped")
    return values


def click_result(value):
    locator, commands, outcome = value.get("locator"), value.get("commands"), value.get("outcome")
    require(isinstance(locator, dict) and locator.get("name") == "Close FractalEngine"
            and locator.get("role") == "button" and locator.get("geometry_source") == "observed_hit_clip_window"
            and isinstance(commands, list) and len(commands) == 1 and isinstance(commands[0], dict)
            and commands[0].get("kind") == "pointer_click" and commands[0].get("button") == "left"
            and isinstance(outcome, dict) and type(outcome.get("commands_applied")) is int
            and outcome["commands_applied"] == 1, "actual named Close click receipt required")
    rect = locator.get("visible_rect")
    require(isinstance(rect, dict) and all(type(rect.get(key)) in (int, float) and math.isfinite(rect[key])
            for key in ("x", "y", "w", "h")) and rect["w"] > 0 and rect["h"] > 0,
            "Close click lacks actual observed hit/clip geometry")
    command = commands[0]
    require(all(type(command.get(key)) in (int, float) and math.isfinite(command[key]) for key in ("x", "y"))
            and rect["x"] <= command["x"] < rect["x"] + rect["w"]
            and rect["y"] <= command["y"] < rect["y"] + rect["h"], "Close click missed its observed target")


def verify_lifecycle(lease, root, record, source, binding, end):
    lifecycle = json_object(declared_snapshot(lease, root, record, source["lifecycle_artifact"], {"tool", "api"}))
    expected_keys = {"schema", "pass_id", "run_id", "checkpoint", "process_identity", "begin_receipt_sha256",
        "before_state_artifact", "close_action_artifact", "confirmation_state_artifact", "confirmation_png",
        "confirm_action_artifact", "controller_artifact", "close_command", "confirm_command", "confirm_exit_code"}
    require(set(lifecycle) == expected_keys and lifecycle["schema"] == "fe-qa-terminal-close-lifecycle-v1",
            "terminal close requires its actual GUI lifecycle evidence")
    for key in ("pass_id", "run_id", "checkpoint", "process_identity"):
        require(lifecycle[key] == binding[key], "terminal GUI evidence belongs to another capture")
    require(lifecycle["begin_receipt_sha256"] == end["begin_receipt_sha256"], "GUI evidence binds a foreign arm receipt")
    controller = jsonl_artifact(lease, root, record, lifecycle["controller_artifact"])
    require(len(controller) == 2 and controller[0].get("phase") == "armed" and controller[1].get("phase") == "complete"
            and controller[0].get("checkpoint") == TERMINAL_CHECKPOINT
            and controller[0].get("session_id") == end["session_id"]
            and controller[0].get("sha256") == end["begin_receipt_sha256"]
            and Path(controller[0].get("begin_artifact", "")) == Path(source["begin_artifact"])
            and Path(controller[1].get("end_artifact", "")) == Path(source["end_artifact"])
            and controller[1].get("sha256") == digest(declared_snapshot(lease, root, record, source["end_artifact"], {"tool", "os"}))
            and controller[1].get("exit") == end["exit"], "retain the actual armed/completed controller output, bound to these receipts")
    before = jsonl_artifact(lease, root, record, lifecycle["before_state_artifact"])
    confirmation = jsonl_artifact(lease, root, record, lifecycle["confirmation_state_artifact"])
    require(len(before) == len(confirmation) == 1, "retain actual full before/confirmation state reads")
    before, confirmation = before[0], confirmation[0]
    require(type(before.get("frame")) is int and type(confirmation.get("frame")) is int
            and 0 <= before["frame"] < confirmation["frame"]
            and isinstance(before.get("navigation"), dict) and before["navigation"]
            and before["navigation"] == confirmation.get("navigation")
            and isinstance(before.get("open_windows"), list) and isinstance(confirmation.get("open_windows"), list)
            and "Close FractalEngine?" not in before["open_windows"]
            and "Close FractalEngine?" in confirmation["open_windows"], "Close must open the actual confirmation in the same view")
    close = jsonl_artifact(lease, root, record, lifecycle["close_action_artifact"])
    close = [value for value in close if "commands" in value]
    require(len(close) == 1, "retain one actual caption Close action result")
    click_result(close[0])
    require(close[0].get("token_verse") == before["navigation"].get("verse_id"), "Close action scope differs from the observed view")
    png = declared_snapshot(lease, root, record, lifecycle["confirmation_png"], {"png"}, limit=16 * 1024 * 1024)
    require(png.startswith(b"\x89PNG\r\n\x1a\n"), "terminal confirmation needs its declared judged PNG")
    run = canonical(Path(root) / ".qa" / "run" / record["run_id"])
    for key in ("close_command", "confirm_command"):
        command = lifecycle[key]
        require(isinstance(command, list) and len(command) == 7 and command[0] == "--run"
                and isinstance(command[1], str) and canonical(command[1]) == run
                and command[2:] == ["click", "--name", "Close FractalEngine", "--role", "button"],
                "retain the actual named Close command expanded under this exact owned run")
    confirm = jsonl_artifact(lease, root, record, lifecycle["confirm_action_artifact"])
    confirm = [value for value in confirm if "commands" in value or "not_running" in value or "refused" in value]
    require(len(confirm) == 1 and type(lifecycle["confirm_exit_code"]) is int,
            "retain the actual modal-confirm client result and exit status")
    if lifecycle["confirm_exit_code"] == 0:
        click_result(confirm[0])
        require(confirm[0].get("token_verse") == before["navigation"].get("verse_id"), "confirm action scope differs from its view")
    else:
        message = confirm[0].get("message")
        require(lifecycle["confirm_exit_code"] == 3 and confirm[0].get("not_running") is True
                and isinstance(message, str) and (message.startswith(
                    "API at " + binding["api_base"] + "/api/v1/debug/input did not answer:")
                    or message.startswith("app died during POST /api/v1/debug/input (")),
                "shutdown before a confirm-input response is distinct from a refusal or an earlier failed probe")


def validate_record(root, checkpoint, record):
    require(checkpoint["id"] == TERMINAL_CHECKPOINT and checkpoint["reachability"] == "Y",
            "post-exit stdout is exclusive to the GUI F080.confirm-close checkpoint")
    source = record.get("log_source")
    require(isinstance(source, dict) and set(source) == {"kind", "begin_artifact", "end_artifact", "lifecycle_artifact"}
            and source["kind"] == "terminal_close_stdout", "invalid terminal close log source")
    require(not any(key in record for key in ("log_watermark", "log_after", "generation_changed", "retention",
            "log_exception", "new_logs_unavailable")), "terminal stdout cannot conceal mixed or incomplete log sources")
    with WindowsLease() as lease:
        file, binding = context(lease, root, record["pass_id"], record["run_id"], checkpoint["id"],
                                live=False, capture_kind="terminal_close")
        file = lease.seal_stdout(Path(root) / binding["source_path"], file)
        begin, end = record_receipts(lease, root, record, source)
        require(binding["stage_commit"].lower() == record["stage_commit"].lower()
                and binding["stage_fingerprint"].lower() == record["stage_fingerprint"].lower(),
                "terminal evidence belongs to another stage")
        reconcile_warnings(verify_terminal(file, binding, begin, end), record.get("new_logs"))
        verify_lifecycle(lease, root, record, source, binding, end)


def watch(args):
    require(type(args.timeout) is int and 1 <= args.timeout <= MAX_WAIT_SECONDS, "terminal timeout must be1–180 seconds")
    start = time.monotonic()
    root = canonical(args.root)
    with WindowsLease() as lease:
        file, binding = context(lease, root, args.pass_id, args.run_id, TERMINAL_CHECKPOINT,
                                live=True, capture_kind="terminal_close")
        begin_path = artifact_path(root, args.pass_id, args.begin)
        end_path = artifact_path(root, args.pass_id, args.out)
        require(begin_path != end_path, "terminal begin and end filenames must differ")
        lease.check_live()
        reserved_end = lease.file(end_path, create=True)
        offset = file.size()
        begin = {**binding, "schema": BEGIN_SCHEMA, "session_id": secrets.token_hex(16), "offset": offset,
            "prefix_sha256": prefix_hash(file, offset), "armed_at_ticks": str(time.time_ns() // 100 + 621355968000000000),
            "timeout_seconds": args.timeout}
        require(offset == 0 or file.read(offset - 1, 1) == b"\n", "terminal begin would split a line")
        begin_hash = write_receipt(lease, begin_path, begin)
        print(json.dumps({"phase": "armed", "checkpoint": TERMINAL_CHECKPOINT, "session_id": begin["session_id"],
            "begin_artifact": str(begin_path.relative_to(root)), "sha256": begin_hash}), flush=True)
        remaining = args.timeout - (time.monotonic() - start)
        require(remaining > 0, "terminal setup exhausted its deadline")
        exit_info = lease.wait_owned_exit(max(1, math.floor(remaining * 1000)))
        require(exit_info["exit_code"] == 0, "owned root exited with a nonzero native status")
        deadline = min(start + args.timeout + DRAIN_SECONDS, time.monotonic() + DRAIN_SECONDS)
        while True:
            try:
                sealed = lease.seal_stdout(root / binding["source_path"], file)
                break
            except BlockingIOError:
                require(time.monotonic() < deadline, "stdout writers did not release within five seconds")
                time.sleep(min(0.1, max(0, deadline - time.monotonic())))
        stop = sealed.size()
        require(type(offset) is int and 0 <= offset <= stop <= MAX_PREFIX
                and stop - offset <= MAX_WINDOW, "terminal stdout exceeded its byte budget")
        raw = sealed.read(offset, stop - offset)
        end = {**binding, "schema": END_SCHEMA, "session_id": begin["session_id"],
            "offset": stop, "prefix_sha256": prefix_hash(sealed, stop), "begin_receipt_sha256": begin_hash,
            "raw_window_sha256": digest(raw), "projection": window_projection(raw), "exit": exit_info,
            "elapsed_ms": math.ceil((time.monotonic() - start) * 1000)}
        verify_terminal(sealed, binding, begin, end)
        require(lease.owned_exit() == exit_info, "retained root exit facts changed")
        data = json.dumps(end, sort_keys=True, allow_nan=False).encode("utf-8")
        require(len(data) <= MAX_RECEIPT, "terminal receipt exceeded its bound")
        reserved_end.write(data)
        require(lease.owned_exit() == exit_info, "retained process exit facts changed during publication")
        print(json.dumps({"phase": "complete", "end_artifact": str(end_path.relative_to(root)), "sha256": digest(data),
                          "exit": exit_info, "raw_bytes_emitted": False}), flush=True)


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("operation", choices=("watch-close",))
    parser.add_argument("--root", default=str(Path(__file__).absolute().parents[2]))
    parser.add_argument("--run-id", required=True)
    parser.add_argument("--pass-id", required=True)
    parser.add_argument("--begin", required=True, help="new begin receipt JSON filename in the existing pass")
    parser.add_argument("--out", required=True, help="new terminal receipt JSON filename in the same pass")
    parser.add_argument("--timeout", type=int, default=90)
    args = parser.parse_args(argv)
    try:
        watch(args)
        return 0
    except (OSError, ValueError, TypeError, KeyError, AttributeError):
        print(json.dumps({"error": "terminal_stdout_refused", "reason": "exact root exit, final stdout or capture evidence unavailable"}), flush=True)
        return 2


if __name__ == "__main__":
    sys.exit(main())
