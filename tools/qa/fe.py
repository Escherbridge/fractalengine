#!/usr/bin/env python3
"""Drive an isolated QA run; see tools/qa/AGENTS.md §fe-py."""

import argparse
import base64
import datetime
import contextlib
import json
import ipaddress
import hashlib
import math
import ntpath
import os
import stat
import sys
import time
import urllib.error
import urllib.request
import urllib.parse

EXIT_OK = 0
EXIT_REFUSED = 2
EXIT_NOT_RUNNING = 3

REPO_ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), "..", ".."))
CURRENT_POINTER = os.path.join(REPO_ROOT, ".qa", "current")

# Transient refusals worth one short retry loop before reporting (rate budgets,
# the single-flight slots, a posed capture holding the camera).
TRANSIENT_REFUSALS = {"http_409", "window_request_busy", "ui_frame_changing", "ui_tree_not_ready",
                      "portal_fixture_not_ready", "portal_fixture_busy",
                      "input_in_progress", "capture_in_progress", "camera_locked_by_capture"}
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


class RefuseRedirects(urllib.request.HTTPRedirectHandler):
    """Keep authenticated QA requests on their validated loopback origin."""

    def http_error_302(self, request, response, code, message, headers):
        raise Refused(code, "redirect_refused", "QA API redirects are refused")

    http_error_300 = http_error_302
    http_error_301 = http_error_302
    http_error_303 = http_error_302
    http_error_305 = http_error_302
    http_error_307 = http_error_302
    http_error_308 = http_error_302


HTTP_OPENER = urllib.request.build_opener(urllib.request.ProxyHandler({}), RefuseRedirects())


# --------------------------------------------------------------------------
# Run discovery
# --------------------------------------------------------------------------

def read_json_file(path):
    # launch.ps1 writes current.json with PowerShell 5.1's UTF-8 BOM.
    with open(path, "r", encoding="utf-8-sig") as handle:
        return require_object(json.load(handle), path)


def require_object(value, source="API response"):
    if not isinstance(value, dict):
        raise Refused(0, "invalid_response", source + " must be a JSON object")
    return value


def decode_object(raw):
    try:
        payload = require_object(json.loads(raw.decode("utf-8")))
    except (ValueError, UnicodeDecodeError) as error:
        raise Refused(0, "invalid_response", "API returned malformed JSON: " + str(error))
    if payload.get("ok") is False:
        raise Refused(0, payload.get("code", "invalid_response"), str(payload.get("error", "API refused the request")))
    return require_object(payload["data"]) if "data" in payload else payload


def load_run(explicit=None):
    if explicit:
        candidate = explicit if os.path.isabs(explicit) or os.path.exists(explicit) else os.path.join(REPO_ROOT, ".qa", "run", explicit)
        run_dir = os.path.dirname(candidate) if os.path.basename(candidate) == "current.json" else candidate
    else:
        if not os.path.exists(CURRENT_POINTER):
            raise NotRunning("no .qa/current pointer - run tools\\qa\\launch.ps1 first")
        with open(CURRENT_POINTER, "r", encoding="utf-8-sig") as handle:
            run_dir = handle.read().strip()
    run_dir = os.path.realpath(run_dir)
    current_path = os.path.join(run_dir, "current.json")
    if not os.path.exists(current_path):
        raise NotRunning("no current.json under " + run_dir)
    current = read_json_file(current_path)
    api_base = current.get("api_base")
    if not isinstance(api_base, str):
        raise Refused(0, "invalid_run", "QA API base must be a string")
    parsed = urllib.parse.urlsplit(api_base)
    try:
        loopback = parsed.hostname == "localhost" or ipaddress.ip_address(parsed.hostname).is_loopback
    except ValueError:
        loopback = False
    if parsed.scheme != "http" or not loopback or parsed.username or parsed.password or parsed.path not in ("", "/") or parsed.query or parsed.fragment:
        raise Refused(0, "invalid_run", "QA API base must be a loopback HTTP origin")
    current["run_dir"] = run_dir
    return current


