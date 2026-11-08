"""Scenario DSL: JSON loader, structural validator, JSON Schema and $-reference resolution; see tools/qa/AGENTS.md §sim."""
import copy
import json
import os
import re
import threading

from predicates import DERIVED, PREDICATES

HERE = os.path.dirname(os.path.abspath(__file__))
REPO_ROOT = os.path.abspath(os.path.join(HERE, "..", "..", ".."))
SEARCH_DIRS = (os.path.join(REPO_ROOT, "qa", "sim", "scenarios"), os.path.join(HERE, "scenarios"))

VIAS = {
    "rest": {"request", "create_fractal", "create_petal", "create_node", "set_transform", "set_property",
             "delete_property", "delete_node", "upsert_group", "delete_group", "upload_asset", "query",
             "lookup_node", "lookup_petal", "lookup_group", "ingest_readings"},
    "mcp": None,  # any MCP tool name; args are the tool arguments
    "gui": {"click", "click_until", "type", "key", "scroll", "scroll_until", "wait_for", "click_at"},
    "host": {"restart", "stop_peer", "sleep", "wait_role", "fingerprint"},
    "macro": {"place_grid", "drone_flight", "freeplay"},
}
PROBES = ("node", "transform", "properties", "hierarchy", "groups", "terrain", "role", "nav", "ui",
          "asset_blob", "query", "timeseries", "get", "alive")
CADENCE_KEYS = {"rest_ms": (250, 60000), "hierarchy_ms": (2000, 600000), "ui_ms": (500, 60000),
                "query_ms": (1000, 600000), "host_ms": (1000, 60000)}
DEFAULT_CADENCE = {"rest_ms": 500, "hierarchy_ms": 2000, "ui_ms": 1000, "query_ms": 5000, "host_ms": 2000}
NAMESPACES = {"fixture", "peers", "assets", "run"}
LOOP_VARS = {"i", "tick"}
SAFE_ID = re.compile(r"^[A-Za-z][A-Za-z0-9_]{0,47}$")
SYNC = re.compile(r"^(barrier|seq|\+\d+(\.\d+)?)$")
WIDGET_WAIT_OPS = {"click", "type", "key", "click_until"}
WIDGET_WAIT_MAX_S = 60
EXACT_OPS = {"click", "type", "key", "click_until", "scroll_until"}  # gui ops that locate by accessible name
WHOLE_REF = re.compile(r"^\$([A-Za-z][A-Za-z0-9_.\-]*)$")
INLINE_REF = re.compile(r"\$\{([A-Za-z][A-Za-z0-9_.\-]*)\}")
CAPTURES = ("scene", "window")

