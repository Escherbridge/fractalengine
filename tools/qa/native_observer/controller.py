"""No-HTTP owned native observer entry point; see AGENTS.md."""

import os
from pathlib import Path
import re
import time
import uuid

from .contract import (MAX_LEDGER, MAX_OUTPUT, MAX_REQUEST, Refused, birth_filetime, decode, digest, encode,
                       fixture_name, hex_text, require, validate_bootstrap)
from .ledger import AttemptLedger
from .history import admit_operation
from .windows import NativeLease, canonical, run_worker

ROOT = Path(__file__).resolve().parents[3]
NATIVE_KEYS = {"schema", "run_id", "ledger_id", "ledger_path", "ledger_file_id", "observer_path",
               "observer_file_id", "process_identity", "helper_sha256"}
HELPER_KEYS = {"schema", "helper_file", "helper_sha256", "source_sha256", "sdk", "architecture", "contract"}
OUTPUT_KEYS = {"schema", "code", "attempt_spent", "invoke_entered", "method_returned", "hr",
               "root_sha256", "target_sha256", "watch_first", "watch_last"}
SIGNATURE_STYLES = ("style_u32", "exstyle_u32")
SIGNATURE_OUTER = ("outer_left_i32", "outer_top_i32", "outer_right_i32", "outer_bottom_i32")
SIGNATURE_CLIENT = ("client_left_i32", "client_top_i32", "client_right_i32", "client_bottom_i32")
SIGNATURE_KEYS = {"schema", *SIGNATURE_STYLES, *SIGNATURE_OUTER, *SIGNATURE_CLIENT}
WORKER_CODES = frozenset("""observed submitted_outcome_unknown invoke_failed_outcome_unknown submitted_state_changed
com_unavailable deadline_refused desktop_refused dpi_refused hash_unavailable input_competing integrity_refused
internal_refused ledger_binding_refused ledger_bound ledger_changed ledger_identity_refused ledger_intent_unconfirmed
ledger_invalid ledger_io ledger_noncanonical ledger_reservation_refused native_child_refused operation_deadline
output_bound output_io process_identity_refused profile_refused property_bound property_invalid property_unavailable
provider_identity_refused provider_unavailable request_bound request_io request_utf8 root_ambiguous root_identity_changed
runtime_identity_refused session_refused source_changed surface_refused target_ambiguous target_ancestry_refused
target_bound target_identity_changed target_tag_refused target_unavailable token_identity_refused tree_incomplete
watchdog_unavailable watcher_changed watcher_gap watcher_unavailable window_ambiguous window_identity_refused
window_enumeration_refused window_owned_count_refused window_class_query_refused
window_unexpected_visible_refused window_root_missing window_roots_multiple
window_service_identity_refused window_service_geometry_refused window_service_signature_refused
window_service_children_refused window_services_multiple window_set_changed
window_service_style_refused window_service_exstyle_refused
window_service_outer_rect_refused window_service_client_rect_refused
wire_invalid witness_invalid witness_stale worker_refused""".split())


def metadata(file):
    require(file.size() <= MAX_REQUEST, "metadata_size_refused")
    return decode(file.read(0, file.size()).removeprefix(b"\xef\xbb\xbf"), MAX_REQUEST)


def run_path(value):
    require(isinstance(value, str) and bool(value), "explicit_run_required")
    path = ROOT / ".qa" / "run" / value if re.fullmatch(r"\d{8}-\d{6}-\d{3}", value) else canonical(value)
    if path.name == "current.json":
        path = path.parent
    path = canonical(path)
    require(path.parent == ROOT / ".qa" / "run" and re.fullmatch(r"\d{8}-\d{6}-\d{3}", path.name), "owned_run_path_refused")
    return path


def bounded_hash(file, deadline):
    import hashlib
    size, hasher = file.size(), hashlib.sha256()
    require(0 < size <= 2 * 1024 * 1024 * 1024, "executable_size_refused")
    for offset in range(0, size, 65536):
        require(time.monotonic() < deadline, "operation_deadline")
        hasher.update(file.read(offset, min(65536, size - offset)))
    require(file.size() == size and time.monotonic() < deadline, "pinned_hash_unavailable")
    return hasher.hexdigest()