def load_tokens(run):
    path = run.get("tokens_path") or os.path.join(run["run_dir"], "tokens.json")
    if not os.path.exists(path):
        raise NotRunning("no tokens file at " + path)
    tokens = read_json_file(path)
    require_object(tokens.get("tokens"), "tokens map")
    require_object(tokens.get("roles"), "roles map")
    return tokens


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
        with HTTP_OPENER.open(request, timeout=timeout) as response:
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


def api_alive(run, transport=None):
    """True once /api/v1/health answers; polled briefly so a dying host is seen dead."""
    time.sleep(0.5)
    for _ in range(3):
        try:
            (transport or http)(run, "GET", "/api/v1/health", timeout=3.0)
            return True
        except NotRunning:
            time.sleep(0.5)
    return False


def call(run, method, path, token, body=None, timeout=40.0, transport=None):
    """Request with a bounded retry on rate-limit / single-flight refusals."""
    for attempt in range(RETRY_ATTEMPTS):
        status, headers, raw = (transport or http)(run, method, path, token, body, timeout)
        if 200 <= status < 300:
            return status, headers, raw
        refusal = refusal_from(status, raw)
        if (status == 429 or (status == 409 and refusal.code in TRANSIENT_REFUSALS)) and attempt + 1 < RETRY_ATTEMPTS:
            time.sleep(RETRY_SLEEP_SECONDS)
            continue
        if status >= 500 and not api_alive(run, transport):
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


def read_camera_with(run, token, transport=None):
    for attempt in range(RETRY_ATTEMPTS):
        status, _headers, raw = (transport or http)(run, "GET", "/api/v1/debug/camera", token, timeout=10.0)
        if status != 429 or attempt + 1 == RETRY_ATTEMPTS:
            break
        time.sleep(0.25)
    return status, raw


def resolve_token(run, verse=None, transport=None, use_cache=True, token_snapshot=None):
    """(verse_id, token, camera_state). Probes owner tokens against GET /camera."""
    tokens = token_snapshot if token_snapshot is not None else load_tokens(run)
    if verse:
        token = tokens.get("tokens", {}).get(verse)
        if not token:
            raise Refused(0, "no_token_for_verse", "tokens.json has no token for verse " + verse)
        status, raw = read_camera_with(run, token, transport)
        if status != 200:
            raise refusal_from(status, raw)
        return verse, token, decode_object(raw)

    candidates = owner_verses(tokens)
    cache = active_cache_path(run)
    if use_cache and os.path.exists(cache):
        with open(cache, "r", encoding="utf-8") as handle:
            cached = handle.read().strip()
        if cached in candidates:
            candidates.remove(cached)
            candidates.insert(0, cached)
    last = None
    for candidate in candidates:
        token = tokens["tokens"][candidate]
        status, raw = read_camera_with(run, token, transport)
        if status == 200:
            if use_cache:
                with open(cache, "w", encoding="utf-8") as handle:
                    handle.write(candidate)
            return candidate, token, decode_object(raw)
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