SCHEMA = {
    "$schema": "https://json-schema.org/draft/2020-12/schema",
    "$id": "fe-qa-sim-scenario-v1",
    "title": "FractalEngine P2P simulation scenario",
    "type": "object",
    "required": ["name", "seed", "peers", "fixture", "steps"],
    "additionalProperties": False,
    "properties": {
        "name": {"type": "string", "pattern": "^[a-z][a-z0-9-]{0,47}$"},
        "description": {"type": "string"},
        "seed": {"type": "integer"},
        "fixture": {"enum": ["basic", "terrain"]},
        "verse_name": {"type": "string"},
        "assets": {"type": "string", "description": "asset manifest the scenario reads ($assets.*): generated|khronos|both"},
        "peers": {"type": "array", "minItems": 2, "maxItems": 8, "items": {
            "type": "object", "required": ["name", "role"], "additionalProperties": False,
            "properties": {"name": {"type": "string", "pattern": "^[a-z][a-z0-9]{0,15}$"},
                           "role": {"enum": ["owner", "editor", "viewer"]}}}},
        "probe": {"$ref": "#/$defs/cadence"},
        "watch": {"type": "array", "items": {"$ref": "#/$defs/watch"}},
        "steps": {"type": "array", "minItems": 1, "items": {"$ref": "#/$defs/step"}},
    },
    "$defs": {
        "cadence": {"type": "object", "additionalProperties": False, "properties": {
            key: {"type": "integer", "minimum": 0} for key in CADENCE_KEYS}},
        "watch": {"type": "object", "required": ["probe", "target"], "additionalProperties": False,
                  "properties": {"probe": {"enum": list(PROBES)}, "target": {}}},
        "step": {"type": "object", "required": ["id", "title", "settle_s"], "additionalProperties": False,
                 "properties": {
                     "id": {"type": "string", "pattern": "^[A-Za-z0-9][A-Za-z0-9_.-]{0,31}$"},
                     "title": {"type": "string"}, "settle_s": {"type": "number", "minimum": 0},
                     "covers": {"type": "array", "items": {"type": "string"}},
                     "note": {"type": "string"},
                     "probe": {"$ref": "#/$defs/cadence"},
                     "prepare": {"type": "array", "items": {"$ref": "#/$defs/action"}},
                     "actions": {"type": "array", "items": {"$ref": "#/$defs/action"}},
                     "expect": {"type": "array", "items": {"$ref": "#/$defs/expect"}},
                     "checkpoints": {"type": "array", "items": {"$ref": "#/$defs/checkpoint"}}}},
        "action": {"type": "object", "required": ["peer", "via", "op"], "additionalProperties": False,
                   "properties": {
                       "id": {"type": "string", "pattern": SAFE_ID.pattern}, "peer": {"type": "string"},
                       "via": {"enum": list(VIAS)}, "op": {"type": "string"}, "args": {"type": "object"},
                       "sync": {"type": "string", "pattern": SYNC.pattern},
                       "expect": {"enum": ["ok", "refused", "any"]}, "optional": {"type": "boolean"},
                       "requires": {"type": "array", "items": {"type": "string"}},
                       "needs": {"type": "array", "items": {"type": "string"}},
                       "bind": {"type": "object", "additionalProperties": {"type": "string"}},
                       "covers": {"type": "string"}, "note": {"type": "string"}}},
        "expect": {"type": "object", "required": ["probe", "target", "predicate"], "additionalProperties": False,
                   "properties": {
                       "probe": {"enum": list(PROBES)}, "target": {}, "path": {"type": "string"},
                       "peers": {"oneOf": [{"const": "all"}, {"type": "array", "items": {"type": "string"}}]},
                       "predicate": {"enum": list(PREDICATES)}, "value": {},
                       "within_s": {"type": "number", "exclusiveMinimum": 0},
                       "after": {"type": "string"}, "mode": {"enum": ["eventually", "always"]},
                       "hold_s": {"type": "number", "minimum": 0}, "covers": {"type": "string"},
                       "needs": {"type": "array", "items": {"type": "string"}},
                       "meta": {"type": "object"}, "note": {"type": "string"}}},
        "checkpoint": {"type": "object", "required": ["peer"], "additionalProperties": False,
                       "properties": {"peer": {"type": "string"}, "capture": {"enum": list(CAPTURES)},
                                      "pose": {"type": "object"}, "expect_visual": {"type": "string"}}},
    },
}


class UnresolvedRef(Exception):
    """A $-reference names a binding that does not exist, or one its producing action left null."""

    def __init__(self, ref, reason=None):
        super().__init__(ref)
        self.ref = ref
        self.reason = reason


def find(name):
    """Resolve a scenario name or path; qa/sim/scenarios (local) wins over the tracked tools/qa/sim/scenarios."""
    if os.path.isfile(name):
        return os.path.abspath(name)
    for directory in SEARCH_DIRS:
        candidate = os.path.join(directory, name if name.endswith(".json") else name + ".json")
        if os.path.isfile(candidate):
            return candidate
    raise FileNotFoundError("no scenario %r in %s" % (name, ", ".join(SEARCH_DIRS)))


