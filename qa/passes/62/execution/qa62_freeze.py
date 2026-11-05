"""Freeze QA62 (same-command native observe worker desktop_refused under exebox thread desktop) packet."""

import json
import os
from pathlib import Path
import stat

from qa_packet_lib import PATTERNS, copy_exact, decode_text, describe_file, describe_log, manifest_files, scan

R = Path(__file__).resolve().parents[2]
P = R / "qa/passes/62"
RUN = R / ".qa/run/20261104-232515-148"
STAGE = R / ".qa/stage/9498869e-20261104204501360"
BATCH = R / "conductor/tracks/project_completion_20261104/evidence/source-batch-20261104"


def write(name, value):
    p = P / name
    p.parent.mkdir(parents=True, exist_ok=True)
    with p.open("x", encoding="utf-8") as h:
        json.dump(value, h, indent=2, ensure_ascii=True)
        h.write("\n")


originals = manifest_files(P)
copies = []


def cp(src, name):
    dst = P / name
    copy_exact(src, dst)
    copies.append({"original_file": src.relative_to(R).as_posix(), "packet_file": name, **describe_file(dst)})


cp(R / ".omc/research/qa62-owned-context.json", "owned-context.json")
cp(R / ".omc/research/qa62-cleanup-identities.json", "cleanup-identities.json")
cp(R / ".omc/research/qa62-launch.log", "logs/launch.log")
cp(R / ".omc/research/qa62-stop.log", "logs/stop.log")
cp(STAGE / "stage.json", "stage-manifest.json")
cp(BATCH / "build-stage-receipt.json", "stage-build-receipt.json")
cp(BATCH / "build-stage.log", "logs/stage.log")

first_result = json.loads((P / "owner-native-observe-provider-facts.jsonl").read_text(encoding="utf-8"))
second_result = json.loads((P / "owner-native-observe-followup.jsonl").read_text(encoding="utf-8"))

first_worker_src = RUN / first_result["worker_record"]
second_worker_src = RUN / second_result["worker_record"]
cp(first_worker_src, "native-observer-worker-receipt.jsonl")
cp(second_worker_src, "native-observer-followup-worker-receipt.jsonl")

for name in ["qa62_native_drive.py", "qa62_native_all_in_one.ps1", "qa62_freeze.py", "qa_pair_drive.py", "qa_packet_lib.py"]:
    cp(R / ".omc/research" / name, "execution/" + name)

logs = {n: describe_log(RUN / n) for n in ["stdout.log", "stderr.log"]}
write(
    "application-log-summary.json",
    {
        "raw_logs_private": True,
        "logs": logs,
        "limits": "Finite marker and four credential-pattern checks over strict-decoded private logs only.",
    },
)

scans = []
for p in sorted(P.rglob("*")):
    if p.is_file() and p.suffix != ".png":
        t, enc = decode_text(p.read_bytes())
        counts = scan(t)
        assert not any(counts.values()), p
        scans.append({"file": p.relative_to(P).as_posix(), "encoding": enc, "counts": counts})

write(
    "credential-scan.json",
    {
        "patterns": PATTERNS,
        "files": scans,
        "private_logs": {n: x["credential_pattern_counts"] for n, x in logs.items()},
        "limits": "Four patterns over listed strict-decoded files and private logs only.",
    },
)

assert all(describe_file(P / x["file"]) == {"bytes": x["bytes"], "sha256": x["sha256"]} for x in originals)

ledger_path = next((RUN / "native-observer").rglob("attempt.jsonl"))
ledger_bytes = ledger_path.read_bytes()
ledger_lines = [json.loads(line) for line in ledger_bytes.decode("utf-8").splitlines() if line.strip()]


def summarize_worker(attempt_name, src_path):
    lines = [json.loads(line) for line in src_path.read_text(encoding="utf-8").splitlines() if line.strip()]
    return {
        "attempt": attempt_name,
        "original_file": src_path.relative_to(R).as_posix(),
        **describe_file(src_path),
        "operation": lines[0]["operation"],
        "phases": [line["phase"] for line in lines],
        "worker_identity": lines[1]["worker_identity"],
        "mode": lines[0]["mode"],
        "exit_code": lines[-1]["exit_code"],
        "terminal_code": lines[-1]["code"],
    }


write(
    "packet-summary.json",
    {
        "pass_id": 62,
        "verdict": "REFUSED_NON_DEFAULT_THREAD_DESKTOP_IN_EXEBOX_SANDBOX",
        "run_id": "20261104-232515-148",
        "source_commit": "9498869e2e1577e939a10f0a1e0f9e07de3b7053",
        "source_dirty": True,
        "stage_matches_source": True,
        "original_artifacts": originals,
        "original_source_anchors": copies,
        "first_native_result": first_result,
        "followup_native_result": second_result,
        "workers": [
            summarize_worker("first", first_worker_src),
            summarize_worker("followup", second_worker_src),
        ],
        "invocation_ledger_summary": {
            "file_sha256": describe_file(ledger_path)["sha256"],
            "record_count": len(ledger_lines),
            "bootstrap_record_count": sum(1 for r in ledger_lines if r.get("kind") == "bootstrap"),
            "invocation_reservation_or_intent_present": any(r.get("kind") != "bootstrap" for r in ledger_lines),
            "raw_ledger_published": False,
        },
        "cleanup": {
            "checked_at": "2026-11-05T06:25:33.2439957Z",
            "recorded_identity_count": 8,
            "all_recorded_identities_absent": True,
        },
        "facts": [
            "Running launch.ps1, two native observe calls, and stop.ps1 inside one shell command preserved the owned app (PID 127884) through both native observer invocations.",
            "Both first and follow-up workers passed controller admission, bootstrap ledger verification, handle inheritance, and Process image/session/token checks, then returned structured fe-native-worker-result-v1 with code desktop_refused, helper_exit_code 2, attempt_spent false, and terminal_code worker_exited.",
            "In this execution environment, GetProcessWindowStation() is WinSta0 and OpenInputDesktop() is Default, but GetThreadDesktop(GetCurrentThreadId()) is an exebox-* desktop rather than Default, which worker.cpp:171 refuses before window enumeration or UIAutomation.",
            "Unlike QA58's follow-up (which returned json_size_refused / exit 1), both QA62 worker runs exited cleanly with exit_code 2 and bounded JSON output.",
        ],
        "limits": [
            "Because worker.cpp refuses non-Default thread desktops at Process construction before EnumWindows and UIAutomation, QA62 does not reach or verify the fed8dab6 UIA root-continuity path on the Default interactive desktop.",
            "The desktop_refused guard is a security/isolation boundary and must not be bypassed or weakened.",
            "No positive UIA root/witness/Invoke/capture acceptance is claimed.",
        ],
    },
)

manifest = {"kind": "qa62-packet-manifest", "files": manifest_files(P)}
write("packet-manifest.json", manifest)

receipt = {
    "kind": "qa62-packet-freeze",
    "manifest": describe_file(P / "packet-manifest.json"),
    "payload_count": len(manifest["files"]),
    "png_count": 0,
    "original_anchor_count": len(originals),
    "source_copy_count": len(copies),
    "source_or_original_mismatches": 0,
}
(R / ".omc/research/qa62-packet-freeze-receipt.json").write_text(
    json.dumps(receipt, indent=2) + "\n", encoding="utf-8"
)

for p in P.rglob("*"):
    if p.is_file():
        os.chmod(p, stat.S_IREAD)

print(json.dumps(receipt))