def helper_stage(files, stage, stage_manifest, deadline):
    helper = stage_manifest.get("native_observer")
    require(type(helper) is dict and set(helper) == HELPER_KEYS
            and helper["schema"] == "fe-native-helper-stage-v1" and helper["helper_file"] == "native_observer.exe"
            and helper["sdk"] == "10.0.22621.0" and helper["architecture"] == "x64"
            and helper["contract"] == "fe-native-worker-v1" and hex_text(helper["helper_sha256"], 64)
            and hex_text(helper["source_sha256"], 64), "staged_native_helper_required")
    executable = stage / "native_observer.exe"
    require(bounded_hash(files.pin(executable), deadline) == helper["helper_sha256"], "helper_hash_changed")
    return executable, helper


def pair_binding(files, directory, current, args):
    linked = current.get("native_pair")
    paired = linked is not None or (directory / "pair.json").exists()
    require(not paired or (args.pair_run and args.member), "explicit_pair_member_required")
    require(bool(args.pair_run) == bool(args.member), "explicit_pair_member_required")
    if not args.pair_run:
        return
    owner = run_path(args.pair_run)
    pair_path = owner / "pair.json"
    pair = metadata(files.pin(pair_path))
    require(pair.get("schema") == "fe-qa-pair-v1" and pair.get("distinct_run_local_identities") is True,
            "pair_record_refused")
    member = pair.get(args.member)
    require(type(member) is dict and canonical(member.get("run_dir", "")) == directory
            and member.get("process_identity") == current["process_identity"]
            and member.get("stage_manifest") == current["stage_manifest"]
            and member.get("native_observer") == current.get("native_observer")
            and canonical(member.get("stage", "")) == canonical(current["stage"]), "pair_member_changed")
    require(type(linked) is dict and set(linked) == {"pair_path", "member"}
            and canonical(linked["pair_path"]) == pair_path and linked["member"] == args.member, "pair_link_missing_or_changed")


class Context:
    def __init__(self, files, args, deadline):
        self.files, self.deadline = files, deadline
        self.directory = run_path(args.run)
        self.lifecycle = files.owned_file(ROOT / ".qa" / "lifecycle.lock", exclusive=True)
        current = metadata(files.pin(self.directory / "current.json"))
        self.current = current
        require(canonical(current.get("run_dir", "")) == self.directory and current.get("stage_matches_source") is True
                and type(current.get("qa_options")) is dict and current["qa_options"].get("synthetic_input") is False,
                "ordinary_input_off_run_required")
        identity = current.get("process_identity")
        born = birth_filetime(identity)
        stage = canonical(current.get("stage", ""))
        require(stage.parent == ROOT / ".qa" / "stage" and canonical(identity["exe_path"]) == stage / "fractalengine.exe",
                "owned_stage_identity_refused")
        manifest = metadata(files.pin(stage / "stage.json"))
        require(manifest.get("kind") == "fractalengine-qa-stage-v1" and current.get("stage_manifest") == manifest,
                "stage_provenance_changed")
        app_sha = manifest.get("exe_sha256")
        require(isinstance(app_sha, str) and hex_text(app_sha.lower(), 64), "app_hash_missing")
        require(bounded_hash(files.pin(stage / "fractalengine.exe"), deadline) == app_sha.lower(), "app_hash_changed")
        self.helper, self.helper_manifest = helper_stage(files, stage, manifest, deadline)
        pair_binding(files, self.directory, current, args)
        files.live_process(identity)
        files.same_interactive_user()
        self.identity, self.born = identity, born
        bootstrap = current.get("native_observer")
        self.ledger = None
        if bootstrap is not None:
            require(type(bootstrap) is dict and set(bootstrap) == NATIVE_KEYS
                    and bootstrap["schema"] == "fe-qa-native-bootstrap-v1"
                    and bootstrap["process_identity"] == identity and hex_text(bootstrap["run_id"], 32)
                    and hex_text(bootstrap["ledger_id"], 32) and hex_text(bootstrap["ledger_file_id"], 24)
                    and hex_text(bootstrap["observer_file_id"], 24)
                    and bootstrap["helper_sha256"] == self.helper_manifest["helper_sha256"], "native_bootstrap_changed")
            self.run_id = bootstrap["run_id"]
            self.native_dir = self.directory / "native-observer" / (born + "-" + self.run_id)
            ledger_path, observer_path = self.native_dir / "attempt.jsonl", self.native_dir / "observer.lock"
            require(canonical(bootstrap["ledger_path"]) == ledger_path and canonical(bootstrap["observer_path"]) == observer_path,
                    "native_bootstrap_path_changed")
            self.observer = files.owned_file(observer_path, exclusive=True)
            info = self.observer.identity()
            require(info["volume"] + info["file_id"] == bootstrap["observer_file_id"], "observer_lease_replaced")
            ledger = files.owned_file(ledger_path, exclusive=True)
            expected = validate_bootstrap({"schema": "fe-native-attempt-v1", "kind": "bootstrap", "run_id": self.run_id,
                "ledger_id": bootstrap["ledger_id"], "pid": identity["pid"], "birth_filetime": born,
                "exe_sha256": app_sha.lower(), "helper_sha256": self.helper_manifest["helper_sha256"]})
            self.ledger = AttemptLedger(ledger, expected, bootstrap["ledger_file_id"])
            self.ledger_file_id = bootstrap["ledger_file_id"]
            self.ledger_id = bootstrap["ledger_id"]
        else:
            require(args.operation == "observe", "bootstrap_missing_observe_only")
            self.run_id = digest(encode(identity))[:32]
            self.ledger_id, self.ledger_file_id = "", ""
            parent = self.directory / "native-observer-observe"
            self.native_dir = parent / (born + "-" + self.run_id)
            admit_operation(self.directory, self.native_dir, deadline)
            files.directory(parent, create=True)
            files.directory(self.native_dir, create=True)
            observer = self.native_dir / "observer.lock"
            self.observer = files.owned_file(observer, exclusive=True, create=not observer.exists())
        files.directory(self.native_dir / "workers", create=True)
        files.check_live()
        require(time.monotonic() < deadline, "operation_deadline")