def load(name):
    path = find(name)
    with open(path, encoding="utf-8") as handle:
        scenario = json.load(handle)
    errors = validate(scenario)
    if errors:
        raise ValueError("scenario %s is invalid:\n  " % path + "\n  ".join(errors))
    scenario["_path"] = path
    return scenario


def refs_in(value):
    """Every $-reference path inside a JSON value."""
    found = []
    if isinstance(value, str):
        whole = WHOLE_REF.match(value)
        if whole:
            found.append(whole.group(1))
        found.extend(INLINE_REF.findall(value))
    elif isinstance(value, list):
        for item in value:
            found.extend(refs_in(item))
    elif isinstance(value, dict):
        for item in value.values():
            found.extend(refs_in(item))
    return found


def _check_keys(obj, allowed, where, errors):
    for key in obj:
        if key not in allowed:
            errors.append("%s: unknown field %r" % (where, key))


def validate(scenario):
    """Structural + cross-reference validation; returns a list of error strings (empty = valid)."""
    errors = []
    if not isinstance(scenario, dict):
        return ["scenario must be a JSON object"]
    _check_keys({k: v for k, v in scenario.items() if not k.startswith("_")}, SCHEMA["properties"], "scenario", errors)
    for key in SCHEMA["required"]:
        if key not in scenario:
            errors.append("scenario: missing %r" % key)
    if errors:
        return errors
    if not re.match(SCHEMA["properties"]["name"]["pattern"], str(scenario["name"])):
        errors.append("name must be lowercase kebab-case")
    if not isinstance(scenario["seed"], int):
        errors.append("seed must be an integer")
    if scenario["fixture"] not in ("basic", "terrain"):
        errors.append("fixture must be basic|terrain")
    peers = scenario["peers"]
    names = [p.get("name") for p in peers if isinstance(p, dict)]
    if len(names) != len(peers) or len(set(names)) != len(names) or not names or names[0] != "owner":
        errors.append("peers: distinct names, first must be owner")
    for peer in peers:
        if isinstance(peer, dict) and peer.get("role") not in ("owner", "editor", "viewer"):
            errors.append("peer %s: role must be owner|editor|viewer" % peer.get("name"))
        if isinstance(peer, dict) and not re.match(r"^[a-z][a-z0-9]{0,15}$", str(peer.get("name"))):
            errors.append("peer %s: name must be lowercase alphanumeric" % peer.get("name"))
    _validate_cadence(scenario.get("probe", {}), "probe", errors)
    for index, watch in enumerate(scenario.get("watch", [])):
        if not isinstance(watch, dict) or watch.get("probe") not in PROBES or "target" not in watch:
            errors.append("watch[%d]: needs a known probe and a target" % index)
    defined = set()
    step_ids = set()
    for step in scenario["steps"]:
        if not isinstance(step, dict):
            errors.append("steps: every step must be an object")
            continue
        where = "step %s" % step.get("id")
        _check_keys(step, SCHEMA["$defs"]["step"]["properties"], where, errors)
        for key in ("id", "title", "settle_s"):
            if key not in step:
                errors.append("%s: missing %r" % (where, key))
        if step.get("id") in step_ids:
            errors.append("%s: duplicate step id" % where)
        step_ids.add(step.get("id"))
        if not isinstance(step.get("settle_s"), (int, float)) or step.get("settle_s", 0) < 0:
            errors.append("%s: settle_s must be a number >= 0" % where)
        _validate_cadence(step.get("probe", {}), where + " probe", errors)
        barrier_peers = []
        step_actions = set()  # ids of this step's `actions` declared so far (prepare finishes before any of them starts)
        for group in ("prepare", "actions"):
            for action in step.get(group, []):
                _validate_action(action, "%s %s" % (where, group), names, defined, errors)
                if not isinstance(action, dict):
                    continue
                if group == "actions" and action.get("sync") == "barrier":
                    barrier_peers.append(action.get("peer"))
                    needs = action.get("needs") if isinstance(action.get("needs"), list) else []
                    same_step = sorted(dep for dep in needs if isinstance(dep, str) and dep in step_actions)
                    if same_step:  # decided at the gate, which must not wait on a lane that may wait on the gate
                        errors.append("%s action %s: a barrier action may only need actions of earlier steps or prepare, not %s"
                                      % (where, action.get("id") or action.get("op"), ", ".join(same_step)))
                if group == "prepare" and action.get("sync", "seq") != "seq":
                    errors.append("%s prepare: prepare actions are sequential (no sync)" % where)
                if action.get("id"):
                    defined.add(action["id"])
                    if group == "actions":
                        step_actions.add(action["id"])
        if len(barrier_peers) != len(set(barrier_peers)):
            errors.append("%s: barrier actions need distinct peers (one firing thread per peer)" % where)
        if len(barrier_peers) == 1:
            errors.append("%s: a barrier needs at least two actions" % where)
        for index, expect in enumerate(step.get("expect", [])):
            _validate_expect(expect, "%s expect[%d]" % (where, index), names, defined, step, errors)
        for checkpoint in step.get("checkpoints", []):
            _check_keys(checkpoint, SCHEMA["$defs"]["checkpoint"]["properties"], where + " checkpoint", errors)
            if checkpoint.get("peer") not in names and checkpoint.get("peer") != "all":
                errors.append("%s checkpoint: unknown peer %r" % (where, checkpoint.get("peer")))
            if checkpoint.get("capture", "scene") not in CAPTURES:
                errors.append("%s checkpoint: capture must be scene|window" % where)
    return errors