def portal_capture_path(run, requested):
    root = os.path.abspath(run["run_dir"])
    filename = datetime.datetime.now().strftime("%H%M%S-%f") + "-portal-native.png"
    requested = requested or os.path.join("shots", filename)
    drive, tail = ntpath.splitdrive(requested)
    if (drive and (len(drive) != 2 or not drive[0].isascii() or not drive[0].isalpha()
                   or drive[1] != ":" or not tail.startswith(("\\", "/")))) or (not drive and tail.startswith(("\\", "/"))):
        raise Refused(0, "capture_path_refused", "native Portal output needs a run-relative or absolute drive path")
    reserved = {"CON", "PRN", "AUX", "NUL", "CONIN$", "CONOUT$"}
    reserved.update(prefix + suffix for prefix in ("COM", "LPT") for suffix in "123456789\u00b9\u00b2\u00b3")
    components = tail.replace("/", "\\").split("\\")
    if drive:
        components = components[1:]
    if any(not part or part in (".", "..") or part.endswith((".", " "))
           or any(ord(char) < 32 or 127 <= ord(char) < 160 or char in '<>:"|?*' for char in part)
           or part.split(".", 1)[0].rstrip(" ").upper() in reserved for part in components):
        raise Refused(0, "capture_path_refused", "native Portal output contains an unsafe Windows path component")
    candidate = os.path.abspath(os.path.join(root, requested))
    try:
        inside = os.path.commonpath([root, candidate]) == root
    except ValueError:
        inside = False
    if not inside or candidate == root or not candidate.lower().endswith(".png"):
        raise Refused(0, "capture_path_refused", "native Portal PNG must be a new .png beneath this run")
    def inspect():
        cursor = candidate
        while True:
            if os.path.lexists(cursor):
                info = os.lstat(cursor)
                if stat.S_ISLNK(info.st_mode) or getattr(info, "st_file_attributes", 0) & 0x400:
                    raise Refused(0, "capture_path_refused", "native Portal output cannot traverse redirected paths")
            parent = os.path.dirname(cursor)
            if parent == cursor:
                break
            cursor = parent
        if os.path.commonpath([os.path.realpath(root), os.path.realpath(candidate)]) != os.path.realpath(root):
            raise Refused(0, "capture_path_refused", "native Portal output escaped its run")
    inspect()
    if not os.path.isdir(os.path.dirname(candidate)):
        raise Refused(0, "capture_path_refused", "native Portal output requires an existing destination directory")
    if os.path.lexists(candidate) or os.path.lexists(candidate + ".json"):
        raise Refused(0, "capture_path_refused", "native Portal output must not replace existing evidence")
    return candidate


@contextlib.contextmanager
def portal_output_lease(path):
    handles = []
    kernel = None
    try:
        if os.name == "nt":
            import ctypes
            kernel = ctypes.WinDLL("kernel32", use_last_error=True)
            kernel.CreateFileW.argtypes = [ctypes.c_wchar_p, ctypes.c_uint32, ctypes.c_uint32,
                                          ctypes.c_void_p, ctypes.c_uint32, ctypes.c_uint32, ctypes.c_void_p]
            kernel.CreateFileW.restype = ctypes.c_void_p
            kernel.CloseHandle.argtypes = [ctypes.c_void_p]
            kernel.GetFileAttributesW.argtypes = [ctypes.c_wchar_p]
            kernel.GetFileAttributesW.restype = ctypes.c_uint32
            ancestors = []
            cursor = os.path.dirname(path)
            while True:
                ancestors.append(cursor)
                parent = os.path.dirname(cursor)
                if parent == cursor:
                    break
                cursor = parent
            for directory in reversed(ancestors):
                handle = kernel.CreateFileW(directory, 0x80, 1, None, 3, 0x02200000, None)
                if handle in (None, ctypes.c_void_p(-1).value):
                    raise Refused(0, "capture_path_refused", "cannot retain the native capture directory boundary")
                handles.append(handle)
                attributes = kernel.GetFileAttributesW(directory)
                if attributes == 0xFFFFFFFF or attributes & 0x400 or not attributes & 0x10:
                    raise Refused(0, "capture_path_refused", "native capture directory boundary is redirected or unavailable")
        yield
    finally:
        if kernel is not None:
            for handle in reversed(handles):
                kernel.CloseHandle(handle)