def service_signature_refusal(sample):
    require(type(sample) is dict and set(sample) == SIGNATURE_KEYS
            and sample["schema"] == "fe-native-service-signature-v1"
            and all(type(sample[key]) is int and 0 <= sample[key] <= (1 << 32) - 1 for key in SIGNATURE_STYLES)
            and all(type(sample[key]) is int and -(1 << 31) <= sample[key] <= (1 << 31) - 1
                    for key in SIGNATURE_OUTER + SIGNATURE_CLIENT), "worker_result_refused")
    if sample["style_u32"] != 0x94000000:
        return "window_service_style_refused"
    if sample["exstyle_u32"] != 0x080800A0:
        return "window_service_exstyle_refused"
    if any(sample[key] != 0 for key in SIGNATURE_OUTER):
        return "window_service_outer_rect_refused"
    if any(sample[key] != 0 for key in SIGNATURE_CLIENT):
        return "window_service_client_rect_refused"
    return None


def worker_output(raw, *, observe, helper_exit_code):
    value = decode(raw, MAX_OUTPUT)
    require(set(value) in (OUTPUT_KEYS, OUTPUT_KEYS | {"service_style_u32"}, OUTPUT_KEYS | {"service_signature"})
            and value["schema"] == "fe-native-worker-result-v1"
            and value["code"] in WORKER_CODES
            and all(type(value[key]) is bool for key in ("attempt_spent", "invoke_entered", "method_returned"))
            and isinstance(value["hr"], str) and re.fullmatch(r"[0-9a-f]{8}", value["hr"])
            and all(value[key] == "" or hex_text(value[key], 64) for key in ("root_sha256", "target_sha256"))
            and all(type(value[key]) is int and 0 <= value[key] <= (1 << 64) - 1 for key in ("watch_first", "watch_last"))
            and value["watch_first"] <= value["watch_last"], "worker_result_refused")
    require(not value["method_returned"] or value["invoke_entered"], "worker_result_refused")
    if "service_style_u32" in value:
        require(type(value["service_style_u32"]) is int and 0 <= value["service_style_u32"] <= (1 << 32) - 1
                and observe is True and type(helper_exit_code) is int and helper_exit_code == 2
                and value["code"] == "window_service_style_refused" and value["hr"] == "00000000"
                and not value["attempt_spent"] and not value["invoke_entered"] and not value["method_returned"],
                "worker_result_refused")
    if "service_signature" in value:
        require(observe is True and type(helper_exit_code) is int and helper_exit_code == 2
                and value["hr"] == "00000000" and not value["attempt_spent"]
                and not value["invoke_entered"] and not value["method_returned"]
                and service_signature_refusal(value["service_signature"]) == value["code"], "worker_result_refused")
    return value


