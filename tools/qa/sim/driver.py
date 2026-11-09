"""Scenario driver: one process, concurrent peer lanes, threading.Barrier same-instant fires, every action logged; see tools/qa/AGENTS.md §sim."""
import base64
import hashlib
import json
import math
import os
import random
import re
import threading
import time
import urllib.parse
from concurrent.futures import ThreadPoolExecutor

import portal_probe
import predicates
from client import Outcome, extract_id
from runtime import small
from scenario import UnresolvedRef, source_probe, target_key

BARRIER_LEAD_S = 0.5
BARRIER_TIMEOUT_S = 120.0
NEEDS_WAIT_S = 90.0  # longest a dependent waits for a same-step dependency still in flight before it counts as unmet
LIVE_POLL_S = 0.25
FINGERPRINT_NODE_CAP = 400
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
                 steps=None, log=print):
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
        if via == "host":
            return self.host_op(client, op, args)
        if via == "macro":
            return self.macro(client, op, args, sid, action_id, action)
        raise ValueError("unknown via " + via)

    def rest(self, client, op, args):
        verse = args.get("verse_id") or self.ctx.try_resolve("$fixture.verse_id")
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
        """Portal rest ops; admission, kill switch, IPC posts and re-grant have no API route yet and refuse by name."""
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
        return Outcome(status=0, ok=False, refused=True, code="portal_op_unwired",
                       error="%s has no API route in this build (admission and re-grant seams are not wired)" % op)

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
