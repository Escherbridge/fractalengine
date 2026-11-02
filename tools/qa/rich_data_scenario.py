"""Validate rich-data scenario intent; see tools/qa/AGENTS.md §rich-data."""

import datetime
import json
import math
from pathlib import Path
import re


MAX_SCENARIO_BYTES = 1024 * 1024
MAX_STEPS = 128
NAME = re.compile(r"[A-Za-z][A-Za-z0-9_-]{0,63}\Z")
ULID = re.compile(r"[0-7][0-9A-HJKMNP-TV-Z]{25}\Z")
OP_FIELDS = {
    "upload_asset": {"asset", "path"},
    "install_schema": {"schema"},
    "reject_schema": {"schema", "reason_contains"},
    "create_object": {"object", "asset", "expect_version"},
    "set_property": {"object", "key", "value", "expect_version"},
    "delete_property": {"object", "key", "expect_version"},
    "transform": {"object", "position", "rotation", "scale", "expect_version"},
    "append_readings": {"metric", "units", "recorded_at", "values"},
    "derive_metric": {"source_metric", "target_metric", "units", "start_ms", "end_ms", "calibration_versions"},
    "query_metrics": {"metric", "start_ms", "end_ms", "oracle", "kind", "mode", "mcp_parity"},
    "checkpoint": {"expect"},
    "state_at": {"object", "version", "expect", "mcp_parity"},
    "history": {"object", "after_version", "limit", "expect_versions", "mcp_parity"},
    "reject_property": {"object", "key", "value", "reason_contains"},
    "distances": {"expected_m"},
}
OPTIONAL_FIELDS = {"asset", "reason_contains", "mcp_parity"}


class ScenarioError(ValueError):
    """A bounded intent, response or assertion failure."""

    def __init__(self, code, message):
        super().__init__(message)
        self.code = code


def require(condition, message, code="invalid_scenario"):
    if not condition:
        raise ScenarioError(code, message)


def unique_object(pairs):
    result = {}
    for key, value in pairs:
        require(key not in result, "duplicate JSON member: " + key)
        result[key] = value
    return result


def reject_constant(value):
    raise ScenarioError("invalid_json", "nonfinite JSON number: " + value)


def parse_json(text):
    return json.loads(text, object_pairs_hook=unique_object, parse_constant=reject_constant)


def finite_tree(value, depth=0):
    require(depth <= 20, "JSON exceeds depth 20")
    if isinstance(value, float):
        require(math.isfinite(value), "nonfinite number")
    elif isinstance(value, dict):
        for key, child in value.items():
            require(isinstance(key, str) and len(key) <= 256, "invalid JSON key")
            finite_tree(child, depth + 1)
    elif isinstance(value, list):
        require(len(value) <= 4096, "JSON array exceeds 4096 items")
        for child in value:
            finite_tree(child, depth + 1)
    elif isinstance(value, str):
        require(len(value) <= 65536, "JSON string exceeds 65536 characters")


def integer(value, low, high, label):
    require(type(value) is int and low <= value <= high, label + " is outside its integer bounds")


def vector(value, label):
    require(isinstance(value, list) and len(value) == 3, label + " needs XYZ")
    require(all(type(x) in (int, float) and math.isfinite(x) and abs(x) <= 1e7 for x in value),
            label + " needs three finite numbers within ±1e7")


def epoch_ms(text):
    require(isinstance(text, str) and len(text) <= 40, "recorded_at must be RFC3339")
    try:
        value = datetime.datetime.fromisoformat(text.replace("Z", "+00:00"))
    except ValueError as error:
        raise ScenarioError("invalid_scenario", "invalid recorded_at") from error
    require(value.tzinfo is not None and value.utcoffset() == datetime.timedelta(0), "recorded_at must be UTC")
    milliseconds = (value - datetime.datetime(1970, 1, 1, tzinfo=datetime.timezone.utc)).total_seconds() * 1000
    require(milliseconds.is_integer(), "recorded_at must resolve to integral milliseconds")
    return int(milliseconds)


def equal_json(actual, expected, tolerance=1e-5, subset=False):
    if type(expected) in (int, float):
        return type(actual) in (int, float) and math.isfinite(actual) and math.isclose(actual, expected, rel_tol=0, abs_tol=tolerance)
    if type(actual) is not type(expected):
        return False
    if isinstance(expected, dict):
        return (subset or actual.keys() == expected.keys()) and all(
            key in actual and equal_json(actual[key], value, tolerance, subset) for key, value in expected.items())
    if isinstance(expected, list):
        return len(actual) == len(expected) and all(equal_json(a, b, tolerance, subset) for a, b in zip(actual, expected))
    return actual == expected


