"""Strict bounded native-helper wire contracts; see AGENTS.md."""

import hashlib
import json
import re

MAX_LEDGER = 65536
MAX_REQUEST = 65536
MAX_OUTPUT = 262144
TICKS_OFFSET = 504911232000000000
LEDGER_SCHEMA = "fe-native-attempt-v1"
BOOTSTRAP_KEYS = {"schema", "kind", "run_id", "ledger_id", "pid", "birth_filetime", "exe_sha256", "helper_sha256"}
RESERVED_KEYS = {"schema", "kind", "operation", "mode", "run_id", "pid", "birth_filetime", "expected_name_sha256"}
INTENT_KEYS = {"schema", "kind", "operation", "method", "submission_possible", "pid", "birth_filetime",
               "window", "w", "c", "b", "s", "pass", "target_set", "target_epoch", "target_sha256"}


class Refused(ValueError):
    def __init__(self, code):
        self.code = code
        super().__init__(code)


def require(condition, code):
    if not condition:
        raise Refused(code)


def hex_text(value, size):
    return isinstance(value, str) and re.fullmatch(r"[0-9a-f]{%d}" % size, value) is not None


def decimal(value, maximum=(1 << 64) - 1):
    return (isinstance(value, str) and re.fullmatch(r"[1-9][0-9]{0,19}", value) is not None
            and int(value) <= maximum)


def pid(value):
    return type(value) is int and 0 < value <= 0xFFFFFFFF


def encode(value):
    return json.dumps(value, sort_keys=True, ensure_ascii=False, allow_nan=False, separators=(",", ":")).encode("utf-8")


def digest(raw):
    return hashlib.sha256(raw).hexdigest()


def _pairs(pairs):
    value = {}
    for key, item in pairs:
        require(key not in value, "duplicate_json_key")
        value[key] = item
    return value


def decode(raw, maximum):
    require(isinstance(raw, bytes) and 0 < len(raw) <= maximum, "json_size_refused")
    try:
        value = json.loads(raw.decode("utf-8"), object_pairs_hook=_pairs,
                           parse_constant=lambda _: (_ for _ in ()).throw(Refused("nonfinite_json")))
    except (UnicodeError, json.JSONDecodeError, RecursionError):
        raise Refused("invalid_json") from None
    require(type(value) is dict, "json_object_required")
    return value


def birth_filetime(identity):
    require(type(identity) is dict and set(identity) == {"pid", "start_ticks", "exe_path"}
            and pid(identity["pid"]) and decimal(identity["start_ticks"]), "process_identity_refused")
    value = int(identity["start_ticks"]) - TICKS_OFFSET
    require(0 < value <= (1 << 64) - 1, "process_time_refused")
    return str(value)


def fixture_name(value):
    require(isinstance(value, str) and value.startswith("Open petal ") and len(value) > len("Open petal "),
            "declared_fixture_name_required")
    try:
        raw = value.encode("utf-8")
        units = len(value.encode("utf-16-le")) // 2
    except UnicodeError:
        raise Refused("fixture_name_refused") from None
    require(len(raw) <= 1024 and units <= 256 and all(ord(char) >= 32 for char in value), "fixture_name_refused")
    return raw


def validate_bootstrap(record):
    require(type(record) is dict and set(record) == BOOTSTRAP_KEYS and record["schema"] == LEDGER_SCHEMA
            and record["kind"] == "bootstrap" and pid(record["pid"]) and decimal(record["birth_filetime"])
            and all(hex_text(record[key], 32) for key in ("run_id", "ledger_id"))
            and all(hex_text(record[key], 64) for key in ("exe_sha256", "helper_sha256")), "ledger_bootstrap_refused")
    return record


def read_ledger(raw, bootstrap):
    validate_bootstrap(bootstrap)
    require(isinstance(raw, bytes) and 0 < len(raw) <= MAX_LEDGER and raw.endswith(b"\n"), "ledger_partial_or_oversize")
    lines = raw.splitlines(keepends=True)
    require(1 <= len(lines) <= 3 and all(line.endswith(b"\n") and not line.endswith(b"\r\n") for line in lines),
            "ledger_record_count_refused")
    records = [decode(line[:-1], MAX_LEDGER) for line in lines]
    require(all(encode(record) + b"\n" == line for record, line in zip(records, lines)), "ledger_noncanonical")
    require(records[0] == bootstrap, "ledger_binding_changed")
    if len(records) > 1:
        reserved = records[1]
        require(set(reserved) == RESERVED_KEYS and reserved["schema"] == LEDGER_SCHEMA
                and reserved["kind"] == "reserved" and reserved["mode"] == "invoke_visible_petal_v1"
                and pid(reserved["pid"])
                and reserved["pid"] == bootstrap["pid"] and reserved["birth_filetime"] == bootstrap["birth_filetime"]
                and reserved["run_id"] == bootstrap["run_id"] and hex_text(reserved["operation"], 32)
                and hex_text(reserved["expected_name_sha256"], 64), "ledger_reservation_refused")
    if len(records) == 3:
        intent = records[2]
        require(set(intent) == INTENT_KEYS and intent["schema"] == LEDGER_SCHEMA and intent["kind"] == "intent"
                and intent["operation"] == records[1]["operation"] and intent["method"] == "Invoke"
                and intent["submission_possible"] is True and pid(intent["pid"]) and intent["pid"] == bootstrap["pid"]
                and intent["birth_filetime"] == bootstrap["birth_filetime"] and hex_text(intent["target_sha256"], 64)
                and all(decimal(intent[key]) for key in ("window", "w", "c", "b", "s", "pass", "target_set", "target_epoch")),
                "ledger_intent_refused")
    return records
