"""QA61 native observation run on matching 9498869e stage."""

import json
from pathlib import Path
import subprocess
import sys

ROOT = Path(__file__).resolve().parents[2]
RUN = ROOT / ".qa/run/20261104-232148-169"
PACKET = ROOT / "qa/passes/61"
PACKET.mkdir(parents=True, exist_ok=True)

current = json.loads((RUN / "current.json").read_text(encoding="utf-8-sig"))
owned_context = {
    "owner": {
        "run_dir": current["run_dir"],
        "pid": current["pid"],
        "process_identity": current["process_identity"],
        "descendant_identities": current["descendant_identities"],
        "stage_manifest": current["stage_manifest"],
        "stage_matches_source": current["stage_matches_source"],
        "fixture": current["fixture"],
        "started_at": current["started_at"],
        "bootstrap_verse_id": current["bootstrap_verse_id"],
    }
}
context_path = ROOT / ".omc/research/qa61-owned-context.json"
context_path.write_text(json.dumps(owned_context, indent=2) + "\n", encoding="utf-8")

fixture_path = PACKET / "fixture-public-detail.json"
with fixture_path.open("x", encoding="utf-8") as handle:
    json.dump(current["fixture_detail"], handle, indent=2)
    handle.write("\n")

first = subprocess.run(
    [sys.executable, str(ROOT / ".omc/research/qa_pair_drive.py"), "61", "owner", "native-observe-provider-facts"],
    input=json.dumps(["native", "observe", "--controlled-fixture"]),
    encoding="utf-8",
    capture_output=True,
)
print("FIRST_STDOUT:", first.stdout.strip(), flush=True)
print("FIRST_STDERR:", first.stderr.strip(), flush=True)
print("FIRST_EXIT:", first.returncode, flush=True)
