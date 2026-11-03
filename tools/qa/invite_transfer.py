"""One-use invitation handoff between pinned owned apps; see AGENTS.md."""

import contextlib
import ctypes
from ctypes import wintypes
import json
import ntpath
import os


class OwnedApp:
    """Retain an exact process handle through the complete transfer."""

    def __init__(self, qa, run):
        self.qa = qa
        self.identity = qa.require_object(run.get("process_identity"), "owned process identity")
        self.handle = None

    def __enter__(self):
        if os.name != "nt":
            raise self.qa.Refused(0, "owned_run_required", "Invitation transfer requires Windows owned-run handles")
        self.kernel = ctypes.WinDLL("kernel32", use_last_error=True)
        self.kernel.OpenProcess.argtypes = [wintypes.DWORD, wintypes.BOOL, wintypes.DWORD]
        self.kernel.OpenProcess.restype = wintypes.HANDLE
        self.kernel.CloseHandle.argtypes = [wintypes.HANDLE]
        self.kernel.CloseHandle.restype = wintypes.BOOL
        self.kernel.WaitForSingleObject.argtypes = [wintypes.HANDLE, wintypes.DWORD]
        self.kernel.WaitForSingleObject.restype = wintypes.DWORD
        self.kernel.QueryFullProcessImageNameW.argtypes = [wintypes.HANDLE, wintypes.DWORD, wintypes.LPWSTR, ctypes.POINTER(wintypes.DWORD)]
        self.kernel.QueryFullProcessImageNameW.restype = wintypes.BOOL
        self.kernel.GetProcessTimes.argtypes = [wintypes.HANDLE] + [ctypes.POINTER(wintypes.FILETIME)] * 4
        self.kernel.GetProcessTimes.restype = wintypes.BOOL
        pid = self.identity.get("pid")
        ticks = self.identity.get("start_ticks")
        image = self.identity.get("exe_path")
        if type(pid) is not int or not 0 < pid <= 0xFFFFFFFF or not isinstance(ticks, str) or not ticks.isdecimal() or not isinstance(image, str) or not ntpath.isabs(image):
            raise self.qa.Refused(0, "owned_run_required", "Complete owned process identity is required")
        self.handle = self.kernel.OpenProcess(0x1000 | 0x100000, False, pid)
        if not self.handle:
            raise self.qa.Refused(0, "owned_run_required", "The recorded app lifetime cannot be pinned")
        try:
            creation, exit_time, kernel_time, user_time = (wintypes.FILETIME() for _ in range(4))
            if not self.kernel.GetProcessTimes(self.handle, ctypes.byref(creation), ctypes.byref(exit_time), ctypes.byref(kernel_time), ctypes.byref(user_time)):
                raise self.qa.Refused(0, "owned_run_required", "The pinned app creation time is unavailable")
            actual_ticks = (creation.dwHighDateTime << 32 | creation.dwLowDateTime) + 504911232000000000
            capacity = wintypes.DWORD(32768)
            actual_image = ctypes.create_unicode_buffer(capacity.value)
            if not self.kernel.QueryFullProcessImageNameW(self.handle, 0, actual_image, ctypes.byref(capacity)):
                raise self.qa.Refused(0, "owned_run_required", "The pinned app executable is unavailable")
            if actual_ticks != int(ticks) or ntpath.normcase(ntpath.normpath(actual_image.value)) != ntpath.normcase(ntpath.normpath(image)):
                raise self.qa.Refused(0, "owned_run_required", "The recorded app lifetime or executable changed")
            self.check()
            return self
        except BaseException:
            self.__exit__(None, None, None)
            raise

    def check(self):
        if self.kernel.WaitForSingleObject(self.handle, 0) != 258:
            raise self.qa.Refused(0, "owned_run_ended", "A pinned app lifetime ended; transfer is unconfirmed")

    def __exit__(self, *_):
        if self.handle:
            self.kernel.CloseHandle(self.handle)
            self.handle = None


def join_controls(qa, run, verse):
    tree = qa.probe(run, "ui-tree", verse)
    nodes = qa.tree_nodes(tree)
    window = qa.require_object(tree.get("window"), "primary window")
    clip = {"x": 0, "y": 0, "w": window.get("width"), "h": window.get("height")}
    controls = {item["id"]: item for item in tree.get("layout", {}).get("controls", [])}

    def center(name, role, enabled):
        matches = [node for node in nodes if node["name"] == name and node["role"] == role
                   and not node.get("hidden") and (not enabled or node.get("enabled"))]
        if len(matches) != 1:
            raise qa.Refused(0, "join_control_unavailable", "The clean masked Join controls are not uniquely available")
        node = matches[0]
        facts = controls.get(node["id"])
        if facts is None:
            raise qa.Refused(0, "join_control_unavailable", "Observed Join control geometry is required")
        visible = qa.locator_rectangle(node.get("bounds"), facts, clip)
        if visible is None:
            raise qa.Refused(0, "join_control_unavailable", "The actual Join control is outside the current modal")
        return visible["x"] + visible["w"] / 2, visible["y"] + visible["h"] / 2

    return center("Verse invite string", "passwordinput", True), center("Join", "button", False)


