"""Scenario driver: one process, concurrent peer lanes, threading.Barrier same-instant fires, every action logged; see tools/qa/AGENTS.md §sim."""
import base64
import hashlib
import json
import math
import os
import random
import re
import subprocess
import threading
import time
import urllib.parse
from concurrent.futures import ThreadPoolExecutor

import portal_probe
from gpx_upload import upload_gpx
import predicates
from client import Outcome, extract_id
import fe  # noqa: E402 - client.py put tools/qa on sys.path
from runtime import small
from scenario import UnresolvedRef, source_probe, target_key

BARRIER_LEAD_S = 0.5
BARRIER_TIMEOUT_S = 120.0
NEEDS_WAIT_S = 90.0  # longest a dependent waits for a same-step dependency still in flight before it counts as unmet
LIVE_POLL_S = 0.25
FINGERPRINT_NODE_CAP = 400
# Extension-platform REST ops (scenario.VIAS["rest"]); each dispatches to Driver._ext_<op>. Administration is not here:
# install, consent, bind, enable, update and uninstall are the "ext" via (EXT_OPS), GUI or local channel, never REST.
EXTENSION_OPS = {"set_extension_policy", "extension_command", "ingress_post", "mqtt_publish", "publish_revocation", "set_binding"}
EXT_OPS = {"ext_install", "ext_consent", "ext_accept", "ext_bind", "ext_enable", "ext_disable", "ext_update", "ext_uninstall",
           "ext_kill_override", "ext_list"}
REPO_ROOT = os.path.abspath(os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "..", ".."))
REF_NAME = re.compile(r"\$\{?([A-Za-z_][A-Za-z0-9_]*)")


def digest_of(value):
    return hashlib.sha256(json.dumps(value, sort_keys=True, default=str).encode("utf-8")).hexdigest()[:16]


PORTAL_OPS = {"upload_bundle", "create_repository_node", "portal_update_bundle", "portal_slot", "portal_observe",
              "portal_ipc_stats", "portal_admit", "portal_killswitch", "portal_ipc_post", "regrant_role"}


def new_ulid(rng=None):
    """Crockford-base32 ULID (time + random); seeded rng keeps free-play ids replayable."""
    alphabet = "0123456789ABCDEFGHJKMNPQRSTVWXYZ"
    value = (int(time.time() * 1000) << 80) | (rng or random.SystemRandom()).getrandbits(80)
    return "".join(alphabet[(value >> (5 * i)) & 31] for i in reversed(range(26)))


def surreal_string(text):
    return "'" + str(text).replace("\\", "\\\\").replace("'", "\\'") + "'"


def verdict_of(expect, outcome):
    """as_expected | accepted_but_expected_refusal | refused_but_expected_ok | peer_down | timed_out."""
    if outcome.get("code") == "not_running":
        return "timed_out" if outcome.get("timed_out") else "peer_down"
    if expect == "any" or (expect == "ok") == outcome.ok:
        return "as_expected"
    return "accepted_but_expected_refusal" if outcome.ok else "refused_but_expected_ok"


def outcome_extras(outcome):
    """Optional event fields an outcome may carry: the GUI widget wait, the wheel scrolls that revealed a clipped widget, why a refused widget was unreachable, the view a landed click left behind, a client-side timeout."""
    extras = {}
    if outcome.get("waited_ms") is not None:
        extras["waited_ms"] = outcome["waited_ms"]
    if outcome.get("scrolled"):
        extras["scrolled"] = outcome["scrolled"]
    if outcome.get("clipped"):
        extras["clipped"] = outcome["clipped"]
    if outcome.get("view_changed"):
        extras["view_changed"] = outcome["view_changed"]
    if outcome.get("timed_out"):
        extras["timed_out"] = True
    return extras


class FireGate:
    """Same-instant gate whose membership is decided at fire time (tools/qa/AGENTS.md §sim, Wait-and-anchor rules).

    A participant either arrives (and waits for the others) or withdraws because its `needs` or refs turned out
    unmet, so a withdrawal neither stalls nor breaks the rest. The last arrival fixes fire_at = now + lead
    (no lead when it is the only one left).
    """

    def __init__(self, parties, clock, lead_s=BARRIER_LEAD_S):
        self._clock = clock
        self._lead_s = lead_s
        self._parties = parties
        self._arrived = 0
        self._fire_at = None
        self._broken = False
        self._condition = threading.Condition()

    def _open_if_complete(self):  # caller holds the lock
        if self._fire_at is None and self._parties > 0 and self._arrived >= self._parties:
            self._fire_at = self._clock.now() + (self._lead_s if self._parties > 1 else 0.0)
            self._condition.notify_all()

    def arrive(self, timeout):
        """Block until every remaining participant has arrived; the shared fire instant (BrokenBarrierError on abort or timeout)."""
        with self._condition:
            self._arrived += 1
            self._open_if_complete()
            self._condition.wait_for(lambda: self._fire_at is not None or self._broken, timeout)
            if self._fire_at is None:
                self._broken = True
                self._condition.notify_all()
                raise threading.BrokenBarrierError()
            return self._fire_at

    def withdraw(self):
        """Leave before arriving; the gate opens if everyone still in has already arrived."""
        with self._condition:
            self._parties -= 1
            self._open_if_complete()

    def abort(self):
        with self._condition:
            self._broken = True
            self._condition.notify_all()


def select_steps(steps, spec):
    """'0-3', '0,2,5', '13a' -> the chosen step dicts in scenario order (None = all)."""
    if not spec:
        return list(steps)
    ids = [str(s["id"]) for s in steps]
    chosen = set()
    for item in str(spec).split(","):
        item = item.strip()
        if "-" in item and item not in ids:
            first, last = item.split("-", 1)
            if first not in ids or last not in ids:
                raise ValueError("step range %r names unknown ids" % item)
            chosen.update(ids[ids.index(first): ids.index(last) + 1])
        elif item in ids:
            chosen.add(item)
        else:
            raise ValueError("unknown step id %r" % item)
    # A chosen step's '$ref'/needs on an action of an unchosen earlier step pulls that step in (transitively), so
    # '--steps 5' runs the steps that create its targets instead of firing at unresolved refs.
    producer = {}
    for step in steps:
        for action in step.get("prepare", []) + step.get("actions", []):
            if action.get("id"):
                producer.setdefault(str(action["id"]), str(step["id"]))
    pending = list(chosen)
    while pending:
        wanted = pending.pop()
        step = next(s for s in steps if str(s["id"]) == wanted)
        text = json.dumps([step.get("prepare", []), step.get("actions", []), step.get("expect", [])])
        names = set(REF_NAME.findall(text))
        for action in step.get("prepare", []) + step.get("actions", []) + step.get("expect", []):
            names.update(dep for dep in action.get("needs", []) if isinstance(dep, str))
        for name in names:
            source = producer.get(name)
            if source and source not in chosen and ids.index(source) < ids.index(str(step["id"])):
                chosen.add(source)
                pending.append(source)
    return [s for s in steps if str(s["id"]) in chosen]