def _validate_cadence(cadence, where, errors):
    if not isinstance(cadence, dict):
        errors.append(where + ": must be an object")
        return
    for key, value in cadence.items():
        if key not in CADENCE_KEYS:
            errors.append("%s: unknown cadence %r" % (where, key))
            continue
        low, high = CADENCE_KEYS[key]
        # 0 pauses a probe family for a step (QA-0125: GUI typing steps pause hierarchy polling).
        if not isinstance(value, int) or (value != 0 and not low <= value <= high):
            errors.append("%s: %s must be 0 or within [%d, %d] ms" % (where, key, low, high))


def _validate_action(action, where, names, defined, errors):
    if not isinstance(action, dict):
        errors.append(where + ": action must be an object")
        return
    label = "%s action %s" % (where, action.get("id") or action.get("op"))
    _check_keys(action, SCHEMA["$defs"]["action"]["properties"], label, errors)
    if action.get("peer") not in names:
        errors.append("%s: unknown peer %r" % (label, action.get("peer")))
    via = action.get("via")
    if via not in VIAS:
        errors.append("%s: via must be one of %s" % (label, sorted(VIAS)))
    elif VIAS[via] is not None and action.get("op") not in VIAS[via]:
        errors.append("%s: op %r is not a %s op %s" % (label, action.get("op"), via, sorted(VIAS[via])))
    elif via == "mcp" and not re.match(r"^[a-z][a-z_]{1,47}$", str(action.get("op"))):
        errors.append("%s: MCP op must be a tool name" % label)
    if action.get("id") is not None:
        if not SAFE_ID.match(str(action["id"])):
            errors.append("%s: id must match %s" % (label, SAFE_ID.pattern))
        if action["id"] in defined or action["id"] in NAMESPACES or action["id"] in LOOP_VARS:
            errors.append("%s: duplicate or reserved action id" % label)
    if not SYNC.match(str(action.get("sync", "seq"))):
        errors.append("%s: sync must be barrier|seq|+<seconds>" % label)
    if action.get("expect", "ok") not in ("ok", "refused", "any"):
        errors.append("%s: expect must be ok|refused|any" % label)
    if not isinstance(action.get("args", {}), dict):
        errors.append("%s: args must be an object" % label)
    if not isinstance(action.get("bind", {}), dict):
        errors.append("%s: bind must map names to result paths" % label)
    requires = action.get("requires", [])
    if not isinstance(requires, list) or any(peer not in names for peer in requires):
        errors.append("%s: requires must list scenario peer names" % label)
    elif requires and action.get("sync") == "barrier":
        errors.append("%s: requires cannot combine with sync barrier (a skipped participant would stall it)" % label)
    _validate_needs(action.get("needs", []), defined, label, errors)
    if isinstance(action.get("args"), dict) and "wait_s" in action["args"] and via == "gui":
        wait = action["args"]["wait_s"]
        if action.get("op") not in WIDGET_WAIT_OPS:
            errors.append("%s: args.wait_s applies to gui %s only" % (label, "|".join(sorted(WIDGET_WAIT_OPS))))
        elif isinstance(wait, bool) or not isinstance(wait, (int, float)) or not 0 <= wait <= WIDGET_WAIT_MAX_S:
            errors.append("%s: args.wait_s must be seconds within [0, %d]" % (label, WIDGET_WAIT_MAX_S))
    if isinstance(action.get("args"), dict) and via == "gui":
        _validate_exact(action["args"], action.get("op"), label, errors)
    for ref in refs_in(action.get("args", {})):
        root = ref.split(".")[0]
        if root not in NAMESPACES and root not in defined and root not in LOOP_VARS:
            errors.append("%s: $%s refers to no earlier action id" % (label, ref))