def command_portal_fixture(run, args):
    verse, token, _camera = resolve_token(run, args.verse)
    base = "/api/v1/debug/portal-fixture/"
    if args.operation == "observe":
        _status, _headers, raw = call(run, "GET", base + "observe", token, timeout=8.0)
        emit(decode_object(raw))
        return
    if args.operation != "capture":
        operation = {"second-link":"activate_second_link", "refused-third-link":"activate_refused_third_link"}[args.operation]
        _status, _headers, raw = call(run, "POST", base + "action", token, {"operation":operation}, timeout=8.0)
        emit(decode_object(raw))
        return
    path = portal_capture_path(run, args.out)
    _status, headers, raw = call(run, "POST", base + "capture", token, timeout=8.0)
    try:
        metadata = require_object(json.loads(next(value for key, value in headers.items() if key.lower() == "x-fe-portal-fixture")))
    except (StopIteration, ValueError) as error:
        raise Refused(0, "capture_receipt_missing", "native Portal PNG has no valid identity receipt") from error
    if not 33 <= len(raw) <= 8 * 1024 * 1024 or raw[:8] != b"\x89PNG\r\n\x1a\n" or raw[12:16] != b"IHDR":
        raise Refused(0, "invalid_native_png", "native Portal capture is not a bounded PNG")
    width, height = int.from_bytes(raw[16:20], "big"), int.from_bytes(raw[20:24], "big")
    digest = hashlib.sha256(raw).hexdigest()
    if not (0 < width <= 2048 and 0 < height <= 2048) or metadata.get("width") != width or metadata.get("height") != height or metadata.get("sha256") != digest or metadata.get("capture_source") != "owned_webview2_capture_preview":
        raise Refused(0, "capture_receipt_mismatch", "native PNG dimensions, source or hash do not match its receipt")
    # Exclusive evidence creation preserves earlier captures and receipts.
    receipt = {"path":path, "bytes":len(raw), "token_verse":verse, "native_capture":metadata}
    with portal_output_lease(path):
        path = portal_capture_path(run, path)
        with open(path, "xb") as handle:
            with open(path + ".json", "x", encoding="utf-8") as receipt_handle:
                handle.write(raw)
                json.dump(receipt, receipt_handle, indent=2)
                receipt_handle.write("\n")
    emit(receipt)


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
    payload = decode_object(raw)
    result = {"token_verse": verse, "commands": commands}
    if getattr(args, "locator_evidence", None):
        result["locator"] = args.locator_evidence
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


def command_transfer_invite(run, args):
    from invite_transfer import transfer
    emit(transfer(sys.modules[__name__], run, args))


def command_move(run, args):
    run_input(run, args, [{"kind": "pointer_move", "x": args.x, "y": args.y}])


def command_click(run, args):
    if args.name:
        args.locator_evidence = {}
        args.x, args.y = locate(run, args.name, args.role, args.nth, args.verse,
                                evidence=args.locator_evidence)
    elif args.x is None or args.y is None:
        raise Refused(0, "missing_coordinates", "click needs X Y or --name TEXT")
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
    if args.into:
        args.locator_evidence = {}
        x, y = locate(run, args.into, args.role, args.nth, args.verse, input_only=True,
                      evidence=args.locator_evidence)
        focus_args = argparse.Namespace(**vars(args))
        focus_args.capture = False
        run_input(run, focus_args, [{"kind":"pointer_click", "x":x, "y":y, "button":"left"}])
    run_input(run, args, [{"kind": "text_entry", "text": args.text}])


def command_scroll(run, args):
    commands = []
    if args.x is not None and args.y is not None:
        commands.append({"kind": "pointer_move", "x": args.x, "y": args.y})
    commands.append({"kind": "scroll", "dx": args.dx, "dy": args.dy})
    run_input(run, args, commands)


def pose_signature(camera):
    pose = require_object(require_object(camera).get("pose"), "camera pose")
    rounded = lambda values: [round(v, 4) for v in (values or [])]
    return (rounded(pose.get("position")), rounded(pose.get("target")), (camera or {}).get("locked"),
            json.dumps((camera or {}).get("active"), sort_keys=True))


