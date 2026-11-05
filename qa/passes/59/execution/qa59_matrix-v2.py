"""Collect actual layout evidence without converting violations into passes."""

import hashlib
import json
from pathlib import Path

from qa59_flow import PACKET, drive, observe_actions, record_actions, resize_body, save, scroll_to

source = Path(__file__).read_bytes()
flow = Path(__file__).with_name("qa59_flow.py").read_bytes()
(PACKET / "execution").mkdir(exist_ok=True)
for name, content in (("qa59_matrix-v2.py", source), ("qa59_flow-matrix-v2.py", flow)):
    with (PACKET / "execution" / name).open("xb") as handle:
        handle.write(content)

cases = []
for width, height, scale in ((1024,720,1),(1280,720,1),(1600,900,1),(1024,720,2),(1280,720,2),(1600,900,2)):
    label = f"matrix-{width}-scale{scale}"
    drive(label + "-size", "window", "size", str(width), str(height))
    drive(label + "-scale", "window", "ui-scale", str(scale))
    state = drive(label + "-state", "state")[-1]
    assert state["window"]["width"] == width and state["window"]["ui_scale"] == scale
    assert state["selection"] is not None
    try:
        resize_body(label + "-body", 260)
        tree = scroll_to(label + "-grid", "Edit property grid_number")
        facts = record_actions(tree, ["Edit property grid_number", "Copy property grid_number",
                                      "Delete property grid_number"])
        drive(label + "-layout", "layout-check")
        shot = drive(label + "-shot", "shot", "--label", "qa59-" + label)[-1]
        facts.update(case=label, window=state["window"], screenshot=shot["path"],
                     ui_artifact=f"owner-{label}-grid-after-ui.jsonl")
    except (AssertionError, KeyError) as error:
        shot = drive(label + "-unknown-shot", "shot", "--label", "qa59-" + label + "-unknown")[-1]
        facts = {"case": label, "whole_targets_fit": False, "coverage": "UNKNOWN",
                 "problem": str(error), "window": state["window"], "screenshot": shot["path"]}
    cases.append(facts)
    print(json.dumps({"case": label, "whole_targets_fit": facts["whole_targets_fit"],
                      "body_logical_width": facts.get("body_logical_width")}), flush=True)
    save(label + "-comparison.json", facts)

save("responsive-matrix-rebuilt.json", {
    "stage_commit": "be081abed05be3c623b93bef33d4dd31d43d9b46",
    "script_sha256": hashlib.sha256(source).hexdigest(),
    "flow_sha256": hashlib.sha256(flow).hexdigest(),
    "cases": cases,
    "all_cases_pass": all(case["whole_targets_fit"] for case in cases),
    "limits": ["Detected violations remain failures; independent review and functional checks remain required.",
               "No object mutations or clipboard reads; actual screenshots require inspection."],
})