def _validate_exact(args, op, label, errors):
    """args.exact (bool): match whole accessible names only; click_until takes it for `until` and in args.click for the click target."""
    spots = [("args", args)] + ([("args.click", args["click"])] if op == "click_until" and isinstance(args.get("click"), dict) else [])
    for where, spot in spots:
        if "exact" not in spot:
            continue
        if op not in EXACT_OPS:
            errors.append("%s: %s.exact applies to gui %s only" % (label, where, "|".join(sorted(EXACT_OPS))))
        elif not isinstance(spot["exact"], bool):
            errors.append("%s: %s.exact must be true or false" % (label, where))


def _validate_needs(needs, defined, where, errors):
    """needs: ids of earlier actions that must have run as expected (ok for expect ok, a refusal for expect refused, any answer for expect any) WHEN the action fires or the expectation is registered, else it is skipped (not exercised)."""
    if not isinstance(needs, list) or any(not isinstance(dep, str) or dep not in defined for dep in needs):
        errors.append("%s: needs must list ids of earlier actions" % where)


def _validate_expect(expect, where, names, defined, step, errors):
    if not isinstance(expect, dict):
        errors.append(where + ": must be an object")
        return
    _check_keys(expect, SCHEMA["$defs"]["expect"]["properties"], where, errors)
    if expect.get("probe") not in PROBES:
        errors.append("%s: unknown probe %r" % (where, expect.get("probe")))
    if expect.get("predicate") not in PREDICATES:
        errors.append("%s: unknown predicate %r" % (where, expect.get("predicate")))
    if "target" not in expect:
        errors.append(where + ": missing target")
    if expect.get("predicate") in ("equals", "not_equals", "includes", "excludes", "count") and "value" not in expect:
        errors.append(where + ": predicate needs a value")
    peers = expect.get("peers", "all")
    if peers != "all" and (not isinstance(peers, list) or any(p not in names for p in peers)):
        errors.append(where + ": peers must be 'all' or a list of scenario peer names")
    step_ids = {a.get("id") for g in ("prepare", "actions") for a in step.get(g, []) if isinstance(a, dict)}
    if expect.get("after") and expect["after"] not in step_ids:
        errors.append(where + ": after must name an action id of the same step")
    if expect.get("mode", "eventually") not in ("eventually", "always"):
        errors.append(where + ": mode must be eventually|always")
    if expect.get("predicate") == "agree" and expect.get("mode") == "always":
        errors.append(where + ": agree is an eventually-only predicate")
    _validate_needs(expect.get("needs", []), defined, where, errors)
    for ref in refs_in([expect.get("target"), expect.get("value"), expect.get("path")]):
        root = ref.split(".")[0]
        if root not in NAMESPACES and root not in defined:
            errors.append("%s: $%s refers to no earlier action id" % (where, ref))