def command_wait_settle(run, args):
    if not math.isfinite(args.timeout) or args.timeout <= 0 or not math.isfinite(args.interval) or args.interval <= 0 or args.stable_polls < 1:
        raise Refused(0, "invalid_settle_options", "timeout/interval must be finite positive values and stable-polls >= 1")
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
        camera = decode_object(raw)
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


def probe(run, path, verse=None, params=None, transport=None, use_cache=True):
    _verse, token, _camera = resolve_token(run, verse, transport, use_cache)
    query = "?" + urllib.parse.urlencode(params) if params else ""
    _status, _headers, raw = call(run, "GET", "/api/v1/debug/" + path + query, token, transport=transport)
    return decode_object(raw)


def tree_nodes(tree):
    nodes = tree.get("nodes")
    if not isinstance(nodes, list) or any(not isinstance(node, dict) for node in nodes):
        raise Refused(0, "invalid_response", "UI tree nodes must be an array of objects")
    if tree.get("coordinate_space") != "physical_pixels":
        raise Refused(0, "coordinate_space_mismatch", "UI tree must use physical screenshot pixels")
    for node in nodes:
        if not isinstance(node.get("name"), str) or not isinstance(node.get("role"), str):
            raise Refused(0, "invalid_response", "UI node names/roles must be strings")
        if node.get("bounds") is not None:
            bounds = require_object(node["bounds"], "widget bounds")
            if any(not isinstance(bounds.get(k), (int, float)) or not math.isfinite(bounds[k]) for k in ("x","y","w","h")):
                raise Refused(0, "invalid_response", "widget bounds must contain finite x/y/w/h numbers")
    return nodes


def locator_rectangle(bounds, facts, window):
    rectangles = [bounds, window]
    if facts is not None:
        if facts.get("allowed") is not True:
            return None
        rectangles.extend([facts.get("hit"), facts.get("clip")])
    for rect in rectangles:
        if not isinstance(rect, dict) or any(type(rect.get(k)) not in (int, float)
                or not math.isfinite(rect[k]) for k in ("x", "y", "w", "h")):
            raise Refused(0, "invalid_response", "locator geometry must contain finite rectangles")
        if rect["w"] <= 0 or rect["h"] <= 0:
            return None
    left = max(rect["x"] for rect in rectangles)
    top = max(rect["y"] for rect in rectangles)
    right = min(rect["x"] + rect["w"] for rect in rectangles)
    bottom = min(rect["y"] + rect["h"] for rect in rectangles)
    if right <= left or bottom <= top:
        return None
    return {"x": left, "y": top, "w": right-left, "h": bottom-top}


def locate(run, name, role=None, nth=None, verse=None, input_only=False, evidence=None):
    params = {"name":name, "interactive":1}
    if role:
        params["role"] = role
    tree = probe(run, "ui-tree", verse, params)
    window = require_object(tree.get("window"), "primary window dimensions")
    width, height = window.get("width"), window.get("height")
    if not isinstance(width, (int, float)) or not isinstance(height, (int, float)):
        raise Refused(0, "invalid_response", "primary window dimensions must be numbers")
    controls = {control["id"]: control for control in tree.get("layout", {}).get("controls", [])}
    window_rect = {"x": 0, "y": 0, "w": width, "h": height}
    candidates = tree_nodes(tree)
    exact = [node for node in candidates if node["name"].casefold() == name.casefold()]
    if exact:
        candidates = exact
    nodes = []
    for node in candidates:
        bounds = node.get("bounds")
        if not node.get("enabled") or node.get("hidden") or not bounds:
            continue
        if input_only and node["role"] not in {"textinput","multilinetextinput","searchinput","passwordinput"}:
            continue
        facts = controls.get(node["id"])
        visible = locator_rectangle(bounds, facts, window_rect)
        if visible is not None:
            nodes.append(dict(node, locator_rect=visible,
                              geometry_source="observed_hit_clip_window" if facts is not None
                              else "bounds_window_fallback_clip_unknown"))
    if not nodes:
        raise Refused(0, "locator_not_found", "no enabled visible widget named " + name)
    if nth is None and len(nodes) != 1:
        raise Refused(0, "ambiguous_locator", "candidates: " + json.dumps([{k:n.get(k) for k in ("id","name","role","bounds")} for n in nodes]))
    index = nth if nth is not None else 0
    if index < 0 or index >= len(nodes):
        raise Refused(0, "locator_index", "--nth is zero-based and outside the matching candidates")
    selected = nodes[index]
    bounds = require_object(selected["locator_rect"], "visible widget bounds")
    if bounds.get("w", 0) <= 0 or bounds.get("h", 0) <= 0:
        raise Refused(0, "empty_widget_bounds", "locator has no clickable rectangle")
    if evidence is not None:
        evidence.update({"id": selected["id"], "name": selected["name"], "role": selected["role"],
                         "geometry_source": selected["geometry_source"], "visible_rect": bounds})
    return bounds["x"] + bounds["w"] / 2, bounds["y"] + bounds["h"] / 2


