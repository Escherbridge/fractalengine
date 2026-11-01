#!/usr/bin/env python3
"""Drive an isolated QA run; see tools/qa/AGENTS.md §fe-py."""

import argparse
import base64
import datetime
import json
import os
import sys
import time
import urllib.error
import urllib.request

EXIT_OK = 0
EXIT_REFUSED = 2
EXIT_NOT_RUNNING = 3

REPO_ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), "..", ".."))
CURRENT_POINTER = os.path.join(REPO_ROOT, ".qa", "current")

# Transient refusals worth one short retry loop before reporting (rate budgets,
# the single-flight slots, a posed capture holding the camera).
RETRYABLE_STATUS = {429, 409}
RETRY_ATTEMPTS = 6
RETRY_SLEEP_SECONDS = 1.1

MODIFIER_KEYS = {
    "ctrl": "ControlLeft", "control": "ControlLeft",
    "shift": "ShiftLeft",
    "alt": "AltLeft", "option": "AltLeft",
    "super": "SuperLeft", "win": "SuperLeft", "cmd": "SuperLeft", "meta": "SuperLeft",
}
NAMED_KEYS = {
    "esc": "Escape", "escape": "Escape",
    "enter": "Enter", "return": "Enter",
    "tab": "Tab", "space": "Space", "spacebar": "Space",
    "backspace": "Backspace", "delete": "Delete", "del": "Delete", "insert": "Insert",
    "home": "Home", "end": "End", "pageup": "PageUp", "pagedown": "PageDown",
    "left": "ArrowLeft", "right": "ArrowRight", "up": "ArrowUp", "down": "ArrowDown",
    "minus": "Minus", "-": "Minus", "equal": "Equal", "=": "Equal",
    "comma": "Comma", ",": "Comma", "period": "Period", ".": "Period",
    "slash": "Slash", "/": "Slash", "backslash": "Backslash",
    "semicolon": "Semicolon", "quote": "Quote", "backquote": "Backquote", "`": "Backquote",
    "bracketleft": "BracketLeft", "[": "BracketLeft", "bracketright": "BracketRight", "]": "BracketRight",
}


class Refused(Exception):
    """A typed refusal from the API (or a client-side precondition failure)."""

    def __init__(self, status, code, message, field=None):
        super().__init__(message)
        self.status = status
        self.code = code
        self.message = message
        self.field = field

    def as_dict(self):
        out = {"refused": True, "status": self.status, "code": self.code, "message": self.message}
        if self.field:
            out["field"] = self.field
        return out


class NotRunning(Exception):
    """No QA run recorded, or the recorded app's API does not answer."""


# --------------------------------------------------------------------------
# Run discovery
# --------------------------------------------------------------------------

def read_json_file(path):
    # launch.ps1 writes current.json with PowerShell 5.1's UTF-8 BOM.
    with open(path, "r", encoding="utf-8-sig") as handle:
        return json.load(handle)


def load_run():
    if not os.path.exists(CURRENT_POINTER):
        raise NotRunning("no .qa/current pointer - run tools\\qa\\launch.ps1 first")
    with open(CURRENT_POINTER, "r", encoding="utf-8-sig") as handle:
        run_dir = handle.read().strip()
    current_path = os.path.join(run_dir, "current.json")
    if not os.path.exists(current_path):
        raise NotRunning("no current.json under " + run_dir)
    current = read_json_file(current_path)
    current["run_dir"] = run_dir
    return current


def load_tokens(run):
    path = run.get("tokens_path") or os.path.join(run["run_dir"], "tokens.json")
    if not os.path.exists(path):
        raise NotRunning("no tokens file at " + path)
    return read_json_file(path)


def owner_verses(tokens):
    return [verse for verse, role in tokens.get("roles", {}).items() if role == "owner"]


# --------------------------------------------------------------------------
# HTTP
# --------------------------------------------------------------------------

def http(run, method, path, token=None, body=None, timeout=40.0):
    """One request. Returns (status, headers, raw_bytes); raises NotRunning on connect failure."""
    url = run.get("api_base", "http://127.0.0.1:8765") + path
    data = None
    headers = {}
    if token:
        headers["Authorization"] = "Bearer " + token
    if body is not None:
        data = json.dumps(body).encode("utf-8")
        headers["Content-Type"] = "application/json"
    request = urllib.request.Request(url, data=data, method=method, headers=headers)
    try:
        with urllib.request.urlopen(request, timeout=timeout) as response:
            return response.status, dict(response.headers), response.read()
    except urllib.error.HTTPError as error:
        return error.code, dict(error.headers or {}), error.read()
    except (urllib.error.URLError, ConnectionError, TimeoutError) as error:
        raise NotRunning("API at %s did not answer: %s" % (url, error))


