"""Collect actual layout evidence without converting violations into passes."""

import hashlib
import json
from pathlib import Path

from qa57_flow import PACKET, drive, observe_actions, record_actions, resize_body, save, scroll_to

source = Path(__file__).read_bytes()
flow = Path(__file__).with_name("qa57_flow.py").read_bytes()
(PACKET / "execution").mkdir(exist_ok=True)
for name, content in (("qa57_matrix-v1.py", source), ("qa57_flow-matrix-v1.py", flow)):
    with (PACKET / "execution" / name).open("xb") as handle:
        handle.write(content)

cases = []
for width, scale in ((1024, 1), (1280, 1), (1600, 1), (1024, 2), (1280, 2), (1600, 2)):
    label = f"matrix-{width}-scale{scale}"
    drive(label + "-size", "window", "size", str(width), "900")
    drive(label + "-scale", "window", "ui-scale", str(scale))
    state = drive(label + "-state", "state")[-1]
    assert state["window"]["width"] == width and state["window"]["ui_scale"] == scale
    assert state["selection"] is not None
    try:
        resize_body(label + "-body", 260)
        tree = scroll_to(label + "-grid", "Edit property grid_number")
        facts = record_actions(tree, ["Edit property grid_number", "Copy property grid_number",
                                      "Delete property grid_number"])
        shot = drive(label + "-shot", "shot", "--label", "qa57-" + label)[-1]
        facts.update(case=label, window=state["window"], screenshot=shot["path"],
                     ui_artifact=f"owner-{label}-grid-after-ui.jsonl")
    except (AssertionError, KeyError) as error:
        shot = drive(label + "-unknown-shot", "shot", "--label", "qa57-" + label + "-unknown")[-1]
        facts = {"case": label, "whole_targets_fit": False, "coverage": "UNKNOWN",
                 "problem": str(error), "window": state["window"], "screenshot": shot["path"]}
    cases.append(facts)
    print(json.dumps({"case": label, "whole_targets_fit": facts["whole_targets_fit"],
                      "body_logical_width": facts.get("body_logical_width")}), flush=True)
    save(label + "-comparison.json", facts)

save("responsive-matrix-before-height-fix.json", {
    "stage_commit": "6ca9f49bf65bb344ff5bdb0fa9794208c54c4a9a",
    "script_sha256": hashlib.sha256(source).hexdigest(),
    "flow_sha256": hashlib.sha256(flow).hexdigest(),
    "cases": cases,
    "all_cases_pass": all(case["whole_targets_fit"] for case in cases),
    "limits": ["Detected violations remain failures; this is not QA0074 closure.",
               "No object mutations or clipboard reads; actual screenshots require inspection."],
})