def execute(args):
    deadline = time.monotonic() + 5.0
    require(args.run and args.operation in ("observe", "invoke-petal"), "explicit_native_operation_required")
    require(not getattr(args, "verse", None), "native_has_no_http_verse_selector")
    invoke = args.operation == "invoke-petal"
    require(not invoke or args.controlled_fixture, "controlled_fixture_declaration_required")
    require(invoke or not args.name, "observe_has_no_target")
    if invoke:
        fixture_name(args.name)
    operation = uuid.uuid4().hex
    with NativeLease() as files:
        context = Context(files, args, deadline)
        admit_operation(context.directory, context.native_dir, deadline)
        reserved_sha = "0" * 64
        if invoke:
            require(context.ledger is not None, "bootstrap_missing_observe_only")
            reserved_sha = context.ledger.reserve(operation, args.name)
        record = files.owned_file(context.native_dir / "workers" / (operation + ".jsonl"), create=True, receipt=True)
        parent = files.process_identity(files.api.GetCurrentProcess())
        prefix = {"schema": "fe-native-worker-owner-v1", "phase": "reserved", "operation": operation,
                  "controller_identity": parent, "app_identity": context.identity, "helper_path": str(context.helper),
                  "helper_sha256": context.helper_manifest["helper_sha256"], "run_id": context.run_id,
                  "ledger_id": context.ledger_id, "ledger_file_id": context.ledger_file_id,
                  "mode": "invoke_visible_petal_v1" if invoke else "observe"}
        raw = encode(prefix) + b"\n"
        record.append_durable(raw, 0, MAX_LEDGER)
        state = {"identity": None}

        def started(identity):
            state["identity"] = identity
            entry = {"schema": "fe-native-worker-owner-v1", "phase": "worker_started", "operation": operation,
                     "worker_identity": identity}
            record.append_durable(encode(entry) + b"\n", record.size(), MAX_LEDGER)

        remaining = max(0, int((deadline - time.monotonic()) * 1000))
        require(0 < remaining <= 5000, "operation_deadline")
        payload = encode({"schema": "fe-native-worker-v1", "mode": prefix["mode"], "pid": context.identity["pid"],
            "birth_filetime": context.born, "process_handle": str(files.processes[0]),
            "ledger_handle": str(context.ledger.file.handle) if invoke else "0",
            "ledger_file_id": context.ledger_file_id if invoke else "0" * 24,
            "operation": operation if invoke else "0" * 32, "reservation_sha256": reserved_sha,
            "expected_name": args.name if invoke else "", "exe_path": context.identity["exe_path"],
            "exe_sha256": context.current["stage_manifest"]["exe_sha256"].lower(), "run_id": context.run_id,
            "deadline_tick": str(files.api.GetTickCount64() + remaining)})
        require(len(payload) <= MAX_REQUEST, "request_overflow")
        try:
            result = run_worker(files, context.helper, payload, deadline, started)
        except Refused as error:
            terminal = {"schema": "fe-native-worker-owner-v1", "phase": "lifetime_unknown", "operation": operation,
                        "worker_identity": state["identity"], "code": error.code}
            record.append_durable(encode(terminal) + b"\n", record.size(), MAX_LEDGER)
            raise
        terminal = {"schema": "fe-native-worker-owner-v1", "phase": "exit_confirmed", "operation": operation,
                    "worker_identity": result.identity, "exit_code": result.exit_code, "code": result.code}
        record.append_durable(encode(terminal) + b"\n", record.size(), MAX_LEDGER)
        spent = context.ledger.spent() if context.ledger is not None else None
        require(result.code == "worker_exited", result.code)
        value = worker_output(result.raw, observe=not invoke, helper_exit_code=result.exit_code)
        files.check_live()
        require(not invoke or spent, "durable_attempt_unconfirmed")
        require(invoke or not value["invoke_entered"] and not value["attempt_spent"], "observe_invocation_refused")
        value["attempt_spent"] = spent
        value["outcome"] = (
            "submitted_outcome_unknown" if value["code"] in {"submitted_outcome_unknown", "submitted_state_changed"}
            else "invocation_outcome_unknown" if value["invoke_entered"]
            else "refused_attempt_spent" if invoke else "observation")
        value["worker_record"] = str((context.native_dir / "workers" / (operation + ".jsonl")).relative_to(context.directory))
        value["capture_available"] = False
        value["helper_exit_code"] = result.exit_code
        return value


def entry(args):
    try:
        value = execute(args)
        return value, 0 if value["helper_exit_code"] == 0 else 2
    except Refused as error:
        return {"schema": "fe-native-controller-result-v1", "code": error.code, "capture_available": False,
                "attempt_spent": None, "outcome": "unavailable_or_unknown"}, 2
    except (OSError, ValueError, TypeError, KeyError, OverflowError):
        return {"schema": "fe-native-controller-result-v1", "code": "native_owned_boundary_unavailable",
                "capture_available": False, "attempt_spent": None, "outcome": "unavailable_or_unknown"}, 2
