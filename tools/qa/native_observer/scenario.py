"""Offline cases for the existing evidence-boundary journey; see AGENTS.md."""

import contextlib
import copy
import io
import json
import os
from pathlib import Path
import subprocess
import tempfile
import time
from unittest.mock import patch

from .contract import Refused, digest, encode, read_ledger
from .ledger import AttemptLedger
from .history import admit, admit_operation, inventory, limits


class MemoryFile:
    def __init__(self, raw, partial=False):
        self.raw, self.partial, self.appends = raw, partial, 0

    def identity(self):
        return {"volume": "a" * 8, "file_id": "b" * 16}

    def size(self):
        return len(self.raw)

    def read(self, offset, length):
        return self.raw[offset:offset + length]

    def append_durable(self, raw, expected, maximum):
        assert expected == len(self.raw) and expected + len(raw) <= maximum
        self.appends += 1
        self.raw += raw[:len(raw) // 2] if self.partial else raw
        if self.partial:
            raise OSError("controlled incomplete durable append")


def exercise():
    bootstrap = {"schema": "fe-native-attempt-v1", "kind": "bootstrap", "run_id": "1" * 32,
                 "ledger_id": "2" * 32, "pid": 1234, "birth_filetime": "123456789",
                 "exe_sha256": "a" * 64, "helper_sha256": "c" * 64}
    raw = encode(bootstrap) + b"\n"
    file_id = "a" * 8 + "b" * 16
    results = []

    def record(name, passed):
        results.append({"case": "native-contract-" + name, "expected_accepted": True, "accepted": passed,
                        "passed": passed, "boundary": "offline production ledger/CLI contracts; no native, DB or application coverage"})

    file = MemoryFile(raw)
    ledger = AttemptLedger(file, bootstrap, file_id)
    reservation = ledger.reserve("3" * 32, "Open petal Controlled fixture")
    record("durable-reservation-before-worker", reservation == digest(file.raw) and len(ledger.read()) == 2 and file.appends == 1)
    for outcome in ("s_ok", "hresult", "worker_timeout", "controller_crash", "pipe_loss", "post_arm_refusal", "host_expired"):
        refused = False
        try:
            ledger.reserve("4" * 32, "Open petal Controlled fixture")
        except Refused:
            refused = True
        record("spent-after-" + outcome, refused and ledger.spent() and file.appends == 1)
    intent = {"schema": "fe-native-attempt-v1", "kind": "intent", "operation": "3" * 32, "method": "Invoke",
              "submission_possible": True, "pid": bootstrap["pid"], "birth_filetime": bootstrap["birth_filetime"],
              "window": "1", "w": "2", "c": "3", "b": "4", "s": "5", "pass": "6", "target_set": "7",
              "target_epoch": "8", "target_sha256": "d" * 64}
    file.append_durable(encode(intent) + b"\n", file.size(), 65536)
    record("canonical-intent-remains-spent", len(ledger.read()) == 3 and ledger.spent())
    for name, corrupted in (("missing", b""), ("partial", raw[:-1]), ("bom", b"\xef\xbb\xbf" + raw),
                            ("duplicate-key", raw.replace(b'"pid":1234', b'"pid":1234,"pid":1234')),
                            ("foreign-binding", raw.replace(b'"pid":1234', b'"pid":1235')),
                            ("noncanonical", raw.replace(b'"pid":1234', b'"pid": 1234')),
                            ("oversize", b"x" * 65537)):
        refused = False
        try:
            AttemptLedger(MemoryFile(corrupted), bootstrap, file_id)
        except Refused:
            refused = True
        record(name + "-never-pristine", refused)
    refused = False
    try:
        AttemptLedger(MemoryFile(raw), bootstrap, "0" * 24)
    except Refused:
        refused = True
    record("replaced-file-never-pristine", refused)
    incomplete = MemoryFile(raw, partial=True)
    try:
        AttemptLedger(incomplete, bootstrap, file_id).reserve("3" * 32, "Open petal Controlled fixture")
    except OSError:
        pass
    refused = False
    try:
        AttemptLedger(incomplete, bootstrap, file_id)
    except Refused:
        refused = True
    record("interrupted-reservation-not-reset", refused and incomplete.appends == 1)
    for field, value in (("submission_possible", False), ("method", "Focus"), ("operation", "4" * 32)):
        changed = copy.deepcopy(intent)
        changed[field] = value
        refused = False
        try:
            read_ledger(raw + encode(ledger.records[1]) + b"\n" + encode(changed) + b"\n", bootstrap)
        except Refused:
            refused = True
        record("intent-" + field + "-refused", refused)

    import fe
    from . import controller
    style_result = {"schema": "fe-native-worker-result-v1", "code": "window_service_style_refused",
                    "attempt_spent": False, "invoke_entered": False, "method_returned": False, "hr": "00000000",
                    "root_sha256": "0" * 64, "target_sha256": "0" * 64, "watch_first": 0, "watch_last": 0}
    record("legacy-style-refusal-without-value", controller.worker_output(
        encode(style_result), observe=True, helper_exit_code=2) == style_result)
    for value in (0, (1 << 32) - 1, 0x94000000):
        sample = dict(style_result, service_style_u32=value)
        parsed = controller.worker_output(encode(sample), observe=True, helper_exit_code=2)
        record("style-u32-" + str(value), parsed == sample)
    record("observe-diagnostic-does-not-reset-spent-ledger", ledger.spent() and not parsed["attempt_spent"])
    invalid_style_cases = []
    for value in (True, False, 1.0, "1", None, -1, 1 << 32):
        invalid_style_cases.append(("type-range-" + repr(value), dict(style_result, service_style_u32=value), True, 2))
    sample = dict(style_result, service_style_u32=0)
    for key, value in (("code", "observed"), ("code", "window_service_exstyle_refused"), ("hr", "80004005"),
                       ("attempt_spent", True), ("invoke_entered", True), ("method_returned", True), ("extra", 0)):
        changed = dict(sample)
        changed[key] = value
        invalid_style_cases.append(("wrong-" + key + "-" + str(value), changed, True, 2))
    invalid_style_cases.append(("invoke-mode", sample, False, 2))
    for exit_code in (0, 3, True):
        invalid_style_cases.append(("exit-" + str(exit_code), sample, True, exit_code))
    for name, changed, observe, exit_code in invalid_style_cases:
        refused = False
        try:
            controller.worker_output(encode(changed), observe=observe, helper_exit_code=exit_code)
        except Refused:
            refused = True
        record("style-diagnostic-" + name + "-refused", refused)
    refused = False
    try:
        duplicate = encode(sample)[:-1] + b',"service_style_u32":1}'
        controller.worker_output(duplicate, observe=True, helper_exit_code=2)
    except Refused:
        refused = True
    record("style-diagnostic-duplicate-key-refused", refused)

    signature = {key: 0 for key in controller.SIGNATURE_OUTER + controller.SIGNATURE_CLIENT}
    signature.update(schema="fe-native-service-signature-v1", style_u32=0x94000000, exstyle_u32=0x080800A0)
    signature_cases = [
        ("style", "style_u32", 0, "window_service_style_refused"),
        ("exstyle", "exstyle_u32", 0, "window_service_exstyle_refused"),
        ("outer", "outer_left_i32", 1, "window_service_outer_rect_refused"),
        ("client", "client_left_i32", -1, "window_service_client_rect_refused"),
    ]
    for key in controller.SIGNATURE_STYLES:
        for value in (0, (1 << 32) - 1):
            code = "window_service_style_refused" if key == "style_u32" else "window_service_exstyle_refused"
            signature_cases.append((key + "-" + str(value), key, value, code))
    for key in controller.SIGNATURE_OUTER + controller.SIGNATURE_CLIENT:
        for value in (-(1 << 31), (1 << 31) - 1):
            code = ("window_service_outer_rect_refused" if key in controller.SIGNATURE_OUTER
                    else "window_service_client_rect_refused")
            signature_cases.append((key + "-" + str(value), key, value, code))
    signature_v2 = dict(signature, schema="fe-native-service-signature-v2", outer_right_i32=16, outer_bottom_i32=16)
    for version, baseline in (("v1", signature), ("v2", signature_v2)):
        for name, key, value, code in signature_cases:
            changed = dict(baseline)
            changed[key] = value
            sample = dict(style_result, code=code, service_signature=changed)
            parsed = controller.worker_output(encode(sample), observe=True, helper_exit_code=2)
            record("signature-" + version + "-" + name, parsed == sample and ledger.spent() and not parsed["attempt_spent"])
    for right, bottom in ((0, 0), (16, 16)):
        for version, baseline in (("v1", signature), ("v2", signature_v2)):
            changed = dict(baseline, outer_right_i32=right, outer_bottom_i32=bottom, client_left_i32=1)
            expected_outer = (0, 0) if version == "v1" else (16, 16)
            code = ("window_service_client_rect_refused" if (right, bottom) == expected_outer
                    else "window_service_outer_rect_refused")
            sample = dict(style_result, code=code, service_signature=changed)
            record("signature-" + version + "-same-facts-" + str(right), controller.worker_output(
                encode(sample), observe=True, helper_exit_code=2) == sample)
    for key, value in (("outer_left_i32", -1), ("outer_left_i32", 1), ("outer_top_i32", -1), ("outer_top_i32", 1),
                       ("outer_right_i32", 15), ("outer_right_i32", 17), ("outer_bottom_i32", 15), ("outer_bottom_i32", 17)):
        changed = dict(signature_v2)
        changed[key] = value
        sample = dict(style_result, code="window_service_outer_rect_refused", service_signature=changed)
        record("signature-v2-exact-outer-" + key + "-" + str(value), controller.worker_output(
            encode(sample), observe=True, helper_exit_code=2) == sample)
    mixed = dict(style_result, service_signature=dict(signature, style_u32=0, exstyle_u32=0,
                                                      outer_left_i32=1, client_left_i32=1))
    record("signature-first-failure-priority", controller.worker_output(
        encode(mixed), observe=True, helper_exit_code=2) == mixed)

    sample = dict(style_result, code="window_service_outer_rect_refused",
                  service_signature=dict(signature, outer_left_i32=1))
    invalid_signatures = []
    for key in sorted(controller.SIGNATURE_KEYS):
        changed = copy.deepcopy(sample)
        del changed["service_signature"][key]
        invalid_signatures.append(("missing-" + key, changed, True, 2))
    for key in controller.SIGNATURE_STYLES + controller.SIGNATURE_OUTER + controller.SIGNATURE_CLIENT:
        lower, upper = (0, (1 << 32) - 1) if key in controller.SIGNATURE_STYLES else (-(1 << 31), (1 << 31) - 1)
        for value in (True, False, 1.0, "1", None, [], {}, lower - 1, upper + 1):
            changed = copy.deepcopy(sample)
            changed["service_signature"][key] = value
            invalid_signatures.append((key + "-" + repr(value), changed, True, 2))
    for value in (None, [], "signature", 1):
        invalid_signatures.append(("object-" + repr(value), dict(sample, service_signature=value), True, 2))
    for key, value in (("schema", "fe-native-service-signature-v3"), ("extra", 0), ("style_u32", 0), ("outer_left_i32", 0)):
        changed = copy.deepcopy(sample)
        changed["service_signature"][key] = value
        invalid_signatures.append(("mismatch-" + key, changed, True, 2))
    changed = copy.deepcopy(sample)
    changed["service_signature"]["exstyle_u32"] = 0
    invalid_signatures.append(("earlier-exstyle-failure", changed, True, 2))
    changed = copy.deepcopy(sample)
    changed["service_signature"]["client_left_i32"] = 1
    changed["code"] = "window_service_client_rect_refused"
    invalid_signatures.append(("earlier-outer-failure", changed, True, 2))
    for key, value in (("code", "observed"), ("code", "window_service_client_rect_refused"),
                       ("code", "window_service_children_refused"), ("hr", "80004005"),
                       ("attempt_spent", True), ("invoke_entered", True), ("method_returned", True),
                       ("service_style_u32", 0x94000000), ("extra", 0)):
        changed = copy.deepcopy(sample)
        changed[key] = value
        invalid_signatures.append(("raw-" + key + "-" + str(value), changed, True, 2))
    invalid_signatures.append(("invoke-mode", sample, False, 2))
    for exit_code in (0, 3, True):
        invalid_signatures.append(("exit-" + str(exit_code), sample, True, exit_code))
    v2_sample = dict(style_result, code="window_service_outer_rect_refused",
                     service_signature=dict(signature_v2, outer_right_i32=15))
    invalid_signatures.append(("v2-all-passing", dict(v2_sample, service_signature=signature_v2), True, 2))
    for key, value in (("schema", "fe-native-service-signature-v3"), ("schema", None), ("extra", 0)):
        changed = copy.deepcopy(v2_sample)
        changed["service_signature"][key] = value
        invalid_signatures.append(("v2-" + key + "-" + str(value), changed, True, 2))
    for key in controller.SIGNATURE_STYLES + controller.SIGNATURE_OUTER + controller.SIGNATURE_CLIENT:
        lower, upper = (0, (1 << 32) - 1) if key in controller.SIGNATURE_STYLES else (-(1 << 31), (1 << 31) - 1)
        for value in (True, False, 1.0, "1", None, [], {}, lower - 1, upper + 1):
            changed = copy.deepcopy(v2_sample)
            changed["service_signature"][key] = value
            invalid_signatures.append(("v2-" + key + "-" + repr(value), changed, True, 2))
    for key, value in (("code", "observed"), ("code", "window_service_client_rect_refused"), ("hr", "80004005"),
                       ("attempt_spent", True), ("invoke_entered", True), ("method_returned", True), ("service_style_u32", 0)):
        changed = copy.deepcopy(v2_sample)
        changed[key] = value
        invalid_signatures.append(("v2-raw-" + key + "-" + str(value), changed, True, 2))
    invalid_signatures.append(("v2-invoke-mode", v2_sample, False, 2))
    for exit_code in (0, 3, True):
        invalid_signatures.append(("v2-exit-" + str(exit_code), v2_sample, True, exit_code))
    for name, changed, observe, exit_code in invalid_signatures:
        refused = False
        try:
            controller.worker_output(encode(changed), observe=observe, helper_exit_code=exit_code)
        except Refused:
            refused = True
        record("signature-" + name + "-refused", refused)
    duplicate_signature_outputs = (
        encode(sample).replace(b'"service_signature":{', b'"service_signature":{"outer_left_i32":0,', 1),
        encode(sample)[:-1] + b',"service_signature":{}}',
    )
    for index, duplicate in enumerate(duplicate_signature_outputs):
        refused = False
        try:
            controller.worker_output(duplicate, observe=True, helper_exit_code=2)
        except Refused:
            refused = True
        record("signature-duplicate-" + str(index) + "-refused", refused)
    provider_result = dict(style_result, code="provider_unavailable")
    record("legacy-provider-refusal-without-diagnostic", controller.worker_output(
        encode(provider_result), observe=True, helper_exit_code=2) == provider_result)
    provider_stages = [(stage, None) for stage in (1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 16, 17, 18, 19)]
    provider_stages.extend(((20, 30019), (20, 30077), (21, 30159)))
    for stage, property_id in provider_stages:
        for hr in ("80000000", "80004005", "ffffffff"):
            diagnostic = {"schema": "fe-native-provider-failure-v1", "stage_id": stage, "hr": hr}
            if property_id is not None:
                diagnostic["property_id"] = property_id
            sample = dict(provider_result, provider_failure=diagnostic)
            parsed = controller.worker_output(encode(sample), observe=True, helper_exit_code=2)
            record("provider-stage-" + str(stage) + "-" + str(property_id) + "-" + hr,
                   parsed == sample and ledger.spent() and not parsed["attempt_spent"])

    provider_sample = dict(provider_result, provider_failure={
        "schema": "fe-native-provider-failure-v1", "stage_id": 20, "property_id": 30019, "hr": "80004005"})
    invalid_providers = []
    for stage in (13, 14, 15, 22, True, False):
        changed = dict(provider_result, provider_failure={
            "schema": "fe-native-provider-failure-v1", "stage_id": stage, "hr": "80004005"})
        invalid_providers.append(("ordinary-stage-" + repr(stage), changed, True, 2))
    for key in ("schema", "stage_id", "property_id", "hr"):
        changed = copy.deepcopy(provider_sample)
        del changed["provider_failure"][key]
        invalid_providers.append(("missing-" + key, changed, True, 2))
    for value in (0, -1, 13, 14, 15, 22, 1 << 32, True, False, 20.0, "20", None, [], {}):
        changed = copy.deepcopy(provider_sample)
        changed["provider_failure"]["stage_id"] = value
        invalid_providers.append(("stage-" + repr(value), changed, True, 2))
    for value in (30031, 30159, 0, -1, 1 << 32, True, False, 30019.0, "30019", None, [], {}):
        changed = copy.deepcopy(provider_sample)
        changed["provider_failure"]["property_id"] = value
        invalid_providers.append(("property-" + repr(value), changed, True, 2))
    for value in ("00000000", "00000001", "7fffffff", "8000000A", "0x80004005", "8000400", "800040050",
                  "g0000000", 0x80004005, True, None, [], {}):
        changed = copy.deepcopy(provider_sample)
        changed["provider_failure"]["hr"] = value
        invalid_providers.append(("hr-" + repr(value), changed, True, 2))
    for key, value in (("schema", "fe-native-provider-failure-v2"), ("schema", None), ("extra", 0)):
        changed = copy.deepcopy(provider_sample)
        changed["provider_failure"][key] = value
        invalid_providers.append(("nested-" + key + "-" + repr(value), changed, True, 2))
    for stage, property_id in ((1, 30019), (1, None), (21, 30019), (21, 30077), (20, 30159)):
        changed = copy.deepcopy(provider_sample)
        changed["provider_failure"].update(stage_id=stage, property_id=property_id)
        invalid_providers.append(("conditional-property-" + str(stage) + "-" + str(property_id), changed, True, 2))
    for value in (None, [], "failure", 1, True):
        invalid_providers.append(("object-" + repr(value), dict(provider_result, provider_failure=value), True, 2))
    for key, value in (("code", "observed"), ("code", "com_unavailable"), ("hr", "80004005"),
                       ("attempt_spent", True), ("invoke_entered", True), ("method_returned", True),
                       ("service_style_u32", 0), ("service_signature", signature_v2), ("extra", 0)):
        changed = copy.deepcopy(provider_sample)
        changed[key] = value
        invalid_providers.append(("raw-" + key + "-" + str(value), changed, True, 2))
    for mode in (False, None, 1):
        invalid_providers.append(("mode-" + repr(mode), provider_sample, mode, 2))
    for exit_code in (-1, 0, 1, 3, True, 2.0, "2", None):
        invalid_providers.append(("exit-" + repr(exit_code), provider_sample, True, exit_code))
    for name, changed, observe, exit_code in invalid_providers:
        refused = False
        try:
            controller.worker_output(encode(changed), observe=observe, helper_exit_code=exit_code)
        except Refused:
            refused = True
        record("provider-" + name + "-refused", refused)
    duplicate_providers = (
        encode(provider_sample)[:-1] + b',"provider_failure":{}}',
        encode(provider_sample).replace(b'"provider_failure":{', b'"provider_failure":{"hr":"80004005",', 1),
        encode(provider_sample).replace(b'"stage_id":20', b'"stage_id":20,"stage_id":20', 1),
        encode(provider_sample).replace(b'"property_id":30019', b'"property_id":30019,"property_id":30019', 1),
    )
    for index, duplicate in enumerate(duplicate_providers):
        refused = False
        try:
            controller.worker_output(duplicate, observe=True, helper_exit_code=2)
        except Refused:
            refused = True
        record("provider-duplicate-" + str(index) + "-refused", refused)
    if os.name == "nt":
        from . import windows
        from types import SimpleNamespace

        class ControlledEvent:
            def __init__(self): self.value = False
            def set(self): self.value = True
            def is_set(self): return self.value
            def wait(self, timeout=None): return self.value

        class ControlledThread:
            def __init__(self, target, args=(), daemon=False):
                self.target, self.args = target, args
            def start(self):
                if not (case == "start-unknown" and self.target.__name__ == "start"):
                    self.target(*self.args)
            def join(self, timeout=None):
                if case == "join-failed": raise OSError("controlled join failure")
            def is_alive(self): return case == "io-retained" and self.target.__name__ == "collect"

        class ControlledProcess:
            def __init__(self, exit_code):
                self.pid, self._handle, self.returncode, self.kills = 1235, 2, exit_code, 0
                self.stdin, self.stdout, self.stderr = io.BytesIO(), io.BytesIO(b"bounded"), io.BytesIO()
                if case == "collector-refusal": self.stderr = io.BytesIO(b"x")
                if case == "overflow-refusal": self.stdout = io.BytesIO(b"x" * (windows.MAX_OUTPUT + 1))
            def poll(self): return self.returncode
            def kill(self):
                self.kills += 1
                if case == "kill-unknown": raise OSError("controlled kill failure")
                self.returncode = 0 if case == "natural-race" else 1
            def wait(self, timeout=None):
                if case == "wait-unknown": raise OSError("controlled wait failure")
                return self.returncode

        for case, initial_exit, expected in (
            ("natural-zero", 0, "worker_exited"), ("natural-two", 2, "worker_exited"),
            ("unexpected-exit", 1, "worker_exited"), ("deadline", None, "worker_deadline_stop_requested"),
            ("natural-race", None, "worker_deadline_stop_requested"),
            ("collector-refusal", None, "worker_output_refused"), ("overflow-refusal", None, "worker_output_refused"),
            ("io-retained", 2, "worker_output_refused"), ("join-failed", 2, "worker_failed_after_start"),
            ("kill-unknown", None, "worker_lifetime_unknown"), ("wait-unknown", None, "worker_lifetime_unknown"),
            ("start-unknown", None, "worker_start_lifetime_unknown"),
        ):
            helper = Path("controlled-native-observer.exe")
            process = ControlledProcess(initial_exit)
            worker_identity = {"pid": process.pid, "start_ticks": "123456790", "exe_path": str(helper)}
            lease = SimpleNamespace(inherited=lambda: [], retained_unknown=False,
                                    process_identity=lambda handle: worker_identity,
                                    api=SimpleNamespace(SetHandleInformation=lambda *args: True))
            clock = iter((0.0, 0.0, 0.0))
            started_workers = []
            with patch.object(windows.subprocess, "Popen", return_value=process), \
                    patch.object(windows.threading, "Event", ControlledEvent), \
                    patch.object(windows.threading, "Thread", ControlledThread), \
                    patch.object(windows.time, "monotonic", side_effect=lambda: next(clock, 5.0)), \
                    patch.object(windows, "UNKNOWN_LIFETIMES", []), patch.object(windows, "UNKNOWN_IO", []):
                try:
                    result = windows.run_worker(lease, helper, b"bounded request", 5.0, started_workers.append)
                    actual = result.code
                    record("transport-" + case + "-identity", result.identity == worker_identity)
                    if case == "natural-race":
                        record("transport-race-reports-request-not-cause", process.kills == 1 and result.exit_code == 0)
                    if case == "io-retained":
                        record("transport-unfinished-io-retained", len(windows.UNKNOWN_IO) == 1)
                except Refused as error:
                    actual = error.code
                    record("transport-" + case + "-owner-retained",
                           lease.retained_unknown and len(windows.UNKNOWN_LIFETIMES) == 1)
                record("transport-" + case, actual == expected and ledger.spent())

        class ControlledLease:
            def __enter__(self): return self
            def __exit__(self, *args): return False
            def owned_file(self, *args, **kwargs): return owner_file
            def process_identity(self, handle): return worker_identity
            def check_live(self): events.append("live")
            api = SimpleNamespace(GetCurrentProcess=lambda: 1, GetTickCount64=lambda: 0)
            processes = [1]

        context = SimpleNamespace(directory=controller.ROOT/".qa/run/controlled", native_dir=controller.ROOT/".qa/run/controlled/native",
                                  identity=worker_identity, helper=helper, helper_manifest={"helper_sha256": "c" * 64},
                                  current={"stage_manifest": {"exe_sha256": "a" * 64}}, ledger=ledger, born="123456790",
                                  run_id="1" * 32, ledger_id="2" * 32, ledger_file_id=file_id)
        args = SimpleNamespace(run="controlled", operation="observe", verse=None, name=None)
        for terminal_code, exit_code, expected in (("worker_deadline_stop_requested", 1, "worker_deadline_stop_requested"),
                                                    ("worker_exited", 1, "worker_exit_refused"),
                                                    ("worker_exited", 0, "controlled_decode"),
                                                    ("worker_exited", 2, "controlled_decode")):
            owner_file, events = MemoryFile(b""), []
            def controlled_worker(files, helper, payload, deadline, on_started):
                on_started(worker_identity)
                return windows.WorkerResult(None, worker_identity, b"", exit_code, terminal_code)
            def controlled_decode(*args):
                phases = [json.loads(line)["phase"] for line in owner_file.raw.splitlines()]
                events.append(("decode", phases))
                raise Refused("controlled_decode")
            with patch.object(controller, "NativeLease", ControlledLease), \
                    patch.object(controller, "Context", return_value=context), \
                    patch.object(controller, "admit_operation"), patch.object(controller, "run_worker", controlled_worker), \
                    patch.object(controller, "decode", controlled_decode):
                try:
                    controller.execute(args)
                    actual = "unexpected_success"
                except Refused as error:
                    actual = error.code
            phases = [json.loads(line)["phase"] for line in owner_file.raw.splitlines()]
            record("transport-receipt-before-decode-" + terminal_code + "-" + str(exit_code),
                   actual == expected and phases == ["reserved", "worker_started", "exit_confirmed"]
                   and (events == [("decode", phases)] if expected == "controlled_decode" else not events)
                   and ledger.spent())

    ledger_before_exit_refusal = file.raw
    with patch.object(controller, "decode", side_effect=AssertionError("unexpected helper exit must precede JSON decode")):
        for observe in (True, False):
            for exit_code in (1, 3, 124, -1073741819, True, 2.0, "2", None):
                refused = False
                try:
                    controller.worker_output(b"", observe=observe, helper_exit_code=exit_code)
                except Refused as error:
                    refused = error.code == "worker_exit_refused"
                record("exit-before-decode-" + str(observe) + "-" + repr(exit_code),
                       refused and ledger.spent() and file.raw == ledger_before_exit_refusal)
    called = []
    def native_entry(args):
        called.append((args.run, args.operation, args.member))
        return {"code": "offline_native_dispatch"}, 2
    with patch.object(controller, "entry", native_entry), \
            patch.object(fe, "load_run", side_effect=AssertionError("implicit run loader")), \
            patch.object(fe, "resolve_token", side_effect=AssertionError("implicit Owner token probe")), \
            patch.object(fe, "log_watermark", side_effect=AssertionError("implicit Owner log probe")), \
            patch.object(fe, "http", side_effect=AssertionError("implicit HTTP")), contextlib.redirect_stdout(io.StringIO()):
        status = fe.main(["--run", "offline-explicit-run", "native", "observe", "--pair-run", "offline-owner", "--member", "viewer"])
    record("recipient-cli-bypasses-owner-http", status == 2 and called == [("offline-explicit-run", "observe", "viewer")])
    with tempfile.TemporaryDirectory(prefix="fe-native-offline-history-") as scratch:
        directory = Path(scratch)
        root = directory / "native-observer" / "old"
        (root / "workers").mkdir(parents=True)
        (root / "attempt.jsonl").write_bytes(raw)
        budget = limits()
        for index in range(budget["jsonl_files"] - 2):
            (root / "workers" / (str(index) + ".jsonl")).write_bytes(b"retained unknown receipt\n")
        admit_operation(directory, root, time.monotonic() + 5)
        (root / "workers" / "last.jsonl").write_bytes(b"retained last receipt\n")
        counts, actual_budget = inventory(directory, time.monotonic() + 5)
        record("last-produced-receipt-remains-inspectable", counts["jsonl_files"] == actual_budget["jsonl_files"])
        refused = False
        try:
            admit_operation(directory, root, time.monotonic() + 5)
        except Refused:
            refused = True
        record("next-receipt-refused-before-reservation", refused and (root / "attempt.jsonl").read_bytes() == raw)
        for key in ("incarnation_directories", "start_directories"):
            boundary = {"jsonl_files": 0, "incarnation_directories": 0, "start_directories": 0}
            boundary[key] = budget[key] - 1
            admit(boundary, {key: 1}, budget)
            boundary[key] += 1
            refused = False
            try:
                admit(boundary, {key: 1}, budget)
            except Refused:
                refused = True
            record("shared-" + key + "-admission", refused)
    if os.name == "nt":
        with tempfile.TemporaryDirectory(prefix="fe-native-offline-lifecycle-") as scratch:
            completed = subprocess.run(["powershell.exe", "-NoProfile", "-NonInteractive", "-File",
                str(Path(__file__).with_name("lifecycle_scenario.ps1")), "-Scratch", scratch],
                capture_output=True, timeout=60, check=False)
            require_output = completed.returncode == 0 and len(completed.stdout) <= 1024 * 1024 and not completed.stderr
            record("production-lifecycle-journey-completed", require_output)
            if require_output:
                results.extend(json.loads(completed.stdout.decode("utf-8-sig")))
    else:
        record("windows-lifecycle-journey-unavailable", True)
        results[-1]["skipped"] = True
    return results