def command_ui(run, args):
    params = {}
    if args.find:
        params["name"] = args.find
    if args.role:
        params["role"] = args.role
    if args.interactive:
        params["interactive"] = 1
    tree = probe(run, "ui-tree", args.verse, params)
    tree_nodes(tree)
    emit(tree)


def command_layout_check(run, args):
    from layout_check import analyze
    tree = probe(run, "ui-tree", args.verse)
    tree_nodes(tree)
    state = probe(run, "ui-state", args.verse)
    result = analyze(tree, state)
    emit(result)
    if result["violations"]:
        raise Refused(0, "layout_violations", str(len(result["violations"])) + " measured layout violations")


def dotted(value, path):
    for key in path.split("."):
        if not isinstance(value, dict) or key not in value:
            raise Refused(0, "state_path_missing", "state path not present: " + path)
        value = value[key]
    return value


def command_state(run, args):
    state = probe(run, "ui-state", args.verse)
    emit(dotted(state, args.path) if args.path else state)


def command_logs(run, args):
    params = {"since":args.since, "level":args.level}
    emit(probe(run, "logs", args.verse, params))


def qa_mutation(run, verse, endpoint, body):
    _verse, token, _camera = resolve_token(run, verse)
    _status, _headers, raw = call(run, "POST", "/api/v1/debug/" + endpoint, token, body)
    return decode_object(raw)


def command_dialog_answer(run, args):
    if args.cancel == bool(args.path):
        raise Refused(0, "invalid_dialog_answer", "provide either a path or --cancel")
    answer = {"kind":args.kind}
    if args.cancel:
        answer["cancel"] = True
    else:
        answer["path"] = args.path
    emit(qa_mutation(run, args.verse, "dialog-answers", {"answers":[answer]}))


def command_window(run, args):
    prior = probe(run, "ui-state", args.verse) if args.operation == "ui-scale" else None
    body = {"action":args.operation.replace("-", "_")}
    if args.operation == "size":
        body.update(width=args.width, height=args.height)
    elif args.operation == "ui-scale":
        if not math.isfinite(args.factor):
            raise Refused(0, "invalid_ui_scale", "UI scale must be finite")
        body["factor"] = args.factor
    emit(qa_mutation(run, args.verse, "window", body))
    def ready(state):
        window = require_object(state.get("window"), "window state")
        if args.operation == "size":
            return window.get("width") == args.width and window.get("height") == args.height
        if args.operation == "maximize":
            return window.get("maximized") is True
        if args.operation == "minimize":
            return window.get("minimized") is True
        if args.operation == "restore":
            return window.get("maximized") is False and window.get("minimized") is False
        if args.operation == "ui-scale":
            old_scale = require_object(prior.get("window"), "previous window state").get("ui_scale", 1.0)
            expected = prior.get("pixels_per_point", 1.0) * args.factor / old_scale
            return abs(window.get("ui_scale", 0) - args.factor) < 0.001 and abs(state.get("pixels_per_point", 0) - expected) < 0.01
        return "Close FractalEngine?" in state.get("open_windows", [])
    state = wait_until(lambda: probe(run, "ui-state", args.verse), ready, 15.0, "window " + args.operation)
    emit({"window_state":state.get("window"), "open_windows":state.get("open_windows")})