def refusal_from(status, raw):
    try:
        payload = json.loads(raw.decode("utf-8"))
    except (ValueError, UnicodeDecodeError):
        payload = {}
    if not isinstance(payload, dict):
        payload = {}
    message = payload.get("error") or payload.get("message") or raw.decode("utf-8", "replace")[:300]
    if isinstance(message, dict):
        message = json.dumps(message)
    return Refused(status, payload.get("code") or "http_%d" % status, message, payload.get("field"))


def api_alive(run):
    """True once /api/v1/health answers; polled briefly so a dying host is seen dead."""
    time.sleep(0.5)
    for _ in range(3):
        try:
            http(run, "GET", "/api/v1/health", timeout=3.0)
            return True
        except NotRunning:
            time.sleep(0.5)
    return False


def call(run, method, path, token, body=None, timeout=40.0):
    """Request with a bounded retry on rate-limit / single-flight refusals."""
    for attempt in range(RETRY_ATTEMPTS):
        status, headers, raw = http(run, method, path, token, body, timeout)
        if 200 <= status < 300:
            return status, headers, raw
        refusal = refusal_from(status, raw)
        if status in RETRYABLE_STATUS and attempt + 1 < RETRY_ATTEMPTS:
            time.sleep(RETRY_SLEEP_SECONDS)
            continue
        if status >= 500 and not api_alive(run):
            # A 5xx whose host then stops answering is a crash, not a refusal.
            raise NotRunning("app died during %s %s (%s: %s) - read <run>/stderr.log"
                             % (method, path, refusal.code, refusal.message))
        raise refusal
    raise Refused(0, "retry_exhausted", "retries exhausted")


# --------------------------------------------------------------------------
# Token selection: the token whose scope covers the ACTIVE verse
# --------------------------------------------------------------------------

def active_cache_path(run):
    return os.path.join(run["run_dir"], "fe-active-verse.txt")


def read_camera_with(run, token):
    status, _headers, raw = http(run, "GET", "/api/v1/debug/camera", token, timeout=10.0)
    if status == 429:
        time.sleep(0.2)
        status, _headers, raw = http(run, "GET", "/api/v1/debug/camera", token, timeout=10.0)
    return status, raw


def resolve_token(run, verse=None):
    """(verse_id, token, camera_state). Probes owner tokens against GET /camera."""
    tokens = load_tokens(run)
    if verse:
        token = tokens.get("tokens", {}).get(verse)
        if not token:
            raise Refused(0, "no_token_for_verse", "tokens.json has no token for verse " + verse)
        status, raw = read_camera_with(run, token)
        if status != 200:
            raise refusal_from(status, raw)
        return verse, token, json.loads(raw.decode("utf-8")).get("data")

    candidates = owner_verses(tokens)
    cache = active_cache_path(run)
    if os.path.exists(cache):
        with open(cache, "r", encoding="utf-8") as handle:
            cached = handle.read().strip()
        if cached in candidates:
            candidates.remove(cached)
            candidates.insert(0, cached)
    last = None
    for candidate in candidates:
        token = tokens["tokens"][candidate]
        status, raw = read_camera_with(run, token)
        if status == 200:
            with open(cache, "w", encoding="utf-8") as handle:
                handle.write(candidate)
            return candidate, token, json.loads(raw.decode("utf-8")).get("data")
        last = refusal_from(status, raw)
        if status != 404:
            raise last
    if last is None:
        raise Refused(0, "no_owner_token", "tokens.json holds no owner token yet")
    raise Refused(
        last.status, "no_active_verse_covered",
        "no owner token covers the active view (no verse open, or the open verse is not "
        "owned by this node) - last refusal: %s" % last.message)


# --------------------------------------------------------------------------
# Commands
# --------------------------------------------------------------------------

def emit(obj):
    print(json.dumps(obj, sort_keys=True))


def shots_dir(run, out):
    if out:
        return None
    directory = os.path.abspath(os.environ.get("FE_QA_SHOTS_DIR") or os.path.join(run["run_dir"], "shots"))
    os.makedirs(directory, exist_ok=True)
    return directory


def shot_path(run, out, label, suffix=""):
    if out:
        parent = os.path.dirname(os.path.abspath(out))
        os.makedirs(parent, exist_ok=True)
        return os.path.abspath(out)
    directory = shots_dir(run, out)
    stamp = datetime.datetime.now().strftime("%H%M%S-%f")[:-3]
    name = stamp + ("-" + label if label else "") + suffix + ".png"
    return os.path.join(directory, name)


