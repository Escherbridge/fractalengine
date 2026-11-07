"""Expectation predicates and probe derivations shared by the driver's live wait and the analyzer; see tools/qa/AGENTS.md §sim."""
import hashlib
import json
import math

PREDICATES = ("equals", "not_equals", "includes", "excludes", "absent", "present", "count", "agree")
# Derived probes read a field of a polled source probe instead of issuing their own request.
DERIVED = {"properties": "node"}
TOLERANCE = 1e-3


def canon(value):
    return json.dumps(value, sort_keys=True, separators=(",", ":"), default=str)


def digest(value):
    return hashlib.sha256(canon(normalize(value)).encode("utf-8")).hexdigest()[:16]


def normalize(value):
    """Collapse -0.0 to 0.0 and round floats so digests compare values, not float noise."""
    if isinstance(value, float):
        if not math.isfinite(value):
            return str(value)
        rounded = round(value, 4)
        return 0.0 if rounded == 0 else rounded
    if isinstance(value, list):
        return [normalize(item) for item in value]
    if isinstance(value, dict):
        return {key: normalize(item) for key, item in value.items()}
    return value


def is_absent_marker(value):
    return value is None or (isinstance(value, dict) and value.get("absent") is True)


def derive(probe, value):
    """Map a source observation onto a derived probe's value (absent markers pass through)."""
    if probe not in DERIVED or is_absent_marker(value):
        return value
    if probe == "properties":
        return value.get("properties") or {}
    return value


def split_path(path):
    """Split a dotted path; a backslash-escaped dot stays inside one key (the 'view.groups' property)."""
    parts, current, text, index = [], "", str(path), 0
    while index < len(text):
        if text[index] == "\\" and index + 1 < len(text) and text[index + 1] == ".":
            current += "."
            index += 2
            continue
        if text[index] == ".":
            parts.append(current)
            current = ""
        else:
            current += text[index]
        index += 1
    parts.append(current)
    return parts


def at_path(value, path):
    """(found, inner) for a dotted path; numeric segments index lists."""
    if not path:
        return (not is_absent_marker(value)), value
    current = value
    for segment in split_path(path):
        if isinstance(current, dict) and segment in current:
            current = current[segment]
        elif isinstance(current, list) and segment.lstrip("-").isdigit() and -len(current) <= int(segment) < len(current):
            current = current[int(segment)]
        else:
            return False, None
    return current is not None, current


def approx_equal(left, right):
    if isinstance(left, bool) or isinstance(right, bool):
        return left is right
    if isinstance(left, (int, float)) and isinstance(right, (int, float)):
        return math.isfinite(left) and math.isfinite(right) and abs(left - right) <= TOLERANCE
    if isinstance(left, list) and isinstance(right, list):
        return len(left) == len(right) and all(approx_equal(a, b) for a, b in zip(left, right))
    if isinstance(left, dict) and isinstance(right, dict):
        return left.keys() == right.keys() and all(approx_equal(left[k], right[k]) for k in left)
    return left == right


def contains(container, item):
    if isinstance(container, str):
        return isinstance(item, str) and item in container
    if isinstance(container, dict):
        if isinstance(item, dict):
            return all(key in container and approx_equal(container[key], val) for key, val in item.items())
        return item in container
    if isinstance(container, list):
        if isinstance(item, list):
            return all(contains(container, element) for element in item)
        if isinstance(item, dict):
            return any(isinstance(element, dict) and contains(element, item) for element in container)
        return any(approx_equal(element, item) for element in container)
    return False


def count_matches(found, inner, expected):
    size = len(inner) if isinstance(inner, (list, dict, str)) else (inner if isinstance(inner, int) else None)
    if not found or size is None:
        return False
    if isinstance(expected, dict):
        return expected.get("min", -math.inf) <= size <= expected.get("max", math.inf)
    return size == expected


def evaluate(predicate, value, expected=None, path=None):
    """True when one peer's observed value satisfies the predicate ('agree' is cross-peer, see agree())."""
    found, inner = at_path(value, path)
    if predicate == "absent":
        return not found
    if predicate == "present":
        return found
    if predicate == "equals":
        return found and approx_equal(normalize(inner), normalize(expected))
    if predicate == "not_equals":
        return not (found and approx_equal(normalize(inner), normalize(expected)))
    if predicate == "includes":
        return found and contains(inner, expected)
    if predicate == "excludes":
        return not (found and contains(inner, expected))
    if predicate == "count":
        return count_matches(found, inner, expected)
    raise ValueError("unknown predicate " + str(predicate))


def agree(values_by_peer, path=None):
    """True when every listed peer has an observation and all selected values are digest-equal."""
    if not values_by_peer or any(v is _MISSING for v in values_by_peer.values()):
        return False
    digests = {digest(at_path(value, path)[1]) for value in values_by_peer.values()}
    return len(digests) == 1


_MISSING = object()
MISSING = _MISSING