def wait_until(read, ready, timeout, description):
    if not math.isfinite(timeout) or timeout <= 0:
        raise Refused(0, "invalid_timeout", "timeout must be finite and positive")
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        value = read()
        if ready(value):
            return value
        time.sleep(min(0.25, max(0, deadline - time.monotonic())))
    raise Refused(0, "wait_timeout", "%s did not match within %.1fs" % (description, timeout))


def command_wait_for(run, args):
    if args.state and "=" not in args.state:
        raise Refused(0, "invalid_state_predicate", "--state requires PATH=JSON_VALUE")
    if args.state:
        path, text = args.state.split("=", 1)
        try:
            expected = json.loads(text)
        except ValueError:
            expected = text
    def poll():
        if args.name:
            found = [n for n in tree_nodes(probe(run, "ui-tree", args.verse, {"name":args.name})) if not n.get("hidden")]
            return not found if args.gone else bool(found)
        ready = dotted(probe(run, "ui-state", args.verse), path) == expected
        return not ready if args.gone else ready
    wait_until(poll, bool, args.timeout, args.name or args.state)
    emit({"matched":True, "predicate":args.name or args.state, "gone":args.gone})


def log_watermark(run, verse):
    try:
        snapshot = probe(run, "logs", verse, {"level":"warn"})
        generation, seq = snapshot.get("generation"), snapshot.get("latest_seq")
        if type(generation) is not int or generation < 0 or type(seq) is not int or seq < 0:
            raise Refused(0, "invalid_response", "log watermark must contain nonnegative integer counters")
        return generation, seq
    except (Refused, NotRunning):
        return None


def print_action_logs(run, verse, watermark):
    if watermark is None:
        emit({"new_logs_unavailable":"initial log watermark was unavailable"})
        return
    try:
        generation, seq = watermark
        snapshot = probe(run, "logs", verse, {"since":seq, "level":"warn"})
        entries = snapshot.get("entries")
        if not isinstance(entries, list):
            raise Refused(0, "invalid_response", "logs entries must be an array")
        after_generation, after_seq = snapshot.get("generation"), snapshot.get("latest_seq")
        if any(type(value) is not int or value < 0 for value in (after_generation, after_seq)):
            raise Refused(0, "invalid_response", "log after-watermark must contain nonnegative integer counters")
        changed = after_generation != generation
        delta = after_seq - seq if type(after_seq) is int else None
        emit({"new_logs":entries, "generation_changed":changed,
              "log_watermark":{"generation":generation,"seq":seq},
              "log_after":{"generation":after_generation,"seq":after_seq},
              "retention":{"possible_gap":changed or delta is None or delta < 0 or delta > 2000,
                           "seq_delta":delta,"source_derived_capacity":2000}})
    except (Refused, NotRunning) as error:
        emit({"new_logs_unavailable":str(error)})


# --------------------------------------------------------------------------
# CLI
# --------------------------------------------------------------------------

