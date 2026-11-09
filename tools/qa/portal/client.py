"""Client for the fixed /api/v1/debug/portal/* routes (plan T5.4); see tools/qa/portal/AGENTS.md section native-routes.

The module takes the caller's transport so tools/qa/fe.py and tools/qa/sim can both use it:
`call(method, path, body=None, timeout=8.0) -> (status, headers, raw_bytes)` and it raises on refusals itself.
No caller supplies JavaScript, selectors or URLs: the only inputs are a slot number 1..32 and nothing else.
"""
import json

BASE = "/api/v1/debug/portal/"
SLOT_MIN, SLOT_MAX = 1, 32
PROBE_TEXT_MAX = 256
PROBE_COUNT_MAX = 64
MODES = ("admitted", "static")
STATS_COUNTERS = ("accepted", "refused_capability", "refused_role", "rate_limited", "dropped_origin", "invalid",
                  "failed", "too_many_subscriptions", "live_subscriptions", "deliveries")


class PortalError(Exception):
    """A refusal or a reply that does not match its fixed shape; `code` mirrors the route's code field."""

    def __init__(self, code, message):
        super().__init__("%s: %s" % (code, message))
        self.code = code
        self.message = message


def _object(raw, what):
    try:
        value = json.loads(raw)
    except (TypeError, ValueError) as error:
        raise PortalError("portal_reply_invalid", "%s is not JSON" % what) from error
    if not isinstance(value, dict):
        raise PortalError("portal_reply_invalid", "%s is not an object" % what)
    return value


def _data(raw):
    envelope = _object(raw, "portal reply")
    if envelope.get("ok") is not True:
        raise PortalError(str(envelope.get("code") or "portal_refused"), str(envelope.get("error") or "refused"))
    data = envelope.get("data")
    if not isinstance(data, dict) or not isinstance(data.get("portal"), dict):
        raise PortalError("portal_reply_invalid", "reply has no portal evidence")
    return data


def probe_digest(probes):
    """BLAKE3 over probe texts in name order, each followed by NUL; None when the blake3 module is absent."""
    try:
        import blake3
    except ImportError:
        return None
    hasher = blake3.blake3()
    for name in sorted(probes):
        hasher.update(probes[name].encode("utf-8"))
        hasher.update(b"\0")
    return hasher.hexdigest()


def validate_observation(portal):
    """Strict shape check of an observe reply; also recomputes the digest when blake3 is importable."""
    probes = portal.get("probes")
    if portal.get("mode") not in MODES or not isinstance(portal.get("label"), str) or not isinstance(portal.get("title"), str):
        raise PortalError("portal_reply_invalid", "observation lacks label, mode or title")
    if not isinstance(probes, dict) or len(probes) > PROBE_COUNT_MAX or not all(
            isinstance(k, str) and isinstance(v, str) and len(v) <= PROBE_TEXT_MAX for k, v in probes.items()):
        raise PortalError("portal_reply_invalid", "observation probes are malformed")
    expected = probe_digest(probes)
    if expected is not None and portal.get("digest") != expected:
        raise PortalError("portal_digest_mismatch", "observation digest does not match its probe texts")
    return portal


def validate_stats(portal):
    """Every documented counter present as a non-negative integer."""
    stats = portal.get("stats")
    if not isinstance(stats, dict):
        raise PortalError("portal_reply_invalid", "ipc-stats reply has no stats object")
    for counter in STATS_COUNTERS:
        value = stats.get(counter)
        if not isinstance(value, int) or isinstance(value, bool) or value < 0:
            raise PortalError("portal_reply_invalid", "ipc counter %s is missing or not a count" % counter)
    return portal


class PortalClient:
    """Fixed observe / slot / ipc-stats operations over one transport."""

    def __init__(self, call):
        self.call = call

    def observe(self):
        _status, _headers, raw = self.call("GET", BASE + "observe", None, 8.0)
        data = _data(raw)
        validate_observation(data["portal"])
        return data

    def slot(self, number):
        if isinstance(number, bool) or not isinstance(number, int) or not SLOT_MIN <= number <= SLOT_MAX:
            raise PortalError("invalid_slot", "slot must be an integer %d..%d" % (SLOT_MIN, SLOT_MAX))
        _status, _headers, raw = self.call("POST", BASE + "action", {"slot": number}, 8.0)
        data = _data(raw)
        if not isinstance(data["portal"].get("activated"), bool):
            raise PortalError("portal_reply_invalid", "action reply lacks activated")
        return data

    def ipc_stats(self):
        _status, _headers, raw = self.call("GET", BASE + "ipc-stats", None, 8.0)
        data = _data(raw)
        validate_stats(data["portal"])
        return data


def run_command(operation, slot, call, emit):
    """Entry point for `fe.py portal observe|slot|ipc-stats`; `emit` prints one JSON object."""
    client = PortalClient(call)
    if operation == "observe":
        emit(client.observe())
    elif operation == "slot":
        emit(client.slot(slot))
    elif operation == "ipc-stats":
        emit(client.ipc_stats())
    else:
        raise PortalError("invalid_operation", "operation must be observe, slot or ipc-stats")