def command_camera(run, args):
    verse, _token, camera = resolve_token(run, args.verse)
    emit({"token_verse": verse, "camera": camera})


def build_pose(args):
    chosen = [name for name in ("pose", "orbit", "tile", "geo", "focus_node", "focus_petal")
              if getattr(args, name) is not None]
    if len(chosen) > 1:
        raise Refused(0, "pose_conflict", "give at most one of --pose/--orbit/--tile/--geo/--focus-node/--focus-petal")
    if not chosen:
        return None
    if args.pose is not None:
        pose = json.loads(args.pose)
        if not isinstance(pose, dict):
            raise Refused(0, "pose_not_object", "--pose must be a JSON object like {\"orbit\":{...}}")
        return pose
    if args.orbit is not None:
        fx, fy, fz, yaw, pitch, distance = args.orbit
        return {"orbit": {"focus": [fx, fy, fz], "yaw_deg": yaw, "pitch_deg": pitch, "distance": distance}}
    if args.tile is not None:
        z, x, y = args.tile
        return {"tile": {"z": z, "x": x, "y": y}}
    if args.geo is not None:
        if len(args.geo) not in (2, 3):
            raise Refused(0, "invalid_geo", "--geo requires LAT LON and optional ALT")
        geo = {"lat": args.geo[0], "lon": args.geo[1]}
        if len(args.geo) > 2:
            geo["altitude_m"] = args.geo[2]
        return {"geo": geo}
    focus = {"node_id": args.focus_node} if args.focus_node else {"petal_id": args.focus_petal}
    if args.navigate:
        focus["navigate"] = True
    return {"focus": focus}


def command_shot(run, args):
    verse, token, _camera = resolve_token(run, args.verse)
    body = {}
    pose = build_pose(args)
    if pose is not None:
        body["pose"] = pose
    if args.no_ui:
        body["include_ui"] = False
        if args.width:
            body["width"] = args.width
        if args.height:
            body["height"] = args.height
    if args.settle is not None:
        body["settle_frames"] = args.settle
    if args.no_restore:
        body["restore"] = False
    _status, headers, raw = call(run, "POST", "/api/v1/debug/screenshot", token, body, timeout=60.0)
    path = shot_path(run, args.out, args.label)
    with open(path, "wb") as handle:
        handle.write(raw)
    pose_header = None
    for key, value in headers.items():
        if key.lower() == "x-fe-capture-pose":
            try:
                pose_header = json.loads(value)
            except ValueError:
                pose_header = value
    emit({"path": path, "bytes": len(raw), "token_verse": verse, "request": body, "pose": pose_header})


def run_input(run, args, commands):
    verse, token, _camera = resolve_token(run, args.verse)
    body = {"commands": commands, "wait_frames": args.wait}
    if args.capture:
        body["capture_after"] = True
    try:
        _status, _headers, raw = call(run, "POST", "/api/v1/debug/input", token, body, timeout=30.0)
    except Refused as refusal:
        # The host re-checks scope every frame a job runs: a click that itself
        # opens ANOTHER verse is denied once that verse is on screen, after the
        # click landed. Say so instead of reporting a plain refusal.
        if refusal.code != "not_found_or_denied" or args.verse:
            raise
        try:
            new_verse, _token, camera = resolve_token(run)
        except Refused:
            raise refusal
        if new_verse == verse:
            raise refusal
        emit({"token_verse": verse, "commands": commands, "view_changed_verse": new_verse,
              "active": (camera or {}).get("active"),
              "warning": "the batch moved the view to another verse; the host denied the rest "
                         "of the batch under the old verse's token (expected for cross-verse "
                         "navigation - re-shoot to see the result)"})
        return
    payload = json.loads(raw.decode("utf-8"))
    if isinstance(payload, dict) and "data" in payload and isinstance(payload["data"], dict):
        payload = payload["data"]
    result = {"token_verse": verse, "commands": commands}
    png = payload.pop("capture_after_png_base64", None) if isinstance(payload, dict) else None
    result["outcome"] = payload
    if png:
        path = shot_path(run, args.out, args.label, "-after")
        with open(path, "wb") as handle:
            handle.write(base64.b64decode(png))
        result["capture_path"] = path
    emit(result)