def build_parser():
    parser = argparse.ArgumentParser(prog="fe.py", description=__doc__,
                                     formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--verse", help="use this verse's token instead of probing for the active one")
    parser.add_argument("--run", help="explicit QA run directory, current.json, or run timestamp")
    sub = parser.add_subparsers(dest="command", required=True)

    sub.add_parser("info", help="run dir, pid, fixture ids, token roles").set_defaults(func=command_info)
    sub.add_parser("camera", help="GET /api/v1/debug/camera").set_defaults(func=command_camera)
    portal = sub.add_parser("portal-fixture", help="fixed owned native fixture page operations and CapturePreview PNG")
    portal_ops = portal.add_subparsers(dest="operation", required=True)
    for operation in ("observe", "second-link", "refused-third-link"):
        portal_ops.add_parser(operation)
    portal_capture = portal_ops.add_parser("capture")
    portal_capture.add_argument("--out", help="new .png beneath the selected run in an existing directory")
    portal.set_defaults(func=command_portal_fixture)
    answer = sub.add_parser("dialog-answer", help="queue a run-confined native dialog answer")
    answer.add_argument("path", nargs="?")
    answer.add_argument("--cancel", action="store_true")
    answer.add_argument("--kind", choices=["open","save","folder"], default="open")
    answer.set_defaults(func=command_dialog_answer)
    window = sub.add_parser("window", help="QA window controls; size uses physical pixels")
    actions = window.add_subparsers(dest="operation", required=True)
    size = actions.add_parser("size")
    size.add_argument("width", type=int)
    size.add_argument("height", type=int)
    scale = actions.add_parser("ui-scale")
    scale.add_argument("factor", type=float)
    for operation in ["maximize","restore","minimize","close"]:
        actions.add_parser(operation)
    window.set_defaults(func=command_window)
    sub.add_parser("layout-check", help="measured layout violations and explicit geometry coverage gaps").set_defaults(func=command_layout_check)
    ui = sub.add_parser("ui", help="primary window accessibility tree in physical pixels")
    ui.add_argument("--find")
    ui.add_argument("--role")
    ui.add_argument("--interactive", action="store_true")
    ui.set_defaults(func=command_ui)
    state = sub.add_parser("state", help="authoritative UI state")
    state.add_argument("--path")
    state.set_defaults(func=command_state)
    logs = sub.add_parser("logs", help="bounded redacted current-verse log tail")
    logs.add_argument("--since", type=int, default=0)
    logs.add_argument("--level", choices=("trace","debug","info","warn","error"), default="warn")
    logs.set_defaults(func=command_logs)
    wait = sub.add_parser("wait-for", help="wait for a named widget or state PATH=VALUE")
    predicate = wait.add_mutually_exclusive_group(required=True)
    predicate.add_argument("--name")
    predicate.add_argument("--state")
    wait.add_argument("--gone", action="store_true")
    wait.add_argument("--timeout", type=float, default=10)
    wait.set_defaults(func=command_wait_for)

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
    click.add_argument("x", type=float, nargs="?")
    click.add_argument("y", type=float, nargs="?")
    click.add_argument("--name")
    click.add_argument("--role")
    click.add_argument("--nth", type=int)
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
    text.add_argument("--into", help="click this named input before entering text")
    text.add_argument("--role")
    text.add_argument("--nth", type=int)

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

    invitation = sub.add_parser("transfer-invite", help="consume the latest GUI invitation once and drive masked Join in another owned app")
    invitation.add_argument("--recipient-run", required=True)
    invitation.add_argument("--recipient-verse")
    invitation.set_defaults(func=command_transfer_invite)

    api = sub.add_parser("api", help="raw REST call with a verse token (fixtures/probes only)")
    api.add_argument("method")
    api.add_argument("path")
    api.add_argument("body", nargs="?")
    api.set_defaults(func=command_api)
    return parser


def main(argv=None):
    args = build_parser().parse_args(argv)
    try:
        run = load_run(args.run)
        observe_logs = args.command in {"move","click","drag","key","type","scroll","dialog-answer","window","portal-fixture"}
        watermark = log_watermark(run, args.verse) if observe_logs else None
        try:
            args.func(run, args)
        finally:
            if observe_logs:
                print_action_logs(run, args.verse, watermark)
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
