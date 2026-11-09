"""Portal probe family and the upload_bundle driver op (plan T5.3); see tools/qa/portal/AGENTS.md section sim-contract.

Read-only: the probe calls the fixed /api/v1/debug/portal/{observe,ipc-stats} routes and the admission summary read,
never the hierarchy. SI wires `collect` into prober.Prober._round (cadence `portal_ms`) and `upload_bundle` into the
driver's rest ops.
"""
import json
import os
import uuid

import predicates
from runtime import small

BUNDLE_MIME = "application/vnd.fractalengine.portal-bundle+hexon"
DEBUG_BASE = "/api/v1/debug/portal/"
ALL_TARGET = "*"
IPC_COUNTERS = ("accepted", "refused_capability", "refused_role", "rate_limited", "dropped_origin", "invalid",
                "failed", "too_many_subscriptions", "live_subscriptions", "deliveries",
                # Host-aggregated extras the scenarios read; absent in a stats reply they read as None.
                "writes_accepted", "delivered_non_control", "dropped_origin_total")
# Debug refusals that say a bundle is not mounted: the observation records them as {"mounted": false}.
NOT_MOUNTED_CODES = {"portal_bundle_not_mounted", "portal_fixture_unavailable", "portal_fixture_changed"}


def upload_bundle(client, args, timeout=300.0):
    """Driver op `upload_bundle`: multipart POST of one built archive to the petal's portal-bundles route."""
    token, source = client.verse_token()
    if not token:
        return client_outcome_refused("no_token_for_verse", source)
    boundary = "fe-sim-" + uuid.uuid4().hex
    with open(args["path"], "rb") as handle:
        content = handle.read()
    filename = args.get("name") or os.path.basename(args["path"])
    head = ("--%s\r\nContent-Disposition: form-data; name=\"file\"; filename=\"%s.hexon\"\r\n"
            "Content-Type: %s\r\n\r\n" % (boundary, filename, BUNDLE_MIME)).encode("utf-8")
    body = head + content + ("\r\n--%s--\r\n" % boundary).encode("utf-8")
    outcome = client.send("POST", "/api/v1/petals/%s/portal-bundles" % args["petal_id"], None, token, timeout, raw=body,
                          content_type="multipart/form-data; boundary=" + boundary)
    outcome["token_source"] = source
    return outcome


def client_outcome_refused(code, source):
    """Same shape as PeerClient.rest's missing-token refusal (imported lazily: client.py needs the whole sim package)."""
    from client import Outcome
    return Outcome(status=0, ok=False, refused=True, code=code, token_source=source)


def _debug(client, token, method, path, body=None):
    return client.send(method, DEBUG_BASE + path, body, token, 8.0)


def observe_value(outcome):
    """The comparable value of one observe reply: mounted flag, label/mode, title, digest, probes, api status."""
    if outcome.ok and isinstance(outcome.get("data"), dict):
        portal = outcome["data"].get("portal") or {}
        has_api = portal.get("has_api")
        return {"mounted": True, "label": portal.get("label"), "mode": portal.get("mode"),
                "title": portal.get("title"), "digest": portal.get("digest"),
                "probes": portal.get("probes") or {},
                # protocol.md section 4: the API script is served only to admitted mounts, so presence of window.fe
                # is the page-side image of /__fe/portal-v1.js answering 200 (absent: 404).
                "api_script_status": 200 if has_api else 404}
    if outcome.get("code") in NOT_MOUNTED_CODES:
        return {"mounted": False, "code": outcome.get("code")}
    return None  # transient or unrelated: the caller counts it as a prober error, never an observation


def ipc_value(outcome):
    """Counters as {counter: int}; `ipc.<counter>` paths read straight from this dict."""
    if outcome.ok and isinstance(outcome.get("data"), dict):
        stats = (outcome["data"].get("portal") or {}).get("stats") or {}
        return {name: stats.get(name) for name in IPC_COUNTERS}
    if outcome.get("code") in NOT_MOUNTED_CODES:
        return {"mounted": False, "code": outcome.get("code")}
    return None


def admission_value(outcome):
    """The local admission read (summary route, host identity only): state and reason, or absent."""
    if outcome.ok and isinstance(outcome.get("data"), dict):
        admission = outcome["data"].get("admission") or {}
        return {"state": admission.get("state"), "reason": admission.get("reason")}
    return {"absent": True, "status": outcome.get("status")}


def collect(client, targets, asset_for_package=None):
    """One probe round for one peer: [(target, value)] for every portal target.

    `targets` are package ids or `*`. The two debug reads run once per round and every target is a view of them:
    only one bundle is mounted at a time (A-7), so `observe`/`ipc` describe whichever is mounted and a target whose
    package is not the mounted one reads as unmounted. `asset_for_package(package_id)` is the SI-supplied binding
    from package id to the uploaded asset id (needed only for the admission read).
    """
    token = client.debug_token()
    observed = observe_value(_debug(client, token, "GET", "observe")) if token else None
    stats = ipc_value(_debug(client, token, "GET", "ipc-stats")) if token else None
    rows = []
    for target in targets:
        value = {"observe": observed, "ipc": stats}
        if target != ALL_TARGET and asset_for_package:
            asset_id = asset_for_package(target)
            if asset_id:
                verse_token, _source = client.verse_token()
                value["admission"] = admission_value(
                    client.send("GET", "/api/v1/portal-bundles/%s" % asset_id, None, verse_token, 8.0))
        if observed is None and stats is None:
            continue
        rows.append((target, value))
    return rows


def digest(value):
    """Stable short digest of a probe value, recorded beside it like every other probe family."""
    return predicates.digest(value)


def recorded(value, limit=2048):
    """Value as stored inline in observations.jsonl."""
    return small(value, limit)


def read_path(value, path):
    """Resolve a probe path in a collected value.

    Observe-derived: `mounted`, `badge` (the host-drawn label), `observe`, `observe.<field>`, `observe.probe.<name>`
    (probe names contain dashes, so the remainder after `probe.` is the name). Counter-derived: `ipc.<counter>` plus
    `writes_accepted`, `ipc.delivered_non_control` (falls back to `deliveries`) and `ipc.dropped_origin_total` (falls
    back to `dropped_origin`). `admission` is the local admission read.
    """
    observed = value.get("observe") if isinstance(value, dict) else None
    stats = value.get("ipc") if isinstance(value, dict) else None
    if path == "mounted":
        return bool(observed and observed.get("mounted"))
    if path == "badge":
        return observed.get("label") if observed and observed.get("mounted") else None
    if path == "writes_accepted":
        return _counter(stats, "writes_accepted", "accepted")
    if path == "ipc.delivered_non_control":
        return _counter(stats, "delivered_non_control", "deliveries")
    if path == "ipc.dropped_origin_total":
        return _counter(stats, "dropped_origin_total", "dropped_origin")
    head, _, rest = path.partition(".")
    if head == "observe" and rest.startswith("probe."):
        return ((observed or {}).get("probes") or {}).get(rest[len("probe."):])
    current = value.get(head) if isinstance(value, dict) else None
    for part in rest.split(".") if rest else []:
        current = current.get(part) if isinstance(current, dict) else None
    return current


def _counter(stats, name, fallback):
    """Named counter, else its plain fallback; None when the mount is absent or neither is reported."""
    if not isinstance(stats, dict):
        return None
    return stats[name] if stats.get(name) is not None else stats.get(fallback)


def encode(value):
    """Canonical JSON for log lines."""
    return json.dumps(value, sort_keys=True, default=str)