class Context:
    """Thread-safe binding store: action ids -> results, plus fixture/peers/assets/run namespaces."""

    def __init__(self, namespaces):
        self._lock = threading.Lock()
        self._values = dict(namespaces)

    def bind(self, name, value):
        with self._lock:
            self._values[name] = value

    def snapshot(self):
        with self._lock:
            return copy.deepcopy(self._values)

    def lookup(self, ref, extra=None):
        parts = ref.split(".")
        with self._lock:
            scope = dict(self._values)
        if extra:
            scope.update(extra)
        if parts[0] not in scope:
            raise UnresolvedRef(ref, "%s is not bound (its action did not run, or has not run yet)" % parts[0])
        root = value = scope[parts[0]]
        for part in parts[1:]:
            if isinstance(value, dict) and part in value:
                value = value[part]
            elif isinstance(value, list) and part.lstrip("-").isdigit() and -len(value) <= int(part) < len(value):
                value = value[int(part)]
            else:
                raise UnresolvedRef(ref, "no field %r" % part)
        if value is None:  # a failed action still binds its record: its null id must never become a target
            raise UnresolvedRef(ref, self._null_reason(parts, root))
        return value

    @staticmethod
    def _null_reason(parts, root):
        if isinstance(root, dict) and root.get("ok") is False:
            return "action %s failed: %s" % (parts[0], root.get("failure") or "refused")
        if isinstance(root, dict) and "ok" in root:
            return "action %s returned no value there" % parts[0]
        return "%s is null" % ".".join(parts)

    def resolve(self, value, extra=None):
        """Deep-substitute '$a.b' (typed) and '${a.b}' (inline text); raises UnresolvedRef."""
        if isinstance(value, str):
            whole = WHOLE_REF.match(value)
            if whole:
                return copy.deepcopy(self.lookup(whole.group(1), extra))
            return INLINE_REF.sub(lambda m: str(self.lookup(m.group(1), extra)), value)
        if isinstance(value, list):
            return [self.resolve(item, extra) for item in value]
        if isinstance(value, dict):
            return {key: self.resolve(item, extra) for key, item in value.items()}
        return value

    def try_resolve(self, value, extra=None):
        try:
            return self.resolve(value, extra)
        except UnresolvedRef:
            return None


def target_key(target):
    return target if isinstance(target, str) else json.dumps(target, sort_keys=True, separators=(",", ":"))


def source_probe(probe):
    return DERIVED.get(probe, probe)


def main(argv):
    """`validate [NAME...]` checks scenarios; `schema` regenerates scenario.schema.json."""
    if argv and argv[0] == "schema":
        out = os.path.join(HERE, "scenario.schema.json")
        with open(out, "w", encoding="utf-8") as handle:
            json.dump(SCHEMA, handle, indent=2)
            handle.write("\n")
        print(out)
        return 0
    names = argv[1:] if argv and argv[0] == "validate" else argv
    if not names:
        names = sorted({f[:-5] for d in SEARCH_DIRS if os.path.isdir(d) for f in os.listdir(d) if f.endswith(".json")})
    status = 0
    for name in names:
        try:
            scenario = load(name)
            actions = sum(len(s.get("actions", [])) + len(s.get("prepare", [])) for s in scenario["steps"])
            expects = sum(len(s.get("expect", [])) for s in scenario["steps"])
            print("OK   %s: %d steps, %d actions, %d expectations (%s)" % (
                scenario["name"], len(scenario["steps"]), actions, expects, scenario["_path"]))
        except (ValueError, FileNotFoundError) as error:
            print("FAIL %s: %s" % (name, error))
            status = 2
    return status


if __name__ == "__main__":
    import sys
    sys.exit(main(sys.argv[1:]))