def transfer(qa, owner, args):
    recipient = qa.load_run(args.recipient_run)
    if owner["run_dir"] == recipient["run_dir"] or owner["api_base"] == recipient["api_base"]:
        raise qa.Refused(0, "distinct_owned_runs_required", "Invitation transfer requires two distinct owned apps")
    with contextlib.ExitStack() as stack:
        owned = [stack.enter_context(OwnedApp(qa, run)) for run in (owner, recipient)]
        if owned[0].identity["pid"] == owned[1].identity["pid"]:
            raise qa.Refused(0, "distinct_owned_runs_required", "The two run records must identify different app lifetimes")

        def check():
            for app in owned:
                app.check()

        check()
        owner_verse, owner_token, _ = qa.resolve_token(owner, args.verse)
        recipient_verse, recipient_token, _ = qa.resolve_token(recipient, args.recipient_verse)
        check()

        def request(run, method, token, body=None):
            check()
            status, headers, raw = qa.http(run, method, "/api/v1/debug/invitation", token, body, timeout=5.0)
            check()
            cache_control = next((value for key, value in headers.items() if key.casefold() == "cache-control"), None)
            if not 200 <= status < 300 or cache_control != "no-store":
                raise qa.Refused(status, "invitation_transfer_unconfirmed", "The one-use invitation response was not confirmed; do not replay consumption")
            try:
                envelope = json.loads(raw)
                if envelope.get("ok") is not True or not isinstance(envelope.get("data"), dict):
                    raise ValueError()
                return envelope["data"]
            except (ValueError, TypeError, AttributeError):
                raise qa.Refused(0, "invitation_transfer_unconfirmed", "The one-use response is invalid; do not replay consumption") from None

        owner_observation = request(owner, "GET", owner_token)
        recipient_observation = request(recipient, "GET", recipient_token)
        issuer = owner_observation.get("local_did")
        recipient_did = recipient_observation.get("local_did")
        expected = owner_observation.get("invitation")
        if not isinstance(issuer, str) or not isinstance(recipient_did, str) or issuer == recipient_did or not isinstance(expected, dict):
            raise qa.Refused(0, "addressed_invitation_required", "Generate an addressed invitation for the other owned app first")
        if expected.get("issuer_did") != issuer or expected.get("recipient_did") != recipient_did or expected.get("verse_id") != owner_verse:
            raise qa.Refused(0, "recipient_identity_mismatch", "The current GUI invitation must name the actual other app identity")
        if recipient_observation.get("join_ready_for_input") is not True:
            raise qa.Refused(0, "clean_join_required", "Open a clean, empty Join dialog on the recipient before transferring")
        text_at, join_at = join_controls(qa, recipient, recipient_verse)
        check()
        # Recheck both actual DIDs after locating controls, before one-use consumption.
        current_owner = request(owner, "GET", owner_token)
        current_recipient = request(recipient, "GET", recipient_token)
        if current_owner.get("local_did") != issuer or current_owner.get("invitation") != expected or current_recipient.get("local_did") != recipient_did or current_recipient.get("join_ready_for_input") is not True:
            raise qa.Refused(0, "recipient_identity_mismatch", "An app identity, invitation generation or clean Join context changed before consumption")
        consumed = request(owner, "POST", owner_token, expected)
        try:
            credential = consumed.get("invitation")
            if consumed.get("metadata") != expected or not isinstance(credential, str) or not credential.startswith("fractalengine://join/v1/") or not credential.isascii() or not 0 < len(credential.encode("utf-8")) <= 65536:
                raise ValueError()
            after_owner = request(owner, "GET", owner_token)
            after_recipient = request(recipient, "GET", recipient_token)
            if after_owner.get("local_did") != issuer or after_recipient.get("local_did") != recipient_did or after_recipient.get("join_ready_for_input") is not True:
                raise ValueError()
            commands = [{"kind": "pointer_click", "x": text_at[0], "y": text_at[1], "button": "left"},
                        {"kind": "key_press", "key": "ControlLeft"}, {"kind": "key_press", "key": "KeyA"}]
            commands.extend({"kind": "text_entry", "text": credential[index:index+4096]} for index in range(0, len(credential), 4096))
            commands.append({"kind": "pointer_click", "x": join_at[0], "y": join_at[1], "button": "left"})
            body = {"commands": commands, "wait_frames": 1, "capture_after": False}
            if len(json.dumps(body).encode("utf-8")) > 65536:
                raise ValueError()
            check()
            status, _, _ = qa.http(recipient, "POST", "/api/v1/debug/input", recipient_token, body, timeout=30.0)
            check()
            if not 200 <= status < 300:
                raise ValueError()
        except Exception:
            raise qa.Refused(0, "masked_join_submission_unconfirmed", "The invitation was consumed but masked Join submission is unconfirmed; do not replay it") from None
        finally:
            consumed.clear()
            credential = None
            commands = None
            body = None
        return {"transfer": "consumed_once", "input_submitted": True,
                "issuer_did": issuer, "recipient_did": recipient_did,
                "generation": expected["generation"], "scope": expected["scope"], "role": expected["role"],
                "join_ready": "pending_real_transport_and_role_confirmation"}
