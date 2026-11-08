"""Per-peer REST/MCP/GUI client on fe.py's loopback transport (in-process, no subprocess per action); see tools/qa/AGENTS.md §sim."""
import json
import os
import sys
import threading
import time
import urllib.error
import urllib.request
import uuid

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, os.path.dirname(HERE))
import fe  # noqa: E402
import reveal  # noqa: E402

INPUT_WAIT_FRAMES = 5
RUN_RELOAD_SECONDS = 2.0
WIDGET_WAIT_S = 5.0  # default budget to poll for an enabled visible widget (args.wait_s overrides, 0 refuses at once)
WIDGET_POLL_S = 0.1
WIDGET_POLLED_REFUSALS = ("locator_not_found", "locator_index")  # index: fewer candidates than nth+1 rendered so far
SCROLL_SETTLE_S = 0.35  # egui eases a wheel scroll over a few frames: read the ui-tree again only after it


class Outcome(dict):
    """One request's normalized result: status, ok, refused, code, data, error (+ token_source)."""

    @property
    def ok(self):
        return self.get("ok") is True


def unwrap(payload):
    """Strip the API's {ok,data} envelope (some routes nest data.data)."""
    data = payload.get("data", payload) if isinstance(payload, dict) else payload
    while isinstance(data, dict) and isinstance(data.get("data"), (dict, list)) and "ok" not in data:
        data = data["data"]
    return data


def extract_id(data):
    """First id-like field of a create/upload result."""
    if isinstance(data, dict):
        for key in ("id", "node_id", "asset_id", "fractal_id", "petal_id", "group_id", "verse_id"):
            if isinstance(data.get(key), str):
                return data[key]
        for value in data.values():
            if isinstance(value, dict):
                found = extract_id(value)
                if found:
                    return found
    return None


def _ambiguous_candidates(refusal):
    if refusal.code != "ambiguous_locator" or "candidates: " not in str(refusal.message):
        return None
    try:
        return json.loads(str(refusal.message).split("candidates: ", 1)[1])
    except ValueError:
        return None


def _is_timeout(error):
    """True when a transport failure was a socket timeout (peer up but silent), not a refused or reset connection."""
    return (isinstance(error, TimeoutError) or isinstance(getattr(error, "reason", None), TimeoutError)
            or "timed out" in str(error).lower())