def expected_state(value):
    require(isinstance(value, dict) and value and not (set(value) - {"position", "rotation", "scale", "properties", "display_name"}),
            "invalid expected object state")
    for field in ("position", "rotation", "scale"):
        if field in value:
            vector(value[field], field)
    if "properties" in value:
        require(isinstance(value["properties"], dict), "expected properties must be an object")


def load_scenario(path):
    raw = Path(path).read_bytes()
    require(len(raw) <= MAX_SCENARIO_BYTES, "scenario exceeds 1 MiB")
    spec = parse_json(raw.decode("utf-8-sig"))
    require(isinstance(spec, dict) and spec.get("format") == "fe-rich-data-v1", "unsupported scenario format")
    require(set(spec) == {"format", "id", "description", "step_timeout_seconds", "objects", "schemas", "oracles", "steps"},
            "unknown or missing scenario fields")
    require(isinstance(spec["id"], str) and NAME.fullmatch(spec["id"]), "invalid scenario ID")
    require(type(spec["step_timeout_seconds"]) in (int, float) and 1 <= spec["step_timeout_seconds"] <= 60,
            "step timeout must be 1..60 seconds")
    require(isinstance(spec["description"], str) and len(spec["description"]) <= 2048, "invalid description")
    for key, cap in (("objects", 16), ("schemas", 16), ("oracles", 32)):
        require(isinstance(spec[key], dict) and 0 < len(spec[key]) <= cap, "invalid " + key)
        require(all(NAME.fullmatch(name) for name in spec[key]), "invalid logical name in " + key)
    for name, body in spec["objects"].items():
        require(isinstance(body, dict) and set(body) == {"name", "position", "rotation", "scale", "properties"}, "invalid object " + name)
        require(isinstance(body["name"], str) and 0 < len(body["name"]) <= 96, "invalid display name")
        require(isinstance(body["properties"], dict), "object properties must be an object")
        for field in ("position", "rotation", "scale"):
            vector(body[field], field)
        require(all(value > 0 for value in body["scale"]), "scale must be positive")
    for schema in spec["schemas"].values():
        require(isinstance(schema, dict) and set(schema) == {"key", "value_type", "descriptor"}, "invalid schema intent")
        require(isinstance(schema["key"], str) and 0 < len(schema["key"]) <= 128, "invalid outer property key")
        require(schema["value_type"] in {"object", "array", "string", "number", "bool"}, "invalid schema value type")
        descriptor = schema["descriptor"]
        require(isinstance(descriptor, dict) and set(descriptor) == {"version", "schema"}, "invalid descriptor envelope")
        integer(descriptor["version"], 1, 2**31-1, "schema version")
        require(isinstance(descriptor["schema"], dict), "schema rule must be an object")
        require(len(json.dumps(descriptor, ensure_ascii=False).encode("utf-8")) <= 65536, "schema exceeds 64 KiB")
    for oracle in spec["oracles"].values():
        require(isinstance(oracle, dict) and oracle and set(oracle).issubset(spec["objects"]), "invalid oracle object references")
        for values in oracle.values():
            require(isinstance(values, dict) and set(values) == {"times_ms", "values", "stats"}, "invalid metric oracle")
            require(isinstance(values["times_ms"], list) and isinstance(values["values"], list)
                    and 0 < len(values["values"]) == len(values["times_ms"]) <= 512, "invalid oracle arrays")
            require(all(type(t) is int for t in values["times_ms"])
                    and values["times_ms"] == sorted(set(values["times_ms"])), "oracle event times must be unique and ordered")
            require(all(type(v) in (int, float) for v in values["values"]), "oracle values must be numeric")
            stats = values["stats"]
            require(isinstance(stats, dict) and set(stats) == {"count", "min", "max", "mean", "sum"}, "invalid oracle statistics")
            integer(stats["count"], 1, 512, "oracle count")
            require(all(type(stats[k]) in (int, float) for k in ("min", "max", "mean", "sum")), "invalid oracle statistic value")
    finite_tree(spec)
    steps = spec["steps"]
    require(isinstance(steps, list) and 0 < len(steps) <= MAX_STEPS, "scenario needs 1..128 steps")
    seen, created, assets, revisions = set(), set(), set(), {}
    for step in steps:
        require(isinstance(step, dict) and isinstance(step.get("id"), str) and NAME.fullmatch(step["id"]), "invalid step ID")
        require(step["id"] not in seen, "duplicate step ID: " + step["id"])
        seen.add(step["id"])
        op = step.get("op")
        require(op in OP_FIELDS, "unsupported operation: " + str(op))
        allowed = OP_FIELDS[op]
        require(not (set(step) - allowed - {"id", "op"}), "unknown step field in " + step["id"])
        require(not ((allowed - OPTIONAL_FIELDS) - set(step)), "missing step field in " + step["id"])
        if "object" in step:
            name = step["object"]
            require(name in spec["objects"], "unknown object reference")
            if op == "create_object":
                require(name not in created, "object is created twice")
                created.add(name)
            else:
                require(name in created, "object used before creation")
        if "schema" in step:
            require(step["schema"] in spec["schemas"], "unknown schema reference")
            schema = spec["schemas"][step["schema"]]
            if op == "install_schema":
                require(schema["descriptor"]["version"] == revisions.get(schema["key"], 0) + 1, "schema revisions must be consecutive")
                revisions[schema["key"]] = schema["descriptor"]["version"]
            else:
                require(schema["key"] in revisions, "rejected evolution needs an installed schema")
        if op == "upload_asset":
            require(NAME.fullmatch(step["asset"]) and step["asset"] not in assets, "duplicate/invalid asset name")
            require(isinstance(step["path"], str) and len(step["path"]) <= 512, "invalid asset path")
            assets.add(step["asset"])
        elif "asset" in step:
            require(step["asset"] in assets, "asset used before upload")
        for field in ("expect_version", "version"):
            if field in step:
                integer(step[field], 1, 2**63-1, field)
        if op == "transform":
            for field in ("position", "rotation", "scale"):
                vector(step[field], field)
            require(all(value > 0 for value in step["scale"]), "scale must be positive")
        if "recorded_at" in step:
            epoch_ms(step["recorded_at"])
        for field in ("start_ms", "end_ms"):
            if field in step:
                integer(step[field], -(2**63), 2**63-1, field)
        if "start_ms" in step:
            require(step["start_ms"] < step["end_ms"] and step["end_ms"] - step["start_ms"] <= 31*86400000,
                    "query window must be positive and <=31 days")
        for field in ("metric", "source_metric", "target_metric"):
            if field in step:
                require(isinstance(step[field], str) and NAME.fullmatch(step[field]), "invalid metric")
        if "key" in step:
            require(isinstance(step["key"], str) and 0 < len(step["key"]) <= 128, "invalid literal outer property key")
        if "units" in step:
            require(isinstance(step["units"], str) and len(step["units"]) <= 32, "invalid units")
        if "reason_contains" in step:
            require(isinstance(step["reason_contains"], str) and 0 < len(step["reason_contains"]) <= 128, "invalid expected refusal text")
        if op in ("append_readings", "derive_metric", "checkpoint", "distances"):
            field = {"append_readings":"values", "derive_metric":"calibration_versions", "checkpoint":"expect", "distances":"expected_m"}[op]
            require(isinstance(step[field], dict) and step[field] and set(step[field]).issubset(created), "unknown object in " + field)
        if op == "append_readings":
            require(all(type(value) in (int, float) for value in step["values"].values()), "readings must be numbers")
        if op == "derive_metric":
            for version in step["calibration_versions"].values():
                integer(version, 1, 2**63-1, "calibration history version")
        if op == "query_metrics":
            require(step["kind"] in {"rows", "aggregate", "latest"} and step["mode"] in {"local", "distributed"}, "invalid query mode")
            require(step["oracle"] in spec["oracles"], "unknown oracle")
            require(set(spec["oracles"][step["oracle"]]).issubset(created), "oracle references an uncreated object")
        if op == "checkpoint":
            for value in step["expect"].values():
                expected_state(value)
        if op == "state_at":
            expected_state(step["expect"])
        if op == "distances":
            require(all(type(value) in (int, float) and 0 <= value <= 1e9 for value in step["expected_m"].values()), "invalid distance oracle")
        if op == "history":
            integer(step["after_version"], 0, 2**63-1, "history cursor")
            integer(step["limit"], 1, 128, "history page size")
            require(isinstance(step["expect_versions"], list) and len(step["expect_versions"]) <= 512, "invalid history oracle")
            require(all(type(v) is int and step["after_version"] < v <= 2**63-1 for v in step["expect_versions"])
                    and step["expect_versions"] == sorted(set(step["expect_versions"])), "invalid expected history order")
        if "mcp_parity" in step:
            require(type(step["mcp_parity"]) is bool, "mcp_parity must be boolean")
    return spec
