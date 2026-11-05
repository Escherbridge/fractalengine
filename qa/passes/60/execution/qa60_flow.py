"""Owned QA60 command recording through the existing reviewed driver."""

import json
from pathlib import Path
import subprocess
import sys

ROOT = Path(__file__).resolve().parents[2]
PACKET = ROOT / "qa/passes/60"
CONTEXT = json.loads((ROOT / ".omc/research/qa60-owned-context.json").read_text(encoding='utf-8'))
FIXTURE = json.loads((PACKET / "fixture-public-detail.json").read_text(encoding='utf-8'))


def drive(label, *arguments):
    result = subprocess.run(
        [sys.executable, str(ROOT / ".omc/research/qa_pair_drive.py"), "60", "owner", label],
        input=json.dumps(list(arguments)), encoding="utf-8", capture_output=True,
    )
    print(result.stdout.strip(), flush=True)
    if result.returncode:
        raise RuntimeError(f"Recorded command refused: {label}, exit {result.returncode}")
    path = PACKET / f"owner-{label}.jsonl"
    return [json.loads(line) for line in path.read_text(encoding='utf-8').splitlines() if line.strip()]


def api(label, method, path, body=None):
    arguments = ["--verse", FIXTURE["verse_id"], "api", method, path]
    if body is not None:
        arguments.append(json.dumps(body))
    response = drive(label, *arguments)[-1]
    if response.get("ok") is not True:
        raise RuntimeError(f"Recorded API operation refused: {label}")
    return response["data"]


def save(name, value):
    path = PACKET / name
    with path.open("x", encoding="utf-8") as handle:
        json.dump(value, handle, indent=2, allow_nan=False)
        handle.write("\n")


def create_result():
    response = json.loads((PACKET / "owner-create-grid-probe.jsonl").read_text(encoding='utf-8'))
    assert response["ok"] is True
    value = response["data"]
    assert value["persist"] == "committed" and value["history_version"] == 1
    return value


def ui(label):
    return drive(label, "ui")[-1]


def named(tree, name):
    matches = [node for node in tree["nodes"] if node.get("name") == name and node.get("interactive")]
    assert len(matches) == 1, (name, len(matches))
    return matches[0]


def scroll_region(tree, name):
    matches = [value for value in tree["layout"]["scrolls"] if value["label"] == name]
    assert len(matches) == 1, (name, len(matches))
    return matches[0]


def scroll_to(label, name):
    tree = ui(label + "-before-ui")
    bounds = named(tree, name)["bounds"]
    clip = scroll_region(tree, "Inspector")["clip"]
    desired = clip["y"] + min(120, clip["h"] * 0.25)
    delta = (desired - bounds["y"]) / (40 * tree["pixels_per_point"])
    drive(label + "-scroll", "scroll", str(delta), str(clip["x"] + 4),
          str(clip["y"] + clip["h"] / 2))
    return ui(label + "-after-ui")


def contains(outer, inner, tolerance=0.6):
    return (inner["x"] >= outer["x"] - tolerance
            and inner["y"] >= outer["y"] - tolerance
            and inner["x"] + inner["w"] <= outer["x"] + outer["w"] + tolerance
            and inner["y"] + inner["h"] <= outer["y"] + outer["h"] + tolerance)


def observe_actions(tree, names):
    table = scroll_region(tree, "Custom property table")
    inspector = scroll_region(tree, "Inspector")
    assert abs(table["offset"][0]) < 0.6
    facts = []
    for name in names:
        node = named(tree, name)
        controls = [value for value in tree["layout"]["controls"] if value["id"] == node["id"]]
        assert len(controls) == 1, (name, len(controls))
        control = controls[0]
        rect = control["rect"]
        assert node["enabled"] and control["allowed"] and control["clickable"], name
        assert all(abs(rect[key] - node["bounds"][key]) <= 0.6 for key in ("x", "y", "w", "h")), name
        assert all(abs(rect[key] - control["hit"][key]) <= 0.6 for key in ("x", "y", "w", "h")), (name, control)
        assert all(contains(region, rect) for region in (control["clip"], table["clip"], inspector["clip"])), (name, control)
        assert rect["w"] >= 19.4 and rect["h"] >= 19.4, (name, rect)
        facts.append({"name": name, "node_id": node["id"], "rect": rect,
                      "hit": control["hit"], "child_clip": control["clip"]})
    return {"pixels_per_point": tree["pixels_per_point"], "table": table,
            "inspector_clip": inspector["clip"], "targets": facts}


def resize_body(label, desired):
    tree = ui(label + "-before-ui")
    table = scroll_region(tree, "Custom property table")
    divider = named(tree, "Resize right panel")["bounds"]
    x = divider["x"] + divider["w"] / 2
    y = divider["y"] + divider["h"] / 2
    delta = table["viewport"]["w"] - desired * tree["pixels_per_point"]
    drive(label + "-drag", "drag", str(x), str(y), str(x + delta), str(y))
    return ui(label + "-after-ui")


def record_actions(tree, names):
    facts, problems = [], []
    for name in names:
        try:
            facts.extend(observe_actions(tree, [name])["targets"])
        except AssertionError as error:
            problems.append({"name": name, "assertion": str(error)})
    table = scroll_region(tree, "Custom property table")
    return {"whole_targets_fit": not problems, "problems": problems, "targets": facts,
            "pixels_per_point": tree["pixels_per_point"], "table": table,
            "body_logical_width": table["viewport"]["w"] / tree["pixels_per_point"]}