class PeerClient:
    """One peers.ps1 member: token selection, REST/MCP requests and synthetic GUI input."""

    def __init__(self, member, verse_id):
        self.name = member["name"]
        self.role = member.get("role")
        self.run_dir = member["run_dir"]
        self.did = member.get("public_did")
        self.verse_id = verse_id
        self.gui_lock = threading.Lock()
        self._run = None
        self._run_loaded = 0.0
        self._stale_tokens = {}
        self._gui_token = None
        self._waited_ms = 0
        self._scrolled = 0  # wheel inputs the current gui op sent to reveal a clipped widget
        self._clipped = None  # why the last refusal's widget was present but unreachable (reveal.Reveal.note)
        self._pixels_per_line = None  # calibrated physical px per wheel line (reveal.Reveal), kept across ops
        self._lock = threading.Lock()

    # -- discovery ---------------------------------------------------------------------------
    def run(self, force=False):
        with self._lock:
            if force or self._run is None or time.monotonic() - self._run_loaded > RUN_RELOAD_SECONDS:
                self._run = fe.load_run(self.run_dir)
                self._run_loaded = time.monotonic()
            return self._run

    def tokens(self):
        try:
            return fe.load_tokens(self.run())
        except (fe.NotRunning, fe.Refused, OSError, ValueError):
            return {}

    def role_in(self, verse_id):
        return (self.tokens().get("roles") or {}).get(verse_id)

    def verse_token(self, verse_id=None):
        """(token, source): the file token, else the last one seen (a revoked peer keeps presenting it)."""
        verse_id = verse_id or self.verse_id
        token = (self.tokens().get("tokens") or {}).get(verse_id)
        if token:
            self._stale_tokens[verse_id] = token
            return token, "file"
        if verse_id in self._stale_tokens:
            return self._stale_tokens[verse_id], "cached_stale"
        return None, "none"

    def debug_token(self):
        tokens = self.tokens()
        if (tokens.get("roles") or {}).get(self.verse_id) == "owner":
            return (tokens.get("tokens") or {}).get(self.verse_id)
        return tokens.get("operator_token") or (tokens.get("tokens") or {}).get(self.verse_id)

    def blob_path(self, content_hash):
        return os.path.join(self.run_dir, "data", "blobs", content_hash[:2], content_hash)

    # -- transport ---------------------------------------------------------------------------
    def send(self, method, path, body=None, token=None, timeout=30.0, raw=None, content_type=None, retry_reads=True):
        """Outcome of one request; refusals and a dead peer are data, never exceptions."""
        try:
            run = self.run()
        except (fe.NotRunning, fe.Refused, OSError, ValueError) as error:
            return Outcome(status=-1, ok=False, refused=False, code="not_running", error=str(error)[:300])
        headers = {}
        data = raw
        if token:
            headers["Authorization"] = "Bearer " + token
        if body is not None:
            data = json.dumps(body).encode("utf-8")
            headers["Content-Type"] = "application/json"
        elif content_type:
            headers["Content-Type"] = content_type
        attempts = 3 if (retry_reads and method == "GET") else 1
        for attempt in range(attempts):
            request = urllib.request.Request(run["api_base"] + path, data=data, method=method, headers=headers)
            try:
                with fe.HTTP_OPENER.open(request, timeout=timeout) as response:
                    status, payload_raw = response.status, response.read()
            except urllib.error.HTTPError as error:
                status, payload_raw = error.code, error.read()
            except fe.Refused as refusal:
                return Outcome(status=refusal.status, ok=False, refused=True, code=refusal.code, error=refusal.message)
            except (urllib.error.URLError, ConnectionError, TimeoutError, OSError) as error:
                return Outcome(status=-1, ok=False, refused=False, code="not_running", error=str(error)[:300],
                               timed_out=_is_timeout(error))
            if status == 429 and attempt + 1 < attempts:
                time.sleep(0.5)
                continue
            break
        return self._outcome(status, payload_raw)

    @staticmethod
    def _outcome(status, payload_raw):
        try:
            payload = json.loads(payload_raw.decode("utf-8")) if payload_raw else {}
        except (ValueError, UnicodeDecodeError):
            payload = {"raw": payload_raw[:300].decode("utf-8", "replace")}
        ok = 200 <= status < 300 and not (isinstance(payload, dict) and payload.get("ok") is False)
        out = Outcome(status=status, ok=ok, refused=not ok, data=unwrap(payload))
        if not ok and isinstance(payload, dict):
            error = payload.get("error") or payload.get("message")
            out["code"] = payload.get("code") or "http_%d" % status
            out["error"] = (json.dumps(error) if isinstance(error, dict) else str(error or ""))[:300]
        return out

    def rest(self, method, path, body=None, timeout=30.0):
        token, source = self.verse_token()
        if not token:
            return Outcome(status=0, ok=False, refused=True, code="no_token_for_verse", token_source=source)
        out = self.send(method, path, body, token, timeout)
        out["token_source"] = source
        return out

    def mcp(self, tool, arguments, timeout=60.0):
        token, source = self.verse_token()
        if not token:
            return Outcome(status=0, ok=False, refused=True, code="no_token_for_verse", token_source=source)
        body = {"jsonrpc": "2.0", "id": uuid.uuid4().hex[:8], "method": "tools/call",
                "params": {"name": tool, "arguments": arguments}}
        out = self.send("POST", "/mcp", body, token, timeout)
        out["token_source"] = source
        data = out.get("data")
        if isinstance(data, dict) and isinstance(data.get("error"), dict):
            out.update(ok=False, refused=True, code="mcp_error_%s" % data["error"].get("code"),
                       error=str(data["error"].get("message"))[:300])
            return out
        result = data.get("result") if isinstance(data, dict) else None
        if isinstance(result, dict):
            content = result.get("structuredContent")
            if content is None:
                texts = [c.get("text") for c in result.get("content") or [] if isinstance(c, dict)]
                try:
                    content = json.loads(texts[0]) if texts else None
                except (ValueError, TypeError):
                    content = {"text": texts[0][:500]}
            out["data"] = content
            if result.get("isError"):
                out.update(ok=False, refused=True, code="mcp_is_error", error=json.dumps(content)[:300])
        return out

    def upload_multipart(self, petal_id, file_path, name=None, timeout=300.0):
        """REST multipart GLB upload (the MCP door caps at 11 MiB)."""
        token, source = self.verse_token()
        if not token:
            return Outcome(status=0, ok=False, refused=True, code="no_token_for_verse", token_source=source)
        boundary = "fe-sim-" + uuid.uuid4().hex
        with open(file_path, "rb") as handle:
            content = handle.read()
        filename = name or os.path.basename(file_path)
        head = ("--%s\r\nContent-Disposition: form-data; name=\"file\"; filename=\"%s\"\r\n"
                "Content-Type: model/gltf-binary\r\n\r\n" % (boundary, filename)).encode("utf-8")
        body = head + content + ("\r\n--%s--\r\n" % boundary).encode("utf-8")
        out = self.send("POST", "/api/v1/petals/%s/assets" % petal_id, None, token, timeout, raw=body,
                        content_type="multipart/form-data; boundary=" + boundary)
        out["token_source"] = source
        return out

    # -- GUI ---------------------------------------------------------------------------------
    def prime_gui(self):
        """Resolve the view token before a barrier so the fire instant carries only the input request."""
        try:
            _verse, token, _camera = fe.resolve_token(self.run())
            self._gui_token = (time.monotonic(), token)
        except (fe.Refused, fe.NotRunning):
            self._gui_token = None

    def gui_input(self, commands):
        """Post one synthetic input batch with the token covering the active view (fe.py run_input semantics)."""
        run = self.run()
        primed, self._gui_token = self._gui_token, None
        if primed and time.monotonic() - primed[0] < 5.0:
            token = primed[1]
        else:
            _verse, token, _camera = fe.resolve_token(run)
        _status, _headers, raw = fe.call(run, "POST", "/api/v1/debug/input", token,
                                         {"commands": commands, "wait_frames": INPUT_WAIT_FRAMES}, timeout=30.0)
        return fe.decode_object(raw)

    def gui(self, op, args):
        """Locator-driven GUI op (fe.py click --name / type --into / key / scroll / wait-for); waited_ms = widget polling time, scrolled = wheel inputs that revealed a clipped widget."""
        with self.gui_lock:
            self._waited_ms = 0
            self._scrolled = 0
            self._clipped = None
            try:
                data = self._gui(op, args)
                return Outcome(status=200, ok=True, refused=False, data=data, waited_ms=self._waited_ms,
                               scrolled=self._scrolled)
            except fe.Refused as refusal:
                message = str(refusal.message)
                if self._clipped and refusal.code in WIDGET_POLLED_REFUSALS:
                    message += " (present but not reachable: %s%s)" % (
                        self._clipped.get("reason"), "; " + self._clipped["why_not"] if self._clipped.get("why_not") else "")
                return Outcome(status=refusal.status, ok=False, refused=True, code=refusal.code, error=message[:300],
                               waited_ms=self._waited_ms, scrolled=self._scrolled, clipped=self._clipped)
            except fe.NotRunning as error:
                return Outcome(status=-1, ok=False, refused=False, code="not_running", error=str(error)[:300],
                               waited_ms=self._waited_ms, scrolled=self._scrolled)

    @staticmethod
    def _widget_wait(args):
        """Seconds an action polls for its widget: args.wait_s, else WIDGET_WAIT_S."""
        return float(args.get("wait_s", WIDGET_WAIT_S))

    def _locate(self, args, input_only=False, wait_s=0.0):
        """(x, y, evidence) of an enabled visible widget.

        Polls WIDGET_POLLED_REFUSALS every WIDGET_POLL_S up to wait_s; while the match exists but sits outside its
        panel's clip, wheels that panel toward it in the same budget (reveal.py), so wait_s=0 never scrolls.
        """
        started = time.monotonic()
        waited = False
        sent_total = 0
        scrolling = reveal.Reveal(self._pixels_per_line)
        try:
            while True:
                try:
                    x, y, evidence = self._locate_once(args, input_only)
                except fe.Refused as refusal:
                    remaining = started + wait_s - time.monotonic()
                    polled = refusal.code in WIDGET_POLLED_REFUSALS
                    diagnostics = getattr(refusal, "diagnostics", None) if polled else None
                    wheel = scrolling.step(diagnostics, allow=remaining > 0) if diagnostics else None
                    self._clipped = scrolling.note if diagnostics else None
                    if not polled or remaining <= 0:
                        raise
                    waited = True
                    sent = wheel is not None and self._wheel(scrolling, wheel)
                    sent_total += 1 if sent else 0
                    time.sleep(min(SCROLL_SETTLE_S if sent else WIDGET_POLL_S, remaining))
                    continue
                if sent_total and not scrolling.settled(x, y) and started + wait_s > time.monotonic():
                    waited = True  # the row may still be sliding: click only once two reads agree
                    time.sleep(WIDGET_POLL_S)
                    continue
                self._clipped = None
                if waited:
                    evidence["waited_ms"] = round((time.monotonic() - started) * 1000)
                if sent_total:
                    evidence["scrolled"] = sent_total
                return x, y, evidence
        finally:
            if waited:
                self._waited_ms += round((time.monotonic() - started) * 1000)
            self._pixels_per_line = scrolling.pixels_per_line or self._pixels_per_line

    def _wheel(self, scrolling, wheel):
        """Send one reveal scroll (pointer move + wheel); False when the host refused it, so the next read judges the stall."""
        try:
            self.gui_input(scrolling.commands(wheel))
        except fe.Refused:
            return False
        self._scrolled += 1
        return True

    def _locate_once(self, args, input_only=False):
        evidence = {}
        diagnostics = {}
        try:
            x, y = fe.locate(self.run(), args["name"], args.get("role"), args.get("nth"), None,
                             input_only=input_only, evidence=evidence, diagnostics=diagnostics)
        except fe.Refused as refusal:
            # Duplicate affordances (sidebar "Open" + browser card) share one exact name and role: take the
            # first and record the count; genuinely different candidates stay a refusal.
            candidates = _ambiguous_candidates(refusal)
            if not candidates or len({(c.get("name", "").casefold(), c.get("role")) for c in candidates}) != 1:
                refusal.diagnostics = diagnostics  # the clipped / unplaced matches, for the caller's reveal scroll
                raise
            x, y = fe.locate(self.run(), args["name"], args.get("role"), 0, None,
                             input_only=input_only, evidence=evidence)
            evidence["duplicates"] = len(candidates)
        return x, y, evidence

    def _gui(self, op, args):
        if op == "click":
            x, y, evidence = self._locate(args, wait_s=self._widget_wait(args))
            click = {"kind": "pointer_click", "x": x, "y": y, "button": args.get("button", "left")}
            mods = [{"kind": "key_press", "key": fe.MODIFIER_KEYS[m]} for m in args.get("modifiers", [])]
            commands = mods + [click] + ((mods + [dict(click)]) if args.get("double") else [])
            return {"locator": evidence, "outcome": self.gui_input(commands)}
        if op == "click_at":
            x, y = args.get("x"), args.get("y")
            if args.get("viewport_frac"):
                # Scene-viewport-relative point (tile sizes differ): ui-state reports the viewport in physical px.
                viewport = fe.probe(self.run(), "ui-state", None).get("viewport") or {}
                fx, fy = args["viewport_frac"]
                x = viewport.get("x", 0) + viewport.get("w", 0) * float(fx)
                y = viewport.get("y", 0) + viewport.get("h", 0) * float(fy)
            return {"outcome": self.gui_input([{"kind": "pointer_click", "x": x, "y": y,
                                                "button": args.get("button", "left")}])}
        if op == "type":
            located = None
            if args.get("into"):
                x, y, located = self._locate({"name": args["into"], "role": args.get("role"),
                                              "nth": args.get("nth")}, input_only=True,
                                             wait_s=self._widget_wait(args))
                self.gui_input([{"kind": "pointer_click", "x": x, "y": y, "button": "left"}])
                if args.get("select_all", True):
                    self.gui_input([{"kind": "key_press", "key": k} for k in fe.parse_combo("ctrl+a")])
            return {"locator": located, "outcome": self.gui_input([{"kind": "text_entry", "text": str(args["text"])}])}
        if op == "key":
            located = None
            if args.get("name"):  # optional guard: hold the keys until this widget is enabled and visible
                _x, _y, located = self._locate(args, wait_s=self._widget_wait(args))
            combos = args.get("combos") or [args["combo"]]
            commands = [{"kind": "key_press", "key": key} for combo in combos for key in fe.parse_combo(combo)]
            outcome = self.gui_input(commands)
            return {"locator": located, "outcome": outcome} if located else {"outcome": outcome}
        if op == "scroll":
            commands = []
            if args.get("x") is not None:
                commands.append({"kind": "pointer_move", "x": args["x"], "y": args["y"]})
            commands.append({"kind": "scroll", "dx": args.get("dx", 0), "dy": args.get("dy", 0)})
            return {"outcome": self.gui_input(commands)}
        if op == "scroll_until":
            # Scroll at a window-relative point until a locator is visible (layouts differ per tile size).
            for attempt in range(int(args.get("max", 6)) + 1):
                try:
                    self._locate({"name": args["until"], "role": args.get("role")})
                    return {"scrolls": attempt}
                except fe.Refused as refusal:
                    if refusal.code != "locator_not_found" or attempt == int(args.get("max", 6)):
                        raise
                tree = fe.probe(self.run(), "ui-tree", None, {"name": args["until"]})
                window = tree.get("window") or {}
                x = float(window.get("width", 1280)) * float(args.get("x_frac", 0.87))
                y = float(window.get("height", 720)) * float(args.get("y_frac", 0.55))
                self.gui_input([{"kind": "pointer_move", "x": x, "y": y},
                                {"kind": "scroll", "dx": 0, "dy": float(args.get("dy", -3))}])
                time.sleep(0.2)
            raise fe.Refused(0, "scroll_until_exhausted", args["until"])
        if op == "wait_for":
            deadline = time.monotonic() + float(args.get("timeout_s", 10))
            while time.monotonic() < deadline:
                tree = fe.probe(self.run(), "ui-tree", None, {"name": args["name"]})
                found = [n for n in fe.tree_nodes(tree) if not n.get("hidden")]
                if bool(found) != bool(args.get("gone")):
                    return {"matched": True}
                time.sleep(0.25)
            raise fe.Refused(0, "wait_timeout", "%s did not %s" % (args["name"], "vanish" if args.get("gone") else "appear"))
        if op == "click_until":
            # Repeat a click (e.g. Back) until a target locator is present; bounded by max.
            for attempt in range(int(args.get("max", 4)) + 1):
                tree = fe.probe(self.run(), "ui-tree", None, {"name": args["until"], "interactive": 1})
                if [n for n in fe.tree_nodes(tree) if not n.get("hidden") and n.get("enabled")]:
                    return {"clicks": attempt}
                if attempt == int(args.get("max", 4)):
                    break
                x, y, _evidence = self._locate(args["click"], wait_s=self._widget_wait(args))
                self.gui_input([{"kind": "pointer_click", "x": x, "y": y, "button": "left"}])
                time.sleep(float(args.get("pause_s", 0.4)))
            raise fe.Refused(0, "click_until_exhausted", "%s never appeared" % args["until"])
        raise fe.Refused(0, "unknown_gui_op", op)

    def screenshot(self, out_path, capture="scene", pose=None, timeout=60.0):
        """Checkpoint capture; scene mode (include_ui=false) by default so the user's window is not re-posed."""
        body = {}
        if pose:
            body["pose"] = pose
        if capture == "scene":
            body["include_ui"] = False
        run = self.run()
        _verse, token, _camera = fe.resolve_token(run)
        _status, headers, raw = fe.call(run, "POST", "/api/v1/debug/screenshot", token, body, timeout=timeout)
        os.makedirs(os.path.dirname(out_path), exist_ok=True)
        with open(out_path, "wb") as handle:
            handle.write(raw)
        return {"path": out_path, "bytes": len(raw), "request": body}