def parse_combo(combo):
    """'ctrl+shift+z' -> ['ControlLeft', 'ShiftLeft', 'KeyZ'] (sticky modifiers server-side)."""
    parts = [part for part in combo.split("+") if part] if combo != "+" else ["+"]
    keys = []
    for part in parts:
        lowered = part.lower()
        if lowered in MODIFIER_KEYS:
            keys.append(MODIFIER_KEYS[lowered])
        elif lowered in NAMED_KEYS:
            keys.append(NAMED_KEYS[lowered])
        elif len(part) == 1 and part.isalpha():
            keys.append("Key" + part.upper())
        elif len(part) == 1 and part.isdigit():
            keys.append("Digit" + part)
        elif lowered.startswith("f") and lowered[1:].isdigit():
            keys.append("F" + lowered[1:])
        else:
            keys.append(part)  # already a KeyCode variant name ("NumpadAdd", "KeyA")
    return keys


def command_move(run, args):
    run_input(run, args, [{"kind": "pointer_move", "x": args.x, "y": args.y}])


def command_click(run, args):
    click = {"kind": "pointer_click", "x": args.x, "y": args.y, "button": args.button}
    run_input(run, args, [click, dict(click)] if args.double else [click])


def command_drag(run, args):
    run_input(run, args, [{
        "kind": "pointer_drag", "button": args.button,
        "from": {"x": args.x1, "y": args.y1}, "to": {"x": args.x2, "y": args.y2},
    }])


def command_key(run, args):
    commands = []
    for combo in args.combos:
        commands.extend({"kind": "key_press", "key": key} for key in parse_combo(combo))
    run_input(run, args, commands)


def command_type(run, args):
    run_input(run, args, [{"kind": "text_entry", "text": args.text}])


def command_scroll(run, args):
    commands = []
    if args.x is not None and args.y is not None:
        commands.append({"kind": "pointer_move", "x": args.x, "y": args.y})
    commands.append({"kind": "scroll", "dx": args.dx, "dy": args.dy})
    run_input(run, args, commands)


def pose_signature(camera):
    pose = (camera or {}).get("pose") or {}
    rounded = lambda values: [round(v, 4) for v in (values or [])]
    return (rounded(pose.get("position")), rounded(pose.get("target")), (camera or {}).get("locked"),
            json.dumps((camera or {}).get("active"), sort_keys=True))


def command_wait_settle(run, args):
    verse, token, camera = resolve_token(run, args.verse)
    deadline = time.time() + args.timeout
    stable = 0
    previous = pose_signature(camera)
    polls = 0
    while time.time() < deadline:
        time.sleep(args.interval)
        status, raw = read_camera_with(run, token)
        polls += 1
        if status != 200:
            raise refusal_from(status, raw)
        camera = json.loads(raw.decode("utf-8")).get("data")
        signature = pose_signature(camera)
        stable = stable + 1 if signature == previous and not camera.get("locked") else 0
        previous = signature
        if stable >= args.stable_polls:
            emit({"settled": True, "polls": polls, "token_verse": verse, "camera": camera})
            return
    raise Refused(0, "settle_timeout", "camera still moving after %.1fs (%d polls)" % (args.timeout, polls))


def command_info(run, args):
    tokens = load_tokens(run)
    emit({
        "run_dir": run["run_dir"], "pid": run.get("pid"), "api_base": run.get("api_base"),
        "fixture": run.get("fixture_detail"), "roles": tokens.get("roles"),
        "expires_at": tokens.get("expires_at"),
    })


def command_api(run, args):
    """Raw REST call with a verse token - for fixtures/probes only, never a UX step."""
    tokens = load_tokens(run)
    if args.verse:
        token = tokens.get("tokens", {}).get(args.verse)
        if not token:
            raise Refused(0, "no_token_for_verse", "tokens.json has no token for verse " + args.verse)
    else:
        _verse, token, _camera = resolve_token(run)
    body = json.loads(args.body) if args.body else None
    path = args.path.strip()
    if "/api/" in path and not path.startswith("/api/"):
        path = path[path.index("/api/"):]  # Git Bash rewrote /api/... into C:/Program Files/Git/api/...
    elif not path.startswith("/"):
        path = "/" + path
    _status, _headers, raw = call(run, args.method.upper(), path, token, body)
    sys.stdout.write(raw.decode("utf-8", "replace") + "\n")


# --------------------------------------------------------------------------
# CLI
# --------------------------------------------------------------------------