class Driver:
    """Runs scenario steps against live peers and records events/expectations/steps JSONL."""

    def __init__(self, scenario, clients, ctx, clock, recorder, store, cadence, run_dir, host=None,
                 steps=None, log=print, loopback=None, stage_dir=None):
        self.stage_dir = stage_dir  # the staged build directory: fe-ext.exe sits beside fractalengine.exe
        self.repo_root = REPO_ROOT
        self.loopback = loopback  # loopback.Loopback (MQTT broker, signed revocation list) when run.py started one
        self.installs = {}  # installation id -> package id, from ext_consent results
        self.device_nodes = {}  # (petal id, device id) -> node id, for ingress_post / ingest_readings by device
        self.scenario = scenario
        self.clients = clients
        self.ctx = ctx
        self.clock = clock
        self.recorder = recorder
        self.store = store
        self.cadence = cadence
        self.run_dir = run_dir
        self.host = host
        self.steps = steps if steps is not None else scenario["steps"]
        self.log = log
        self.present = set(clients)
        self.abort = threading.Event()
        self._settled = {}  # this step's action id -> Event set once that action has finished (any status), for fire-time `needs`

    # -- step loop -----------------------------------------------------------------------------
    def run(self):
        for index, step in enumerate(self.steps):
            if self.abort.is_set():
                break
            self.cadence.set_step(index)
            self.run_step(step)
        self.fingerprint("final")

    def run_step(self, step):
        sid = str(step["id"])
        self.cadence.set_override(step.get("probe"))
        t_start = self.clock.now()
        self.recorder.write("steps", {"step": sid, "event": "start", "t": t_start, "title": step["title"],
                                      "cadence": self.cadence.as_dict()})
        self.log("[step %s] %s" % (sid, step["title"]))
        self._settled = {action["id"]: threading.Event() for group in ("prepare", "actions")
                         for action in step.get(group, []) if action.get("id")}
        prepared = [self.execute(action, sid, "prepare", t_start) for action in step.get("prepare", [])]
        t_actions = self.clock.now()
        events = self.run_actions(step, sid, t_actions)
        self.recover_gui(sid, prepared + events)
        t_done = self.clock.now()
        instances = self.register_expectations(step, sid, events, t_actions)
        settled_at, unmet = self.wait_settle(step, instances, t_done)
        for checkpoint in step.get("checkpoints", []):
            self.checkpoint(checkpoint, sid)
        self.cadence.set_override(None)
        t_end = self.clock.now()
        self.recorder.write("steps", {"step": sid, "event": "end", "t": t_end, "t_start": t_start,
                                      "t_actions": t_actions, "t_actions_done": t_done, "settled_at": settled_at,
                                      "settle_s": step["settle_s"], "unmet_live": unmet})
        self.log("[step %s] done in %.1fs (%s)" % (sid, t_end - t_start,
                                                   "settled" if not unmet else "unmet live: " + ", ".join(unmet)))

    def run_actions(self, step, sid, t_step):
        actions = step.get("actions", [])
        seq = [a for a in actions if a.get("sync", "seq") == "seq"]
        barrier = [a for a in actions if a.get("sync") == "barrier"]
        offset = [a for a in actions if str(a.get("sync", "")).startswith("+")]
        events = []
        events_lock = threading.Lock()

        def keep(event):
            with events_lock:
                events.append(event)

        threads = []
        # Static facts only: an absent peer or absent required peer never joins. `needs` is decided at fire time, inside the gate.
        live_barrier = [a for a in barrier if a["peer"] in self.present
                        and all(peer in self.present for peer in a.get("requires", []))]
        for action in barrier:
            if action not in live_barrier:  # skipped as a unit, never a half-filled barrier
                keep(self.execute(action, sid, "barrier", t_step))
        if live_barrier:
            gate = FireGate(len(live_barrier), self.clock)

            def fire_in_gate(participant):
                try:
                    keep(self.execute(participant, sid, "barrier", t_step, gate=gate))
                except BaseException:  # a harness defect in one participant must not hold the others at the gate
                    gate.withdraw()
                    raise

            for action in live_barrier:  # a participant whose needs/refs fail withdraws; the others still fire together
                threads.append(threading.Thread(target=lambda a=action: fire_in_gate(a), daemon=True))
        for action in offset:
            delay = float(action["sync"][1:])
            threads.append(threading.Thread(target=lambda a=action, d=delay: keep(self.execute(
                a, sid, "offset", t_step, at=t_step + d)), daemon=True))
        threads.append(threading.Thread(target=lambda: [keep(self.execute(a, sid, "seq", t_step)) for a in seq],
                                        daemon=True))
        for thread in threads:
            thread.start()
        for thread in threads:
            thread.join()
        return events

    def unmet_needs(self, item):
        """Ids in item["needs"] whose action did not run as expected (it failed, was skipped or has not run).

        "As expected" is the action's own `expect`: ok for expect ok, a refusal for expect refused, any answer for
        expect any, so a race write that editor2's app refuses still counts as a race outcome and the expectations
        that need it are exercised (QA-0163 d). Bindings without `met` (older records, test stubs) fall back to `ok`.
        """
        unmet = []
        for dep in item.get("needs", []):
            met = self.ctx.try_resolve("$%s.met" % dep)
            if met is None:
                met = self.ctx.try_resolve("$%s.ok" % dep)
            if met is not True:
                unmet.append(dep)
        return unmet

    def settle_needs(self, item):
        """(unmet ids, seconds waited): `needs` judged when the dependent fires, after same-step dependencies still in flight have finished.

        A revoked-writer action due at +25 s therefore runs once the revoke GUI sequence has actually completed (or is
        skipped when it failed), however long that sequence took; a dependency that never finishes is unmet after NEEDS_WAIT_S.
        """
        started = time.monotonic()
        deadline = started + NEEDS_WAIT_S
        for dep in item.get("needs", []):
            finished = self._settled.get(dep)
            while finished is not None and not finished.is_set() and not self.abort.is_set():
                remaining = deadline - time.monotonic()
                if remaining <= 0:
                    break
                finished.wait(min(remaining, LIVE_POLL_S))
        return self.unmet_needs(item), time.monotonic() - started

    def recover_gui(self, sid, events):
        """Press Escape on each peer whose GUI sequence ended in a failure, so a menu or dialog it left open cannot cover the next step.

        Only the peer's LAST GUI action counts: after a later success (a failed optional "Close window", then
        "Open verse") nothing is left open, and an Escape there would navigate the peer back out of the verse.
        """
        last_gui = {}
        for event in sorted(events, key=lambda event: event.get("t_fire") or 0.0):
            if event.get("via") == "gui" and event.get("status") == "done" and event["peer"] in self.present:
                last_gui[event["peer"]] = event
        failed = [peer for peer, event in last_gui.items()
                  if event.get("ok") is False
                  and str(event.get("verdict") or "").removeprefix("optional_") == "refused_but_expected_ok"]
        for peer in failed:
            base = {"step": sid, "lane": "recovery", "peer": peer, "action_id": "%s.recover.%s" % (sid, peer),
                    "via": "gui", "op": "escape", "expect": "any", "sync": "recovery", "optional": True}
            t_fire = self.clock.now()
            try:
                outcome = self.clients[peer].gui("key", {"combo": "Escape"})
            except Exception as error:  # noqa: BLE001 - a harness defect is recorded, the run continues
                outcome = Outcome(status=-2, ok=False, refused=False, code="harness_error", error=str(error)[:300])
            self._event(base, status="done", t_fire=t_fire, t_ack=self.clock.now(), ok=outcome.ok, code=outcome.get("code"),
                        error=outcome.get("error"), verdict="as_expected", **outcome_extras(outcome))

    # -- one action ----------------------------------------------------------------------------
    def execute(self, action, sid, lane, t_step, gate=None, at=None, extra=None):
        """Run one action and log its event; whatever the outcome, dependents waiting on its id are released."""
        try:
            return self._execute(action, sid, lane, t_step, gate, at, extra)
        finally:
            finished = self._settled.get(action.get("id"))
            if finished is not None:
                finished.set()

    def _execute(self, action, sid, lane, t_step, gate, at, extra):
        action_id = action.get("id") or "%s.%s.%s" % (sid, action["peer"], action["op"])
        base = {"step": sid, "lane": lane, "peer": action["peer"], "action_id": action_id, "via": action["via"],
                "op": action["op"], "expect": action.get("expect", "ok"), "sync": action.get("sync", "seq"),
                "optional": bool(action.get("optional"))}
        if action["peer"] not in self.present:
            return self._event(base, status="skipped_peer_absent")
        absent = [peer for peer in action.get("requires", []) if peer not in self.present]
        if absent:  # e.g. the owner's half of an editor2 revoke: skipped as a unit, never a half-run GUI sequence
            return self._event(base, status="skipped_peer_absent", requires_absent=absent)
        client = self.clients[action["peer"]]
        if gate is None and at is not None:  # offset lane: sleep to the scheduled instant first, THEN judge needs
            self.clock.sleep_until(at)
            base["fire_at"] = at
        unmet, waited = self.settle_needs(action)  # at fire time, so a dependency still running is awaited, not assumed failed
        if waited >= 0.05:
            base["needs_waited_ms"] = round(waited * 1000)
        if unmet:  # e.g. a write expected to be refused after a revoke whose click never landed: not exercised
            if gate is not None:
                gate.withdraw()
            return self._event(base, status="skipped_dependency_failed", needs_failed=unmet)
        try:
            args = self.ctx.resolve(action.get("args", {}), extra)
        except UnresolvedRef as missing:
            if gate is not None:
                gate.withdraw()
            return self._event(base, status="skipped_unresolved" if action.get("optional") else "error_unresolved",
                               error="unresolved $%s" % missing.ref, unresolved_ref=missing.ref,
                               unresolved_reason=missing.reason)
        base["args_digest"] = digest_of(args)
        base["args_small"] = small(_scrub(args), 2048)
        if gate is not None:
            if action["via"] == "gui":
                try:
                    client.prime_gui()
                except Exception as error:  # noqa: BLE001 - leave the gate so the other participants still fire together
                    gate.withdraw()
                    return self._event(base, status="error_barrier_broken", error="prime_gui: %s" % str(error)[:200])
            try:
                fire_at = gate.arrive(BARRIER_TIMEOUT_S)
            except threading.BrokenBarrierError:
                return self._event(base, status="error_barrier_broken")
            self.clock.sleep_until(fire_at)
            base["fire_at"] = fire_at
        t_fire = self.clock.now()
        try:
            outcome = self.dispatch(client, action, args, sid, action_id)
        except Exception as error:  # noqa: BLE001 - a harness defect is recorded, the run continues
            outcome = Outcome(status=-2, ok=False, refused=False, code="harness_error",
                              error="%s: %s" % (type(error).__name__, str(error)[:300]))
        t_ack = self.clock.now()
        verdict = verdict_of(action.get("expect", "ok"), outcome)
        if action.get("optional") and verdict != "as_expected":
            verdict = "optional_" + verdict
        record = {"id": extract_id(outcome.get("data")), "ok": outcome.ok, "met": verdict == "as_expected",
                  "status": outcome.get("status"), "data": outcome.get("data"),
                  "failure": None if outcome.ok else "%s: %s" % (outcome.get("code"), str(outcome.get("error") or "")[:120])}
        if isinstance(outcome.get("data"), dict):
            for key, value in outcome["data"].items():  # $sp.fractal_id, $up.content_hash without explicit bind
                record.setdefault(key, value)
        for name, path in (action.get("bind") or {}).items():
            record[name] = predicates.at_path(record, path)[1]
        if action.get("id"):
            self.ctx.bind(action["id"], record)
        return self._event(base, status="done", t_fire=t_fire, t_ack=t_ack, ok=outcome.ok,
                           http_status=outcome.get("status"), code=outcome.get("code"),
                           error=outcome.get("error"), verdict=verdict, token_source=outcome.get("token_source"),
                           result_id=record["id"], result_digest=digest_of(outcome.get("data")),
                           result_small=small(_scrub(outcome.get("data")), 2048),
                           fire_late_ms=round((t_fire - base["fire_at"]) * 1000, 3) if "fire_at" in base else None,
                           **outcome_extras(outcome))

    def _event(self, base, **fields):
        event = dict(base)
        event.update(fields)
        for key in ("t_fire", "t_ack"):
            if event.get(key) is not None:
                event["wall_" + key[2:]] = round(self.clock.wall(event[key]), 4)
        if "t_fire" not in event:
            event["t_fire"] = event["t_ack"] = self.clock.now()
        self.recorder.write("events", event)
        if event.get("verdict") not in (None, "as_expected") or str(event.get("status", "")).startswith("error"):
            self.log("  ! %s %s/%s %s %s %s" % (event["action_id"], event["peer"], event["op"], event.get("status"),
                                                event.get("verdict"), (event.get("error") or "")[:160]))
        return event

    def dispatch(self, client, action, args, sid, action_id):
        via, op = action["via"], action["op"]
        if via == "rest":
            return self.rest(client, op, args)
        if via == "mcp":
            return client.mcp(op, args)
        if via == "gui":
            return client.gui(op, args)
        if via == "ext":
            return self.ext_op(client, op, args)
        if via == "host":
            return self.host_op(client, op, args)
        if via == "macro":
            return self.macro(client, op, args, sid, action_id, action)
        raise ValueError("unknown via " + via)

    def rest(self, client, op, args):
        verse = args.get("verse_id") or self.ctx.try_resolve("$fixture.verse_id")
        if op in EXTENSION_OPS:
            return getattr(self, "_ext_" + op)(client, args, verse)
        if op == "ingest_readings" and "device" in args and "readings" not in args:
            return self._ingest_device(client, args)
        if op == "request":
            return client.rest(args["method"].upper(), args["path"], args.get("body"))
        if op == "create_fractal":
            return client.rest("POST", "/api/v1/verses/%s/fractals" % verse, {"name": args["name"]})
        if op == "create_petal":
            return client.rest("POST", "/api/v1/verses/%s/fractals/%s/petals" % (verse, args["fractal_id"]),
                               {"name": args["name"]})
        if op == "create_node":
            body = {"name": args["name"], "position": args.get("position", [0, 0, 0])}
            return client.rest("POST", "/api/v1/verses/%s/fractals/%s/petals/%s/nodes" % (
                verse, args["fractal_id"], args["petal_id"]), body)
        if op == "set_transform":
            body = {k: args[k] for k in ("position", "rotation", "scale")}
            return client.rest("PATCH", "/api/v1/nodes/%s/transform" % args["node_id"], body)
        if op == "set_property":
            return client.rest("PATCH", "/api/v1/nodes/%s/properties" % args["node_id"],
                               {"key": args["key"], "value": args["value"]})
        if op == "delete_property":
            return client.rest("DELETE", "/api/v1/nodes/%s/properties/%s" % (
                args["node_id"], urllib.parse.quote(args["key"], safe="")))
        if op == "delete_node":
            return client.rest("DELETE", "/api/v1/nodes/%s" % args["node_id"])
        if op == "upsert_group":
            group_id = args.get("group_id") or new_ulid()
            body = {k: args[k] for k in ("name", "visible", "order") if k in args}
            out = client.rest("PATCH", "/api/v1/petals/%s/visibility-groups/%s" % (args["petal_id"], group_id), body)
            out["data"] = {"id": group_id, "registry": out.get("data")}
            return out
        if op == "delete_group":
            return client.rest("DELETE", "/api/v1/petals/%s/visibility-groups/%s" % (args["petal_id"], args["group_id"]))
        if op == "upload_asset":
            return client.upload_multipart(args["petal_id"], args["path"], args.get("name"))
        if op == "upload_gpx":
            return upload_gpx(client, args["petal_id"], args["path"], args.get("name"))
        if op in PORTAL_OPS:
            return self.portal_op(client, op, args, verse)
        if op == "query":
            return client.rest("POST", "/api/v1/query", {"sql": args["sql"]})
        if op == "ingest_readings":
            return client.rest("POST", "/api/v1/petals/%s/iot/readings" % args["petal_id"], {"readings": args["readings"]})
        if op == "lookup_node":
            sql = ("SELECT node_id FROM node WHERE petal_id = %s AND display_name = %s AND tombstone = NONE"
                   % (surreal_string(args["petal_id"]), surreal_string(args["name"])))
            return self._poll_lookup(client, lambda: client.rest("POST", "/api/v1/query", {"sql": sql}),
                                     lambda rows: rows[0].get("node_id") if isinstance(rows, list) and rows else None,
                                     float(args.get("wait_s", 10)))
        if op == "lookup_petal":
            # One hierarchy read per attempt (0.5 Hz): QA-0125 says hierarchy GETs rebuild the GUI tree.
            def find(data):
                for verse_row in data if isinstance(data, list) else []:
                    for fractal in verse_row.get("fractals", []):
                        if args.get("fractal_id") and fractal.get("id") != args["fractal_id"]:
                            continue
                        for petal in fractal.get("petals", []):
                            if petal.get("name") == args["name"]:
                                return {"id": petal.get("id"), "fractal_id": fractal.get("id")}
                return None
            return self._poll_lookup(client, lambda: client.rest("GET", "/api/v1/hierarchy"), find,
                                     float(args.get("wait_s", 10)), interval=2.0)
        if op == "lookup_group":
            def find_group(data):
                groups = data.get("groups") if isinstance(data, dict) else None
                match = [g for g in groups or [] if isinstance(g, dict) and g.get("name") == args["name"]]
                return match[0].get("id") if match else None
            return self._poll_lookup(client, lambda: client.rest(
                "GET", "/api/v1/petals/%s/visibility-groups" % args["petal_id"]), find_group, float(args.get("wait_s", 10)))
        raise ValueError("unknown rest op " + op)

    # -- extension platform ops (E19, BND-11): thin wrappers over the REST doors, bindings.md section 9 --------
    @staticmethod
    def _refused(code, error, status=0):
        return Outcome(status=status, ok=False, refused=True, code=code, error=error)

    def _installation_of(self, args):
        return args.get("installation") or ""

    def _package_of(self, args):
        return args.get("package_id") or self.installs.get(self._installation_of(args)) or "org.example.%s" % args.get("package", "")

    # -- extension administration through the app's own surfaces (via "ext"; tools/qa/sim/AGENTS.md section ext) ----
    # The Extensions panel is driven by accessible name (docs/extensions/extensions-ui.md) and `fe-ext admin` speaks the
    # local admin channel (docs/extensions/admin-channel.md). Neither path answers consent for the operator: the driver
    # clicks the consent dialog's own button, so every grant is still a GUI click. Nothing here calls a REST admin route.
    EXT_ERROR_LINE = re.compile(r"^(host-off|package-invalid|untrusted-signer|revoked|entitlement-required|policy-denied|"
                                r"role-denied|consent-missing|not-available|not-found|invalid-input)\b.*$")
    EXT_CLI_ENV = "FE_EXT_ADMIN_INSTANCE"

    def ext_op(self, client, op, args):
        return getattr(self, "_x_" + op[len("ext_"):])(client, args)

    def _x_package(self, args):
        """(example name, package id, version, path) for an op's `package` (an example name) or `package_id`."""
        staged = (self.loopback.packages if self.loopback is not None else {}) or {}
        name = args.get("package")
        entry = staged.get(name) if name else next((e for e in staged.values() if e.get("package_id") == args.get("package_id")), None)
        entry = entry or {}
        package_id = args.get("package_id") or entry.get("package_id") or ("org.example.%s" % name if name else "")
        return name, package_id, args.get("version") or entry.get("version"), entry.get("path")

    def _x_nodes(self, client, fragment):
        """Visible ui-tree nodes whose accessible name contains `fragment`: [(name, enabled)]; [] when the app is unreachable."""
        try:
            tree = fe.probe(client.run(), "ui-tree", None, {"name": fragment})
            return [(n["name"], bool(n.get("enabled", True))) for n in fe.tree_nodes(tree) if not n.get("hidden")]
        except (fe.Refused, fe.NotRunning):
            return []

    def _x_has(self, client, name, enabled=None):
        """True when a control named exactly `name` is visible (and enabled, when `enabled` is True)."""
        return any(n.casefold() == name.casefold() and (enabled is not True or on) for n, on in self._x_nodes(client, name))

    def _x_click(self, client, name, wait_s=8.0, role=None):
        args = {"name": name, "exact": True, "wait_s": wait_s}
        if role:
            args["role"] = role
        return client.gui("click", args)

    def _x_wait_any(self, client, names, wait_s):
        """First of `names` that appears within wait_s (exact name, visible), else None."""
        deadline = time.monotonic() + wait_s
        while True:
            for name in names:
                if self._x_has(client, name):
                    return name
            if time.monotonic() >= deadline or self.abort.is_set():
                return None
            time.sleep(0.4)

    def _x_open_panel(self, client, view="Installed extensions"):
        """Open the Extensions panel (the toolbar toggle) and select a view tab; refuses with the failing click's outcome."""
        if not self._x_wait_any(client, ["Installed extensions", "Browse extensions"], 0.6):
            opened = self._x_click(client, "Toggle Extensions panel", 6.0)
            if not opened.ok:
                return opened
            if not self._x_wait_any(client, ["Installed extensions"], 6.0):
                return self._refused("ext_panel_missing", "the Extensions panel did not open")
        return self._x_click(client, view, 6.0)

    def _x_error(self, client):
        """The bridge's refusal line (kebab reason, verbatim) when the panel shows its error row, then dismiss it."""
        if not self._x_has(client, "Dismiss extension error"):
            return None
        line = None
        try:
            tree = fe.probe(client.run(), "ui-tree", None)
            names = [n["name"] for n in fe.tree_nodes(tree) if not n.get("hidden")]
            line = next((n for n in names if self.EXT_ERROR_LINE.match(n)), None)
        except (fe.Refused, fe.NotRunning):
            pass
        self._x_click(client, "Dismiss extension error", 2.0)
        return line or "unreadable"

    def _x_installations(self, client):
        """GET /api/v1/extensions (a read): [{installation, package_id, state}]; empty when the list is not readable."""
        out = client.rest("GET", "/api/v1/extensions")
        data = out.get("data") if out.ok else None
        rows = data.get("extensions") if isinstance(data, dict) else data
        return [{"installation": r.get("installation") or r.get("id"), "package_id": r.get("package_id"), "state": r.get("state")}
                for r in rows or [] if isinstance(r, dict)]

    def _x_find_installation(self, client, package_id, wait_s):
        deadline = time.monotonic() + wait_s
        while True:
            rows = [r for r in self._x_installations(client) if r["package_id"] == package_id and r["installation"]]
            if rows:
                return rows[-1]
            if time.monotonic() >= deadline or self.abort.is_set():
                return None
            time.sleep(0.5)

    def _x_stage_install(self, client, args):
        """Panel route of an install up to the consent dialog: Browse row when the package is indexed, else the file field."""
        name, package_id, version, path = self._x_package(args)
        if args.get("via", "browse") == "file" or not version:
            if not path:
                return None, self._refused("no_package_file", "no staged .hexon for %s (run.py stages the examples)" % (name or package_id))
            opened = self._x_open_panel(client, "Installed extensions")
            if not opened.ok:
                return None, opened
            typed = client.gui("type", {"into": "Package file path", "text": path, "exact": True})
            if not typed.ok:
                return None, typed
            return package_id, self._x_click(client, "Install from file", 6.0)
        opened = self._x_open_panel(client, "Browse extensions")
        if not opened.ok:
            return None, opened
        self._x_click(client, "Refresh extension list", 3.0)
        return package_id, self._x_click(client, "Install %s %s" % (package_id, version), float(args.get("wait_s", 20)))

    def _x_install_outcome(self, client, package_id, wait_s, keep_dialog=False):
        """Classify what the panel shows after an install press: consent needed (ok), kill-listed or refused (refused)."""
        enable, override = "Enable %s" % package_id, "Override kill list for %s" % package_id
        seen = self._x_wait_any(client, [enable, override, "Dismiss extension error"], wait_s)
        if seen == enable:
            return Outcome(status=200, ok=True, refused=False, data={"package_id": package_id, "state": "needs_consent"})
        if seen == override:
            if not keep_dialog:
                self._x_click(client, "Cancel extension consent", 3.0)
            return Outcome(status=200, ok=False, refused=True, code="revoked", error="the build is kill-listed; only the verse Owner override lifts it",
                           data={"package_id": package_id, "state": "kill_listed"})
        if seen:
            reason = self._x_error(client)
            return Outcome(status=200, ok=False, refused=True, code=reason, error="install refused: %s" % reason, data={"package_id": package_id})
        return self._refused("ext_no_response", "neither the consent dialog nor an error row appeared for %s" % package_id)

    def _x_install(self, client, args):
        """ext_install: stage the package and stop at the consent dialog; ext_consent answers it. A refusal is the panel's own line."""
        package_id, pressed = self._x_stage_install(client, args)
        if not package_id:
            return pressed
        if not pressed.ok:
            reason = self._x_error(client)
            return self._refused(reason or pressed.get("code") or "install_not_offered", "install not offered: %s" % (reason or pressed.get("error")))
        return self._x_install_outcome(client, package_id, float(args.get("outcome_wait_s", 25)))

    def _x_kill_override(self, client, args):
        """ext_kill_override: the verse Owner's audited override, in the consent dialog (typed package id); consent is still owed."""
        package_id, pressed = self._x_stage_install(client, args)
        if not package_id:
            return pressed
        if not pressed.ok:
            return pressed
        seen = self._x_install_outcome(client, package_id, float(args.get("outcome_wait_s", 25)), keep_dialog=True)
        if seen.get("code") != "revoked":
            return seen if not seen.ok else self._refused("not_kill_listed", "%s was not kill-listed, nothing to override" % package_id)
        typed = client.gui("type", {"into": "Confirm package id to override kill list", "text": package_id, "exact": True})
        if not typed.ok:
            return typed
        clicked = self._x_click(client, "Override kill list for %s" % package_id, 6.0)
        if not clicked.ok:
            return clicked
        if not self._x_wait_any(client, ["Enable %s" % package_id], 15.0):  # a fresh ConsentNeeded for the same installation
            return self._refused("override_not_applied", "no consent dialog followed the override of %s" % package_id)
        return Outcome(status=200, ok=True, refused=False, data={"package_id": package_id, "state": "needs_consent", "overridden": True})

    def _x_consent(self, client, args):
        """ext_consent: the consent dialog's own button ("Enable <id>", or "Update <id>" on a re-prompt), then read the installation id."""
        _name, package_id, _version, _path = self._x_package(args)
        label = "Update %s" % package_id if args.get("reprompt") else "Enable %s" % package_id
        clicked = self._x_click(client, label, float(args.get("wait_s", 15)))
        if not clicked.ok:
            return clicked
        found = self._x_find_installation(client, package_id, float(args.get("installation_wait_s", 20)))
        if not found:
            return self._refused("installation_not_listed", "no installation of %s appeared after consent" % package_id)
        self.installs[found["installation"]] = package_id
        return Outcome(status=200, ok=True, refused=False,
                       data={"installation": found["installation"], "package_id": package_id, "state": found["state"]})

    def _x_accept(self, client, args):
        """ext_accept: the Bindings view's one offer ("Allow in this verse"); its consent dialog is answered when it opens."""
        hook, key = args.get("hook", "petal.map"), args.get("key", "")
        opened = self._x_open_panel(client, "Extension bindings")
        if not opened.ok:
            return opened
        point = "binding:%s:%s:action" % (hook, key) if key else "binding:%s:action" % hook
        clicked = self._x_click(client, point, float(args.get("wait_s", 10)))
        if not clicked.ok:
            return clicked
        package_id = self._x_package(args)[1] or self.installs.get(args.get("installation", ""), "")
        if package_id and self._x_wait_any(client, ["Enable %s" % package_id], 4.0):
            clicked = self._x_click(client, "Enable %s" % package_id, 6.0)
        return clicked

    def _x_binding_buttons(self, client, package_id, verb):
        """Names of the "<verb> <binding label> for <id>" toggles of an open Overview."""
        return [n for n, on in self._x_nodes(client, " for %s" % package_id) if n.startswith(verb + " ") and on]

    def _x_toggle(self, client, args, verb):
        """GUI route of enable/disable: Open the row, press every (or the named) binding toggle."""
        _name, package_id, _version, _path = self._x_package({**args, "package_id": args.get("package_id") or self.installs.get(args.get("installation", ""))})
        opened = self._x_open_panel(client, "Installed extensions")
        if not opened.ok:
            return opened
        opened = self._x_click(client, "Open %s" % package_id, 8.0)
        if not opened.ok:
            return opened
        names = self._x_binding_buttons(client, package_id, verb)
        if args.get("binding"):
            names = [n for n in names if n == "%s %s for %s" % (verb, args["binding"], package_id)]
        if not names:
            return self._refused("no_binding_toggle", "no %s control for %s" % (verb, package_id))
        out = None
        for name in names:
            out = self._x_click(client, name, 6.0)
            if not out.ok:
                return out
            if verb == "Enable" and self._x_wait_any(client, ["Enable %s" % package_id], 1.5):  # Needs consent: Enable asks first
                out = self._x_click(client, "Enable %s" % package_id, 6.0)
        return Outcome(status=200, ok=bool(out and out.ok), refused=not (out and out.ok), data={"toggled": names})

    def _x_cli(self, client, verb_args, args):
        """Run `fe-ext admin --json <verb>` on the local channel; the reply frame is the outcome (consent stays a GUI click)."""
        exe = self.fe_ext_exe()
        if not exe:
            return self._refused("no_fe_ext", "fe-ext was not staged (stage.ps1 builds it) and target/debug has none")
        env = dict(os.environ)
        instance = args.get("instance")
        if instance:
            env[self.EXT_CLI_ENV] = instance
        try:
            done = subprocess.run([exe, "admin", "--json"] + verb_args, capture_output=True, text=True, env=env,
                                  timeout=float(args.get("cli_timeout_s", 120)))
        except (OSError, subprocess.SubprocessError) as error:
            return self._refused("cli_failed", str(error)[:300])
        try:
            reply = json.loads(done.stdout.strip().splitlines()[-1]) if done.stdout.strip() else {}
        except ValueError:
            reply = {}
        status = reply.get("status")
        if status == "ok":
            return Outcome(status=200, ok=True, refused=False, data=reply.get("result") or {})
        return Outcome(status=200, ok=False, refused=True, code=reply.get("code") or "cli_failed",
                       error=(reply.get("message") or done.stderr or "fe-ext admin failed")[:300])

    def fe_ext_exe(self):
        """The staged fe-ext (beside the staged fractalengine.exe), else target/debug."""
        for base in (self.stage_dir, os.path.join(self.repo_root, "target", "debug")):
            candidate = os.path.join(base, "fe-ext.exe") if base else None
            if candidate and os.path.isfile(candidate):
                return candidate
        return None

    def _x_disable(self, client, args):
        if args.get("channel", "cli") == "gui":
            return self._x_toggle(client, args, "Disable")
        return self._x_cli(client, ["disable", args["installation"]] + (["--binding", args["binding"]] if args.get("binding") else []), args)

    def _x_enable(self, client, args):
        if args.get("channel", "cli") == "gui":
            return self._x_toggle(client, args, "Enable")
        return self._x_cli(client, ["enable", args["installation"]] + (["--binding", args["binding"]] if args.get("binding") else []), args)

    def _x_bind(self, client, args):
        """ext_bind: enable the installation's binding rows in the panel, then name the nodes the instance made (`discover`).

        Binding config (movers, hz, devices) is the manifest's: the panel has no config form, so `config` is recorded, not sent.
        """
        enabled = self._x_toggle(client, {**args, "channel": "gui"}, "Enable")
        if not enabled.ok:
            return enabled
        data = dict(enabled.get("data") or {})
        if args.get("config"):
            data["config_note"] = "panel binding uses manifest defaults; config %s was not sent" % json.dumps(args["config"], sort_keys=True)
        prefix = args.get("discover")
        if prefix and args.get("petal_id"):  # the same node discovery as the REST era: mover0.. as probe targets
            sql = "SELECT node_id, display_name FROM node WHERE petal_id = %s AND tombstone = NONE" % surreal_string(args["petal_id"])
            found = self._poll_lookup(client, lambda: client.rest("POST", "/api/v1/query", {"sql": sql}),
                                      lambda rows: [r for r in rows if str(r.get("display_name", "")).startswith(prefix)]
                                      if isinstance(rows, list) else None, float(args.get("wait_s", 20)))
            if found.ok:
                nodes = sorted(found["data"].get("id") or [], key=lambda r: str(r.get("display_name")))
                data["movers"] = [n.get("node_id") for n in nodes]
                for index, node in enumerate(nodes):
                    data["%s%d" % (prefix, index)] = node.get("node_id")
        return Outcome(status=200, ok=True, refused=False, data=data)

    def _x_update(self, client, args):
        """ext_update: Open the row, press "Update <id> to <version>"; a changed capability digest raises the dialog (ext_consent reprompt)."""
        _name, package_id, version, _path = self._x_package({**args, "package_id": args.get("package_id") or self.installs.get(args.get("installation", ""))})
        opened = self._x_open_panel(client, "Installed extensions")
        if not opened.ok:
            return opened
        opened = self._x_click(client, "Open %s" % package_id, 8.0)
        if not opened.ok:
            return opened
        name = "Update %s to %s" % (package_id, version) if version else next(
            (n for n, on in self._x_nodes(client, "Update %s to " % package_id) if on), "Update %s" % package_id)
        return self._x_click(client, name, float(args.get("wait_s", 10)))

    def _x_uninstall(self, client, args):
        """ext_uninstall: the two-step Uninstall / Confirm uninstall on the row's Overview."""
        package_id = args.get("package_id") or self.installs.get(args.get("installation", "")) or self._x_package(args)[1]
        opened = self._x_open_panel(client, "Installed extensions")
        if not opened.ok:
            return opened
        opened = self._x_click(client, "Open %s" % package_id, 8.0)
        if not opened.ok:
            return opened
        for name in ("Uninstall %s" % package_id, "Confirm uninstall %s" % package_id):
            out = self._x_click(client, name, 6.0)
            if not out.ok:
                return out
        return out

    def _x_list(self, client, args):
        return Outcome(status=200, ok=True, refused=False, data={"installations": self._x_installations(client)})

    def _ext_set_extension_policy(self, client, args, verse):
        body = {k: v for k, v in args.items() if k != "verse_id"}
        return client.rest("PUT", "/api/v1/verses/%s/extension-policy" % verse, body)

    def _ext_extension_command(self, client, args, verse):
        path = "/api/v1/petals/%s/extensions/%s/commands/%s" % (args["petal_id"], self._package_of(args), args["command"])
        out = client.rest("POST", path, {"args": args.get("args", {})})
        if args.get("door") == "ws":  # no WS client in the simulator yet: the REST door answers for it, and the record says so
            out["door"] = "rest-standing-in-for-ws"
        return out

    def _ext_set_binding(self, client, args, verse):
        body = {k: v for k, v in args.items() if k not in ("clear",)}
        body.setdefault("verse_id", verse)
        if args.get("clear"):
            return client.rest("DELETE", "/api/v1/bindings", {k: body[k] for k in ("verse_id", "fractal_id", "petal_id", "hook", "key") if k in body})
        return client.rest("PUT", "/api/v1/bindings", body)

    def _ext_publish_revocation(self, client, args, verse):
        if self.loopback is None:
            return self._refused("no_loopback", "publish_revocation needs run.py's loopback services")
        entries = [{k: v for k, v in e.items()} for e in args.get("entries", [])]
        return Outcome(status=200, ok=True, refused=False, data=self.loopback.publish_revocation(entries))

    @staticmethod
    def _frame_value(index):
        return round(20 + 4 * math.sin(index / 4.0) + 0.01 * (index % 7), 3)  # the same series for every device: parity compares like with like

    def _frame(self, index, metric):
        at_ms = 1700000000000 + (self.scenario["seed"] % 1000) * 1000 + index * 1000
        return {"readings": [{"metric": metric, "value": self._frame_value(index), "units": "C", "at_ms": at_ms}]}

    def _device_node(self, client, petal, device):
        """The node named by the device id (device.id property set), created once through the owner when absent."""
        key = (petal, device)
        if key in self.device_nodes:
            return self.device_nodes[key]
        provisioner = self.clients.get("owner", client)
        found = self.rest(provisioner, "lookup_node", {"petal_id": petal, "name": device, "wait_s": 0})
        node_id = found["data"].get("id") if found.ok else None
        if not node_id:
            made = self.rest(provisioner, "create_node", {"fractal_id": self.ctx.try_resolve("$fixture.fractal_id"),
                                                          "petal_id": petal, "name": device, "position": [0, 0, 0]})
            node_id = extract_id(made.get("data")) if made.ok else None
            if node_id:
                self.rest(provisioner, "set_property", {"node_id": node_id, "key": "device.id", "value": device})
        if node_id:
            self.device_nodes[key] = node_id
        return node_id

    def _ext_ingress_post(self, client, args, verse):
        petal, endpoint = args["petal_id"], args.get("endpoint", "telemetry")
        path = "/api/v1/petals/%s/extensions/%s/ingress/%s" % (petal, self._package_of(args), endpoint)
        devices, count, period = args.get("devices", []), int(args.get("count", 1)), float(args.get("period_s", 1))
        for device in devices:
            if not self._device_node(client, petal, device):
                return self._refused("device_node_unavailable", "no node could be made for %s" % device)
        token, _source = client.verse_token(verse)

        def post(device, index):
            frame = self._frame(index, args.get("metric", "temp_c"))
            frame["device_id"] = device
            return client.send("POST", path, None, token, 15.0, raw=json.dumps(frame).encode("utf-8"), content_type="application/json")

        first = post(devices[0], 0) if devices else self._refused("no_devices", "ingress_post needs devices")
        if not first.ok:  # a refusal (role floor, disabled) is the answer; nothing more is sent
            return first
        failures = [0]

        def run_device(device):
            start = self.clock.now()
            for index in range(0 if device != devices[0] else 1, count):
                if index and period:
                    self.clock.sleep_until(start + index * period)
                if self.abort.is_set():
                    break
                if not post(device, index).ok:
                    failures[0] += 1
        with ThreadPoolExecutor(max_workers=max(1, len(devices))) as pool:
            list(pool.map(run_device, devices))
        return Outcome(status=200, ok=failures[0] == 0, refused=failures[0] > 0, code=None if not failures[0] else "partial",
                       data={"devices": devices, "frames": count * len(devices), "failed": failures[0]})

    def _ext_mqtt_publish(self, client, args, verse):
        if self.loopback is None:
            return self._refused("no_loopback", "mqtt_publish needs run.py's loopback broker")
        broker = self.loopback.broker
        if args.get("start_broker"):
            self.loopback.start()
        devices, topic = args.get("devices"), args["topic"]
        qos = int(args.get("qos", 1))
        if not devices:
            delivered = broker.inject(topic, args.get("payload", "{}"), qos)
            return Outcome(status=200, ok=True, refused=False, data={"delivered": delivered, "topic": topic})
        deadline = time.monotonic() + float(args.get("subscriber_wait_s", 30))
        while not broker.subscribers(topic.replace("{device}", devices[0])) and time.monotonic() < deadline:
            time.sleep(0.25)  # the host dials the broker after bind; a frame sent before its SUBSCRIBE would be lost
        if not broker.subscribers(topic.replace("{device}", devices[0])):
            return self._refused("no_subscriber", "no session subscribed to %s within the wait" % topic)
        count, period = int(args.get("count", 1)), float(args.get("period_s", 1))
        for device in devices:
            self._device_node(client, args.get("petal_id") or self.ctx.try_resolve("$fixture.petal_a_id"), device)
        start, sent = self.clock.now(), 0
        for index in range(count):
            if index and period:
                self.clock.sleep_until(start + index * period)
            if self.abort.is_set():
                break
            for device in devices:
                sent += broker.inject(topic.replace("{device}", device), json.dumps(self._frame(index, args.get("metric", "temp_c"))), qos)
        return Outcome(status=200, ok=sent > 0, refused=sent == 0, code=None if sent else "undelivered", data={"delivered": sent})

    def _ingest_device(self, client, args):
        """The REST readings door for one device: same series as ingress_post, so adapter and door rows can be compared."""
        petal, device = args["petal_id"], args["device"]
        node_id = self._device_node(client, petal, device)
        if not node_id:
            return self._refused("device_node_unavailable", "no node for %s" % device)
        base = 1700000000000 + (self.scenario["seed"] % 1000) * 1000
        readings = []
        for index in range(int(args.get("count", 1))):
            at_ms = base + index * 1000
            stamp = time.strftime("%Y-%m-%dT%H:%M:%S", time.gmtime(at_ms // 1000)) + ".%03dZ" % (at_ms % 1000)
            readings.append({"node_id": node_id, "metric": args.get("metric", "temp_c"), "value": self._frame_value(index),
                             "units": "C", "recorded_at": stamp})
        return client.rest("POST", "/api/v1/petals/%s/iot/readings" % petal, {"readings": readings})

    @staticmethod
    def _poll_lookup(client, request, pick, wait_s, interval=0.5):
        deadline = time.monotonic() + wait_s
        while True:
            out = request()
            found = pick(out.get("data")) if out.ok else None
            if found:
                out["data"] = found if isinstance(found, dict) else {"id": found}
                return out
            if time.monotonic() >= deadline:
                return Outcome(status=out.get("status"), ok=False, refused=True, code="lookup_not_found",
                               error="not found within %.0fs" % wait_s)
            time.sleep(interval)

    def host_op(self, client, op, args):
        if op == "sleep":
            time.sleep(float(args.get("seconds", 1)))
            return Outcome(status=200, ok=True, refused=False, data={"slept": args.get("seconds", 1)})
        if op == "stop_peer":
            if self.host is None:
                return Outcome(status=0, ok=False, refused=True, code="no_host", error="stop_peer needs run.py's host")
            return Outcome(status=200, ok=True, refused=False, data=self.host.stop_peer(client.name))
        if op == "restart":
            if self.host is None:
                return Outcome(status=0, ok=False, refused=True, code="no_host", error="restart needs run.py's host")
            data = self.host.restart(client.name, bool(args.get("no_bootstrap")))
            client.run(force=True)
            return Outcome(status=200, ok=True, refused=False, data=data)
        if op == "wait_role":
            verse = args.get("verse_id") or client.verse_id
            deadline = time.monotonic() + float(args.get("timeout_s", 30))
            while time.monotonic() < deadline:
                role = client.role_in(verse)
                if ("role" in args and role == args["role"]) or ("not_role" in args and role != args["not_role"]):
                    return Outcome(status=200, ok=True, refused=False, data={"role": role})
                time.sleep(0.5)
            return Outcome(status=0, ok=False, refused=True, code="role_wait_timeout", data={"role": client.role_in(verse)})
        if op == "fingerprint":
            return Outcome(status=200, ok=True, refused=False, data=self.fingerprint(args.get("label", "mid")))
        raise ValueError("unknown host op " + op)

    # -- portal ops (tools/qa/portal/AGENTS.md section sim-contract) --------------------------------
    def portal_op(self, client, op, args, verse):
        """Portal ops; admission and re-grant drive the GUI, the kill switch is a fixed debug route, IPC row posts refuse by
        name (no seam yet)."""
        if op == "upload_bundle":
            out = portal_probe.upload_bundle(client, args)
            data = out.get("data")
            if out.ok and isinstance(data, dict):
                data.setdefault("id", data.get("asset_id"))  # scenarios bind data.id like upload_asset
                if data.get("package_id") and data.get("asset_id"):
                    assets = getattr(self.ctx, "portal_assets", None)
                    if assets is None:
                        assets = self.ctx.portal_assets = {}
                    assets[data["package_id"]] = data["asset_id"]
            return out
        if op == "create_repository_node":
            created = client.rest("POST", "/api/v1/verses/%s/fractals/%s/petals/%s/nodes" % (
                verse, args["fractal_id"], args["petal_id"]),
                {"name": args["name"], "position": args.get("position", [0, 0, 0])})
            node_id = extract_id(created.get("data")) if created.ok else None
            if not node_id:
                return created
            for key, value in (("node_kind", "repository"), ("portal.bundle_asset", args["bundle_asset"])):
                written = client.rest("PATCH", "/api/v1/nodes/%s/properties" % node_id, {"key": key, "value": value})
                if not written.ok:
                    return written
            return created
        if op == "portal_update_bundle":
            return client.rest("PATCH", "/api/v1/nodes/%s/properties" % args["repository_node_id"],
                               {"key": "portal.bundle_asset", "value": args["bundle_asset"]})
        if op in ("portal_observe", "portal_ipc_stats"):
            route = "observe" if op == "portal_observe" else "ipc-stats"
            return client.send("GET", portal_probe.DEBUG_BASE + route, None, client.debug_token(), 8.0)
        if op == "portal_slot":
            return client.send("POST", portal_probe.DEBUG_BASE + "action", {"slot": int(args["slot"])},
                               client.debug_token(), 8.0)
        if op == "portal_killswitch":
            # Fixed body {engage} only, behind the portal-fixture Owner+input gate (steer 2026-11-09 02:15).
            return client.send("POST", portal_probe.DEBUG_BASE + "killswitch", {"engage": bool(args.get("engage", True))},
                               client.debug_token(), 8.0)
        if op == "portal_ipc_post":
            # Fixed PJ2 row (protocol.md section 9): {row} plus optional mount pins, same gate as the kill switch.
            body = {"row": str(args["row"])}
            for key in ("package_id", "content_hash"):
                if args.get(key):
                    body[key] = str(args[key])
            return client.send("POST", portal_probe.DEBUG_BASE + "ipc-post", body, client.debug_token(), 8.0)
        if op == "portal_admit":
            return self._portal_admit(client, args)
        if op == "regrant_role":
            return self._regrant_role(client, args)
        return Outcome(status=0, ok=False, refused=True, code="portal_op_unwired",
                       error="%s has no API route or GUI affordance in this build" % op)

    def _portal_admit(self, client, args):
        """`portal_admit`: the repository view's "Admit bundle" button (ux.md section 5), then the consent dialog's "Enable <package_id>"."""
        out = client.gui("click", {"name": "Admit bundle", "exact": True, "wait_s": float(args.get("wait_s", 5))})
        if out.ok and args.get("package_id"):  # the admit consent dialog confirms with "Enable <package_id>" (governance consent)
            out = client.gui("click", {"name": "Enable %s" % args["package_id"], "exact": True, "wait_s": float(args.get("confirm_wait_s", 8))})
        return out

    def _regrant_role(self, client, args):
        """`regrant_role`: the Access tab assign chain of p2p-core 13c; clicks stop at the first refusal."""
        target = self.clients[args["peer"]]
        role, did = args.get("role", "editor"), target.did
        verse_name = args.get("verse_name", "QA Basic")
        chain = [("Browse current verse fractals", None), ("Options menu for verse " + verse_name, None),
                 ("Options… verse " + verse_name, None), ("Access", "button"),
                 ("Role to assign for " + did, None), (role, None),
                 ("Assign role %s to %s" % (role, did), None),
                 ("Confirm Assign role %s to %s" % (role, did), None)]
        out = None
        for name, kind in chain:
            click = {"name": name, "exact": True, "wait_s": 3}
            if kind:
                click["role"] = kind
            out = client.gui("click", click)
            if not out.ok:
                return out
        client.gui("click", {"name": "Close window", "role": "button", "wait_s": 1})
        return out

    # -- macros ----------------------------------------------------------------------------------
    def macro(self, client, op, args, sid, action_id, action):
        if op == "place_grid":
            return self._place_grid(client, args, sid, action_id)
        if op == "drone_flight":
            return self._drone_flight(client, args, sid, action_id)
        if op == "freeplay":
            return self._freeplay(client, args, sid, action_id)
        raise ValueError("unknown macro " + op)

    def _sub(self, client, sid, action_id, index, via, op, args, expect="ok"):
        """Run and log one macro sub-action (ids '<macro>.<n>'), so macros stay replayable."""
        base = {"step": sid, "lane": "macro", "peer": client.name, "action_id": "%s.%d" % (action_id, index),
                "via": via, "op": op, "expect": expect, "sync": "macro", "optional": False,
                "args_digest": digest_of(args), "args_small": small(_scrub(args), 1024)}
        t_fire = self.clock.now()
        try:
            outcome = client.mcp(op, args) if via == "mcp" else self.rest(client, op, args)
        except Exception as error:  # noqa: BLE001
            outcome = Outcome(status=-2, ok=False, refused=False, code="harness_error", error=str(error)[:300])
        t_ack = self.clock.now()
        self._event(base, status="done", t_fire=t_fire, t_ack=t_ack, ok=outcome.ok, http_status=outcome.get("status"),
                    code=outcome.get("code"), error=outcome.get("error"), verdict=verdict_of(expect, outcome),
                    token_source=outcome.get("token_source"), result_id=extract_id(outcome.get("data")),
                    result_digest=digest_of(outcome.get("data")), result_small=None, **outcome_extras(outcome))
        return outcome

    def _place_grid(self, client, args, sid, action_id):
        ids, failures = [], 0
        ox, oy, oz = args.get("origin", [0, 0, 0])
        index = 0
        for row in range(int(args["rows"])):
            for col in range(int(args["cols"])):
                position = [ox + col * float(args["spacing"]), oy, oz + row * float(args["spacing"])]
                call = {"petal_id": args["petal_id"], "asset_id": args["asset_id"], "position": position,
                        "name": "%s %d-%d" % (args.get("name_prefix", "Building"), row, col)}
                out = self._sub(client, sid, action_id, index, "mcp", "place_asset", call)
                index += 1
                node_id = extract_id(out.get("data"))
                if out.ok and node_id:
                    ids.append(node_id)
                else:
                    failures += 1
        return Outcome(status=200, ok=failures == 0, refused=failures > 0, code=None if not failures else "partial",
                       data={"ids": ids, "placed": len(ids), "failed": failures})

    def _skipped_tick(self, client, sid, action_id, tick, period, late):
        """Log one skipped flight tick: its wall-clock slot had already passed while earlier calls ran."""
        base = {"step": sid, "lane": "macro", "peer": client.name, "action_id": "%s.skip%d" % (action_id, tick),
                "via": "macro", "op": "skipped_tick", "expect": "any", "sync": "macro", "optional": True,
                "tick": tick, "scheduled_s": round(tick * period, 3), "late_ms": round(late * 1000, 1)}
        self._event(base, status="skipped_tick")

    def _drone_flight(self, client, args, sid, action_id):
        nodes = list(args["node_ids"])
        rate = float(args.get("rate_hz", 2))
        period = 1.0 / rate
        ticks = int(float(args.get("duration_s", 180)) * rate)
        radius, altitude = float(args.get("radius", 20)), float(args.get("altitude", 30))
        scale = [float(value) for value in args.get("scale", [1, 1, 1])]
        every = int(args.get("payload_every", 0) or 0)
        start = self.clock.now()
        flight_end = start + ticks * period
        index = failures = skipped = 0
        # A tick's writes go out together (serial round trips of ~130 ms each skipped 101 of 360 ticks, QA-0152 e).
        with ThreadPoolExecutor(max_workers=2 * len(nodes) + 1) as pool:
            for tick in range(ticks):
                if self.abort.is_set():
                    break
                late = self.clock.now() - (start + tick * period)
                if late >= period:  # wall-clock schedule: a tick whose whole slot passed is skipped, never caught up
                    skipped += 1
                    self._skipped_tick(client, sid, action_id, tick, period, late)
                    continue
                self.clock.sleep_until(start + tick * period)
                calls, readings = [], []
                stamp = time.strftime("%Y-%m-%dT%H:%M:%S", time.gmtime()) + ".%03dZ" % (int(time.time() * 1000) % 1000)
                for k, node_id in enumerate(nodes):
                    if self.clock.now() >= flight_end:  # a hung call must not stretch the flight past its end
                        break
                    angle = 2 * math.pi * (tick / max(1.0, rate * float(args.get("period_s", 30))) + k / len(nodes))
                    position = [round(radius * math.cos(angle), 3), round(altitude + 2 * k, 3), round(radius * math.sin(angle), 3)]
                    calls.append(("mcp", "update_transform", {"node_id": node_id, "position": position,
                                                              "rotation": [0, round(angle, 4), 0], "scale": scale}))
                    readings.append({"node_id": node_id, "metric": args.get("metric", "altitude_m"),
                                     "value": position[1] + 0.01 * tick, "units": "m", "recorded_at": stamp})
                    if every and tick % every == 0 and args.get("format_id"):
                        payload = json.dumps({"node": node_id, "tick": tick, "position": position}).encode("utf-8")
                        calls.append(("mcp", "ingest_device_payload",
                                      {"petal_id": args["petal_id"], "node_id": node_id, "format_id": args["format_id"],
                                       "files": [base64.b64encode(payload).decode("ascii")], "recorded_at": stamp,
                                       "payload_id": new_ulid()}))
                if readings and self.clock.now() < flight_end:
                    calls.append(("rest", "ingest_readings", {"petal_id": args["petal_id"], "readings": readings}))
                futures = []
                for via, op, call in calls:  # sub-action ids '<macro>.<n>' are assigned in call order, whatever finishes first
                    futures.append(pool.submit(self._sub, client, sid, action_id, index, via, op, call))
                    index += 1
                for future in futures:
                    failures += 0 if future.result().ok else 1
        return Outcome(status=200, ok=failures == 0, refused=failures > 0, code=None if not failures else "partial",
                       data={"ticks": ticks, "calls": index, "failed": failures, "skipped_ticks": skipped})

    def _freeplay(self, client, args, sid, action_id):
        """Seeded role-appropriate random actions; seed = scenario seed + step + peer, so a run is replayable."""
        seed_text = "%s/%s/%s" % (self.scenario["seed"], sid, client.name)
        rng = random.Random(int(hashlib.sha256(seed_text.encode()).hexdigest()[:12], 16))
        targets = [t for t in args["targets"] if t]
        mode = args.get("mode", "writer")
        rate = float(args.get("rate_hz", 1))
        start = self.clock.now()
        count = int(float(args.get("duration_s", 60)) * rate)
        accepted_denied = 0
        sentinel = float(args.get("sentinel", 77))
        for index in range(count):
            if self.abort.is_set():
                break
            self.clock.sleep_until(start + index / rate + rng.uniform(0, 0.3 / rate))
            node_id = rng.choice(targets)
            choice = rng.random()
            if mode == "denied":
                if choice < 0.5:
                    via, op, call = "rest", "set_transform", {"node_id": node_id, "position": [sentinel, 0, sentinel],
                                                             "rotation": [0, 0, 0], "scale": [1, 1, 1]}
                else:
                    via, op, call = ("mcp", "set_property", {"node_id": node_id, "key": "fp_deny_%s" % client.name,
                                                             "value": index})
                out = self._sub(client, sid, action_id, index, via, op, call, expect="refused")
                accepted_denied += 1 if out.ok else 0
                continue
            if choice < 0.4:
                via, op, call = "rest", "set_transform", {
                    "node_id": node_id, "position": [round(rng.uniform(-8, 8), 2), 0, round(rng.uniform(-8, 8), 2)],
                    "rotation": [0, round(rng.uniform(0, 3.1), 3), 0], "scale": [1, 1, 1]}
            elif choice < 0.65:
                via, op, call = "rest", "set_property", {"node_id": node_id, "key": "fp_%s" % client.name,
                                                         "value": rng.randint(0, 999)}
            elif choice < 0.8:
                via, op, call = "rest", "delete_property", {"node_id": node_id, "key": "fp_%s" % client.name}
            else:
                via, op, call = "mcp", "set_property", {"node_id": node_id, "key": "fp_mcp_%s" % client.name,
                                                        "value": rng.randint(0, 999)}
            self._sub(client, sid, action_id, index, via, op, call, expect="any")
        return Outcome(status=200, ok=accepted_denied == 0, refused=accepted_denied > 0,
                       code=None if not accepted_denied else "denied_but_accepted",
                       data={"actions": count, "seed": seed_text, "denied_but_accepted": accepted_denied})

    # -- expectations ----------------------------------------------------------------------------
    def register_expectations(self, step, sid, events, t_actions):
        fired = {e["action_id"]: e for e in events if e.get("t_fire") is not None and e.get("status") == "done"}
        default_t0 = max([e["t_fire"] for e in fired.values()], default=t_actions)
        instances = []
        for index, expect in enumerate(step.get("expect", [])):
            exp_id = "%s.e%d" % (sid, index)
            peers = expect.get("peers", "all")
            peers = [p for p in (list(self.clients) if peers == "all" else peers) if p in self.present]
            record = {"exp_id": exp_id, "step": sid, "probe": expect["probe"], "source": source_probe(expect["probe"]),
                      "predicate": expect["predicate"], "mode": expect.get("mode", "eventually"),
                      "within_s": float(expect.get("within_s", step["settle_s"] or 10)),
                      "hold_s": float(expect.get("hold_s", 0)), "peers": peers, "covers": expect.get("covers"),
                      "note": expect.get("note"), "after": expect.get("after"),
                      "optional": bool((expect.get("meta") or {}).get("optional"))}
            if expect.get("after"):
                anchor = fired.get(expect["after"])
                # A macro is a write burst whose state cannot settle while it runs: judge from its completion.
                burst = bool(anchor) and anchor.get("via") == "macro"
                record["t0"] = (anchor["t_ack"] if burst else anchor["t_fire"]) if anchor else None
                record["t0_basis"] = ("completion" if burst else "fire") if anchor else None
            else:
                record["t0"] = default_t0
                record["t0_basis"] = "step_last_fire"
            unmet = self.unmet_needs(expect)  # judged at registration, after the step's actions have all finished
            if unmet:  # its action never ran ok: not exercised (the failed action carries its own anomaly)
                record.update(status="skipped_dependency_failed", needs_failed=unmet)
                self.recorder.write("expectations", record)
                instances.append(record)
                continue
            try:
                record["target"] = self.ctx.resolve(expect["target"])
                record["value"] = self.ctx.resolve(expect.get("value"))
                record["path"] = self.ctx.resolve(expect.get("path")) if expect.get("path") else None
                record["meta"] = self.ctx.resolve(expect.get("meta", {}))
                record["target_key"] = target_key(record["target"])
                if not peers:
                    record["status"] = "skipped_peers_absent"
                else:
                    record["status"] = "registered" if record["t0"] is not None else "no_anchor"
            except UnresolvedRef as missing:
                record["status"] = "unresolved"
                record["error"] = "unresolved $%s" % missing.ref
                record["unresolved_ref"] = missing.ref
                record["unresolved_reason"] = missing.reason
            self.recorder.write("expectations", record)
            instances.append(record)
        return instances

    def live_holds(self, record):
        """Per-peer live verdict from the prober's latest observations taken after t0.

        A reading counts once its prober round has closed (the peer's `_tick` is logged): the analyzer samples at ticks
        and change times, so a step that settled on a reading the log has no tick for yet closed before the analyzer
        could see the value held (QA-0163 c: three of four peers' 13a.e4-e6 first tick fell 0.04-0.84 s after the step closed). A store that never
        ticked (a test stub without a prober) counts every reading.
        """
        if record.get("status") != "registered":
            return {}
        last_tick = getattr(self.store, "last_tick", None)
        values = {}
        for peer in record["peers"]:
            latest = self.store.get(peer, record["source"], record["target_key"])
            ticked = last_tick(peer) if last_tick else None
            logged = bool(latest) and latest[0] >= record["t0"] and (ticked is None or ticked >= latest[0])
            values[peer] = predicates.derive(record["probe"], latest[1]) if logged else predicates.MISSING
        if record["predicate"] == "agree":
            together = predicates.agree(values, record.get("path"))
            return {peer: together for peer in values}
        return {peer: value is not predicates.MISSING and predicates.evaluate(
            record["predicate"], value, record.get("value"), record.get("path")) for peer, value in values.items()}

    def wait_settle(self, step, instances, t_done):
        """Wait until every eventually-expectation holds live (plus hold_s) or the step's budget runs out.

        The budget is the longer of `settle_s` after the step's actions and every watched expectation's own `within_s`
        after its anchor, so a window is never cut shorter than the scenario declared it (QA-0163 c: 13a's `reenter`
        fires at +90 s, past settle_s, and its 30 s windows were judged 0.4 s later).
        """
        watch = [r for r in instances if r.get("status") == "registered" and r["mode"] == "eventually"]
        deadline = max([t_done + float(step["settle_s"])] + [r["t0"] + r["within_s"] for r in watch])
        hold = max([r["hold_s"] for r in watch] + [0.0])
        all_met_at = None
        while not self.abort.is_set():
            now = self.clock.now()
            unmet = ["%s@%s" % (r["exp_id"], p) for r in watch for p, ok in self.live_holds(r).items() if not ok]
            if not unmet and all_met_at is None:
                all_met_at = now
            if unmet:
                all_met_at = None
            if all_met_at is not None and now - all_met_at >= hold:
                return all_met_at, []
            if now >= deadline:
                return None, unmet
            time.sleep(LIVE_POLL_S)
        return None, ["aborted"]

    # -- checkpoints and fingerprints ------------------------------------------------------------
    def checkpoint(self, checkpoint, sid):
        peers = list(self.present) if checkpoint["peer"] == "all" else [checkpoint["peer"]]
        threads = []
        for peer in peers:
            if peer not in self.present:
                continue

            def shoot(name=peer):
                path = os.path.join(self.run_dir, "shots", "%s-%s.png" % (sid, name))
                base = {"step": sid, "lane": "checkpoint", "peer": name, "action_id": "%s.shot.%s" % (sid, name),
                        "via": "host", "op": "checkpoint", "expect": "any", "sync": "checkpoint", "optional": True,
                        "capture": checkpoint.get("capture", "scene"), "expect_visual": checkpoint.get("expect_visual")}
                t_fire = self.clock.now()
                try:
                    data = self.clients[name].screenshot(path, checkpoint.get("capture", "scene"), checkpoint.get("pose"))
                    self._event(base, status="done", t_fire=t_fire, t_ack=self.clock.now(), ok=True,
                                result_small=data, verdict="as_expected")
                except Exception as error:  # noqa: BLE001
                    self._event(base, status="done", t_fire=t_fire, t_ack=self.clock.now(), ok=False,
                                error=str(error)[:300], verdict="optional_capture_failed")
            threads.append(threading.Thread(target=shoot, daemon=True))
        for thread in threads:
            thread.start()
        for thread in threads:
            thread.join()

    def fingerprint(self, label):
        """Per-peer canonical view of the shared verse (hierarchy + node rows + groups + terrain)."""
        verse = self.ctx.try_resolve("$fixture.verse_id")
        out = {}
        for name in sorted(self.present):
            client = self.clients[name]
            hierarchy = client.rest("GET", "/api/v1/hierarchy")
            rows = {}
            for verse_row in hierarchy.get("data") or [] if hierarchy.ok else []:
                if not isinstance(verse_row, dict) or verse_row.get("id") != verse:
                    continue
                for fractal in verse_row.get("fractals", []):
                    rows["F " + fractal["id"]] = fractal.get("name")
                    for petal in fractal.get("petals", []):
                        rows["P " + petal["id"]] = petal.get("name")
                        groups = client.rest("GET", "/api/v1/petals/%s/visibility-groups" % petal["id"])
                        rows["G " + petal["id"]] = sorted(((groups.get("data") or {}).get("groups") or []),
                                                          key=lambda g: str(g.get("id"))) if groups.ok else None
                        terrain = client.rest("GET", "/api/v1/petals/%s/terrain" % petal["id"])
                        rows["T " + petal["id"]] = digest_of(terrain.get("data")) if terrain.ok else None
                        nodes = petal.get("nodes", [])
                        if len(nodes) > FINGERPRINT_NODE_CAP:  # never a silent cut: the key carries the full count
                            rows["X " + petal["id"]] = {"nodes": len(nodes), "fingerprinted": FINGERPRINT_NODE_CAP}
                            self.log("  ! fingerprint %s/%s: petal %s has %d nodes, only the first %d are compared"
                                     % (label, name, petal["id"], len(nodes), FINGERPRINT_NODE_CAP))
                        for node in nodes[:FINGERPRINT_NODE_CAP]:
                            detail = client.rest("GET", "/api/v1/nodes/%s" % node["id"])
                            data = detail.get("data") if detail.ok else None
                            fields = {k: data.get(k) for k in ("display_name", "position", "rotation", "scale",
                                                               "properties", "asset_id", "petal_id")} if isinstance(data, dict) else {"unreadable": detail.get("code")}
                            rows["N " + node["id"]] = predicates.normalize({**fields, "asset_path": node.get("asset_path")})
            out[name] = {"hierarchy_ok": hierarchy.ok, "sha256": predicates.digest(rows),
                         "counts": {k: sum(key.startswith(k + " ") for key in rows) for k in "FPN"}, "rows": rows}
        path = os.path.join(self.run_dir, "fingerprint-%s.json" % label)
        with open(path, "w", encoding="utf-8") as handle:
            json.dump(out, handle, indent=1, sort_keys=True)
        self.recorder.write("steps", {"step": "_fingerprint", "event": label, "t": self.clock.now(), "path": path,
                                      "sha256": {p: v["sha256"] for p, v in out.items()}})
        return {p: v["sha256"] for p, v in out.items()}


def _scrub(value):
    """Drop bulky/secret-looking fields from logged args/results (base64 payloads, tokens)."""
    if isinstance(value, dict):
        return {k: ("<%d chars>" % len(v) if isinstance(v, str) and (len(v) > 512 or "token" in k.lower()) else _scrub(v))
                for k, v in value.items()}
    if isinstance(value, list):  # the logged copy keeps 64 items and says how many it dropped; args_digest covers all
        return [_scrub(v) for v in value[:64]] + (["<%d more>" % (len(value) - 64)] if len(value) > 64 else [])
    if isinstance(value, str) and len(value) > 512:
        return "<%d chars>" % len(value)
    return value
