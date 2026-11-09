"""Portal probe family and the upload_bundle driver op (plan T5.3); see tools/qa/portal/AGENTS.md section sim-contract.

Read-only: the probe calls the fixed /api/v1/debug/portal/{observe,ipc-stats} routes, /api/v1/debug/ui-state (the
host-drawn badge) and the admission summary read, never the hierarchy. prober.Prober._portal_round calls `collect` at
cadence `portal_ms`; the driver's portal ops call `upload_bundle`.
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
                # Host-aggregated extras the scenarios read; filled from TOTAL_FALLBACKS when the host omits them.
                "writes_accepted", "delivered_non_control", "dropped_origin_total")
# Extra counter -> the reported counter it equals while one bundle is mounted (A-7); the flat stats are process totals.
TOTAL_FALLBACKS = {"delivered_non_control": "deliveries", "dropped_origin_total": "dropped_origin"}
# Summary `admission.state` -> the BundleState name scenarios assert (admission.md section 2).
ADMISSION_STATES = {"admitted": "Admitted", "static_sandbox": "StaticSandbox", "revoked": "Revoked", "blocked": "Blocked"}
# What `observe` / `ipc` read as while no bundle is mounted (scenarios assert `observe equals "refused"`).
REFUSED = "refused"
# ui-state carries the host-drawn bundle badge (fractalengine/src/qa_debug_bridge.rs `portal` block).
UI_STATE = "/api/v1/debug/ui-state"
# Debug refusals that say a bundle is not mounted: the observation records `mounted: false` and REFUSED reads.
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
    """One observe reply: the mount's facts (label, mode, title, digest, `probe` texts, api script status), REFUSED when
    no bundle is mounted, None when transient or unrelated (the caller never records that as an observation)."""
    if outcome.ok and isinstance(outcome.get("data"), dict):
        portal = outcome["data"].get("portal") or {}
        has_api = portal.get("has_api")
        return {"label": portal.get("label"), "mode": portal.get("mode"), "title": portal.get("title"),
                "digest": portal.get("digest"), "probe": portal.get("probes") or {},
                # protocol.md section 4: the API script is served only to admitted mounts, so presence of window.fe
                # is the page-side image of /__fe/portal-v1.js answering 200 (absent: 404).
                "api_script_status": 200 if has_api else 404}
    if outcome.get("code") in NOT_MOUNTED_CODES:
        return REFUSED
    return None


def ipc_value(outcome):
    """`ipc-stats` counters as {counter: int} (flat values are process totals, ipc-v1 section 7.3), REFUSED unmounted."""
    if outcome.ok and isinstance(outcome.get("data"), dict):
        stats = (outcome["data"].get("portal") or {}).get("stats") or {}
        value = {name: stats.get(name) for name in IPC_COUNTERS}
        for name, fallback in TOTAL_FALLBACKS.items():
            if value.get(name) is None:
                value[name] = stats.get(fallback)
        return value
    if outcome.get("code") in NOT_MOUNTED_CODES:
        return REFUSED
    return None


def badge_value(outcome):
    """(visible badge text, ASCII badge name) of the open bundle portal from ui-state's `portal` block, else (None, None)."""
    data = outcome.get("data") if outcome.ok else None
    portal = data.get("portal") if isinstance(data, dict) else None
    if not isinstance(portal, dict):
        return None, None
    return portal.get("badge_text"), portal.get("badge_name")


def admission_value(outcome):
    """The local admission read (summary route, host identity only) as (BundleState name, reason); (None, None) when the
    summary carries no admission block or the read failed."""
    data = outcome.get("data") if outcome.ok else None
    admission = data.get("admission") if isinstance(data, dict) else None
    if not isinstance(admission, dict) or not admission.get("state"):
        return None, None
    return ADMISSION_STATES.get(admission["state"], admission["state"]), admission.get("reason")


def writes_accepted(observed, stats):
    """Accepted set_property writes: the host counter when reported, else the page's own `writes-ok` probe (never the
    all-message `accepted` counter, which also counts hello and subscribe)."""
    if isinstance(stats, dict) and stats.get("writes_accepted") is not None:
        return stats["writes_accepted"]
    text = (observed.get("probe") or {}).get("writes-ok") if isinstance(observed, dict) else None
    return int(text) if isinstance(text, str) and text.isdigit() else None


def collect(client, targets, asset_for_package=None):
    """One probe round for one peer: [(target, value)] for every portal target.

    `targets` are package ids or `*`. The debug reads run once per round and every target is a view of them: only one
    bundle is mounted at a time (A-7). The value is shaped so every scenario path resolves with the generic dotted
    lookup (`predicates.at_path`): `mounted`, `badge`, `badge_name`, `observe` (or "refused"), `observe.title`,
    `observe.probe.<name>`, `observe.api_script_status`, `ipc.<counter>`, `writes_accepted`, `admission`.
    `asset_for_package(package_id)` is the driver's package-to-uploaded-asset binding (needed only for `admission`).
    """
    token = client.debug_token()
    observed = observe_value(_debug(client, token, "GET", "observe")) if token else None
    stats = ipc_value(_debug(client, token, "GET", "ipc-stats")) if token else None
    badge, badge_name = badge_value(client.send("GET", UI_STATE, None, token, 8.0)) if token else (None, None)
    rows = []
    for target in targets:
        value = {"mounted": isinstance(observed, dict), "observe": observed, "ipc": stats, "badge": badge,
                 "badge_name": badge_name, "writes_accepted": writes_accepted(observed, stats)}
        if target != ALL_TARGET and asset_for_package:
            asset_id = asset_for_package(target)
            if asset_id:
                verse_token, _source = client.verse_token()
                state, reason = admission_value(
                    client.send("GET", "/api/v1/portal-bundles/%s" % asset_id, None, verse_token, 8.0))
                value["admission"], value["admission_reason"] = state, reason
        if observed is None and stats is None and badge is None and value.get("admission") is None:
            continue  # nothing judgeable this round: transient, not an observation
        rows.append((target, value))
    return rows


def digest(value):
    """Stable short digest of a probe value, recorded beside it like every other probe family."""
    return predicates.digest(value)


def recorded(value, limit=2048):
    """Value as stored inline in observations.jsonl."""
    return small(value, limit)


def encode(value):
    """Canonical JSON for log lines."""
    return json.dumps(value, sort_keys=True, default=str)
