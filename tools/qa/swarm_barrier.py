"""Cross-agent start barrier for peer swarms; see AGENTS.md §swarm-barrier."""
import argparse
import importlib.util
import json
import os
import re
import subprocess
import sys
import time

SAFE_NAME = re.compile(r"^[A-Za-z0-9][A-Za-z0-9_.-]{0,63}$")
FIRE_LEAD_SECONDS = 2.0
POLL_SECONDS = 0.05
EXIT_TIMEOUT = 3


def write_json_atomic(path, payload):
    pending = path + ".pending"
    with open(pending, "w", encoding="utf-8") as handle:
        json.dump(payload, handle)
    os.replace(pending, path)


def read_ready_times(barrier_dir):
    ready = {}
    for entry in os.listdir(barrier_dir):
        if not entry.endswith(".ready.json"):
            continue
        try:
            with open(os.path.join(barrier_dir, entry), encoding="utf-8") as handle:
                ready[entry[: -len(".ready.json")]] = float(json.load(handle)["ready_at"])
        except (OSError, ValueError, KeyError, TypeError):
            continue  # a peer mid-write; the next poll sees it complete
    return ready


def prepare_in_process(command):
    """Pre-import a Python script exposing main(argv) so firing skips interpreter startup."""
    script = command[0]
    if not script.endswith(".py") or not os.path.isfile(script):
        return None
    script = os.path.abspath(script)
    sys.path.insert(0, os.path.dirname(script))
    spec = importlib.util.spec_from_file_location("swarm_target_" + os.path.basename(script)[:-3], script)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    entry = getattr(module, "main", None)
    if not callable(entry):
        return None

    def invoke():
        sys.argv = [script] + list(command[1:])  # the target sees its own argv, as a child process would
        return entry(command[1:])
    return invoke


def run_command(command, in_process):
    if in_process is None:
        return subprocess.call(command)
    try:
        code = in_process()
    except SystemExit as stop:
        code = stop.code
    if code is None:
        return 0
    return code if isinstance(code, int) else 1


def main(argv=None):
    parser = argparse.ArgumentParser(description="Release N swarm agents at one shared instant.")
    parser.add_argument("--run", required=True, help="swarm run directory")
    parser.add_argument("--step", required=True, help="barrier id; use a fresh id per attempt (5, 5b...)")
    parser.add_argument("--peer", required=True)
    parser.add_argument("--count", required=True, type=int)
    parser.add_argument("--timeout", type=float, default=180.0)
    parser.add_argument("--subprocess", action="store_true", help="never pre-import a .py target")
    parser.add_argument("--then", nargs=argparse.REMAINDER, default=[], help="command run at the fire instant")
    args = parser.parse_args(argv)
    if not SAFE_NAME.match(args.step) or not SAFE_NAME.match(args.peer) or args.count < 1:
        parser.error("step/peer must be simple names and count >= 1")

    barrier_dir = os.path.join(os.path.abspath(args.run), "swarm", "barrier", args.step)
    os.makedirs(barrier_dir, exist_ok=True)
    in_process = None if args.subprocess or not args.then else prepare_in_process(args.then)

    write_json_atomic(os.path.join(barrier_dir, args.peer + ".ready.json"), {"ready_at": time.time()})
    deadline = time.monotonic() + args.timeout
    ready = read_ready_times(barrier_dir)
    while len(ready) < args.count and time.monotonic() < deadline:
        time.sleep(POLL_SECONDS)
        ready = read_ready_times(barrier_dir)
    timed_out = len(ready) < args.count

    fire_at = max(ready.values()) + FIRE_LEAD_SECONDS
    delay = fire_at - time.time()
    if delay > 0:
        time.sleep(delay)
    started_at = time.time()
    report = {"step": args.step, "peer": args.peer, "ready_peers": sorted(ready), "fire_at": fire_at,
              "barrier": "barrier_timeout" if timed_out else "released"}
    print(json.dumps(report), file=sys.stderr, flush=True)

    exit_code = None
    if args.then:
        sys.stdout.flush()
        exit_code = run_command(args.then, in_process)
        sys.stdout.flush()
        write_json_atomic(os.path.join(barrier_dir, args.peer + ".done.json"), {
            "fire_at": fire_at, "started_at": started_at, "exit_code": exit_code,
            "elapsed_ms": round((time.time() - started_at) * 1000, 1),
            "late_ms": round((started_at - fire_at) * 1000, 1), "barrier": report["barrier"]})
    if timed_out:
        return EXIT_TIMEOUT
    return exit_code or 0


if __name__ == "__main__":
    sys.exit(main())