def build_parser():
    parser = argparse.ArgumentParser(prog="fe.py", description=__doc__,
                                     formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--verse", help="use this verse's token instead of probing for the active one")
    sub = parser.add_subparsers(dest="command", required=True)

    sub.add_parser("info", help="run dir, pid, fixture ids, token roles").set_defaults(func=command_info)
    sub.add_parser("camera", help="GET /api/v1/debug/camera").set_defaults(func=command_camera)

    shot = sub.add_parser("shot", help="capture a PNG (window+UI by default)")
    shot.add_argument("--pose", help='raw pose JSON, e.g. \'{"camera":{"position":[0,5,10],"target":[0,0,0]}}\'')
    shot.add_argument("--orbit", nargs=6, type=float, metavar=("FX", "FY", "FZ", "YAW", "PITCH", "DIST"))
    shot.add_argument("--tile", nargs=3, type=int, metavar=("Z", "X", "Y"))
    shot.add_argument("--geo", nargs="+", type=float, metavar="LAT_LON_[ALT]")
    shot.add_argument("--focus-node")
    shot.add_argument("--focus-petal")
    shot.add_argument("--navigate", action="store_true", help="with --focus-*: switch petal first")
    shot.add_argument("--no-ui", action="store_true", help="scene-only off-screen render (include_ui=false)")
    shot.add_argument("--width", type=int)
    shot.add_argument("--height", type=int)
    shot.add_argument("--settle", type=int, help="settle_frames")
    shot.add_argument("--no-restore", action="store_true", help="leave the camera at the pose")
    shot.add_argument("--out", help="PNG path (default: $FE_QA_SHOTS_DIR or <run>/shots/)")
    shot.add_argument("--label", default="", help="filename suffix when --out is not given")
    shot.set_defaults(func=command_shot)

    def input_parser(name, help_text, func):
        command = sub.add_parser(name, help=help_text)
        command.add_argument("--wait", type=int, default=5, help="wait_frames after the batch (<=120)")
        command.add_argument("--capture", action="store_true", help="capture_after: save a PNG of the result")
        command.add_argument("--out", help="PNG path for --capture")
        command.add_argument("--label", default="", help="filename suffix for --capture")
        command.set_defaults(func=func)
        return command

    move = input_parser("move", "pointer_move to physical pixel x y", command_move)
    move.add_argument("x", type=float)
    move.add_argument("y", type=float)

    click = input_parser("click", "pointer_click at x y", command_click)
    click.add_argument("x", type=float)
    click.add_argument("y", type=float)
    click.add_argument("--button", choices=("left", "right", "middle"), default="left")
    click.add_argument("--double", action="store_true")

    drag = input_parser("drag", "pointer_drag from x1 y1 to x2 y2", command_drag)
    for name in ("x1", "y1", "x2", "y2"):
        drag.add_argument(name, type=float)
    drag.add_argument("--button", choices=("left", "right", "middle"), default="left")

    key = input_parser("key", "key combos in order, e.g. ctrl+z Escape F12 g", command_key)
    key.add_argument("combos", nargs="+")

    text = input_parser("type", "text_entry", command_type)
    text.add_argument("text")

    scroll = input_parser("scroll", "wheel lines (positive = up/zoom in), optionally at x y", command_scroll)
    scroll.add_argument("dy", type=float)
    scroll.add_argument("x", type=float, nargs="?")
    scroll.add_argument("y", type=float, nargs="?")
    scroll.add_argument("--dx", type=float, default=0.0)

    settle = sub.add_parser("wait-settle", help="poll the camera until its pose is stable")
    settle.add_argument("--timeout", type=float, default=10.0)
    settle.add_argument("--interval", type=float, default=0.15)
    settle.add_argument("--stable-polls", type=int, default=3)
    settle.set_defaults(func=command_wait_settle)

    api = sub.add_parser("api", help="raw REST call with a verse token (fixtures/probes only)")
    api.add_argument("method")
    api.add_argument("path")
    api.add_argument("body", nargs="?")
    api.set_defaults(func=command_api)
    return parser


def main(argv=None):
    args = build_parser().parse_args(argv)
    try:
        run = load_run()
        args.func(run, args)
        return EXIT_OK
    except Refused as refusal:
        emit(refusal.as_dict())
        sys.stderr.write("REFUSED %s %s: %s\n" % (refusal.status, refusal.code, refusal.message))
        return EXIT_REFUSED
    except NotRunning as error:
        emit({"not_running": True, "message": str(error)})
        sys.stderr.write("NOT RUNNING: %s\n" % error)
        return EXIT_NOT_RUNNING
    except OSError as error:
        emit({"not_running": True, "message": str(error)})
        sys.stderr.write("QA FILE ERROR: %s\n" % error)
        return EXIT_NOT_RUNNING
    except (ValueError, TypeError) as error:
        refusal = Refused(0, "invalid_json_or_value", str(error))
        emit(refusal.as_dict())
        return EXIT_REFUSED


if __name__ == "__main__":
    sys.exit(main())
