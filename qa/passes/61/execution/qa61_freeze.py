"""Freeze QA61 (separate-command launch job termination before observe) packet."""

import json
import os
from pathlib import Path
import stat

from qa_packet_lib import PATTERNS, copy_exact, decode_text, describe_file, describe_log, manifest_files, scan

R = Path(__file__).resolve().parents[2]
P = R / "qa/passes/61"
RUN = R / ".qa/run/20261104-232148-169"
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


cp(R / ".omc/research/qa61-owned-context.json", "owned-context.json")
cp(R / ".omc/research/qa61-cleanup-identities.json", "cleanup-identities.json")
cp(R / ".omc/research/qa61-launch.log", "logs/launch.log")
cp(R / ".omc/research/qa61-stop.log", "logs/stop.log")
cp(STAGE / "stage.json", "stage-manifest.json")
cp(BATCH / "build-stage-receipt.json", "stage-build-receipt.json")
cp(BATCH / "build-stage.log", "logs/stage.log")
for name in ["qa61_native_run.py", "check_qa61_cleanup.ps1", "qa61_freeze.py", "qa_pair_drive.py", "qa_packet_lib.py"]:
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
    if p.is_file() and p.suffix != ".png" and p.name != "stage.log":
        t, enc = decode_text(p.read_bytes())
        counts = scan(t)
        assert not any(counts.values()), p
        scans.append({"file": p.relative_to(P).as_posix(), "encoding": enc, "counts": counts})
    elif p.is_file() and p.name == "stage.log":
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

write(
    "packet-summary.json",
    {
        "pass_id": 61,
        "verdict": "REFUSED_OWNED_PROCESS_TERMINATED_WHEN_SEPARATE_LAUNCH_COMMAND_EXITED",
        "run_id": "20261104-232148-169",
        "source_commit": "9498869e2e1577e939a10f0a1e0f9e07de3b7053",
        "source_dirty": True,
        "stage_matches_source": True,
        "original_artifacts": originals,
        "original_source_anchors": copies,
        "controller_result": json.loads((P / "owner-native-observe-provider-facts.jsonl").read_text(encoding="utf-8")),
        "workers": [],
        "invocation_ledger_summary": {
            "file_sha256": describe_file(ledger_path)["sha256"],
            "record_count": len(ledger_lines),
            "bootstrap_record_count": sum(1 for r in ledger_lines if r.get("kind") == "bootstrap"),
            "invocation_reservation_or_intent_present": any(r.get("kind") != "bootstrap" for r in ledger_lines),
            "raw_ledger_published": False,
        },
        "cleanup": {
            "checked_at": "2026-11-05T06:24:41.6229843Z",
            "recorded_identity_count": 8,
            "all_recorded_identities_absent": True,
        },
        "facts": [
            "Launch completed in a separate backgrounded shell command (PID 103968). When that shell command exited, the host command runner closed its job object and terminated PID 103968 before the subsequent command invoked native observe.",
            "Controller Context.__init__ failed live_process(identity) with ValueError('exact owned app process is unavailable'), returning controller code native_owned_boundary_unavailable, outcome unavailable_or_unknown, attempt_spent null, and spawning zero worker processes.",
            "The private invocation ledger contains the single bootstrap record only. All eight recorded identities are confirmed absent at cleanup.",
        ],
        "limits": [
            "No native worker executed in QA61 because the owned app exited when the separate launch command completed.",
            "Preserved as immutable failed-launch-lifetime evidence; QA62 runs launch, native observe, and stop within a single shell command lifetime.",
        ],
    },
)

manifest = {"kind": "qa61-packet-manifest", "files": manifest_files(P)}
write("packet-manifest.json", manifest)

receipt = {
    "kind": "qa61-packet-freeze",
    "manifest": describe_file(P / "packet-manifest.json"),
    "payload_count": len(manifest["files"]),
    "png_count": 0,
    "original_anchor_count": len(originals),
    "source_copy_count": len(copies),
    "source_or_original_mismatches": 0,
}
(R / ".omc/research/qa61-packet-freeze-receipt.json").write_text(
    json.dumps(receipt, indent=2) + "\n", encoding="utf-8"
)

for p in P.rglob("*"):
    if p.is_file():
        os.chmod(p, stat.S_IREAD)

print(json.dumps(receipt))
