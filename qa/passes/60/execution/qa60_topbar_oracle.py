"""Derived topbar-layer geometry oracle from recorded QA60 UI trees without mutating prior files."""

import json
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
PACKET = ROOT / "qa/passes/60"

CASES = [
    (1024, 720, 1),
    (1280, 720, 1),
    (1600, 900, 1),
    (1024, 720, 2),
    (1280, 720, 2),
    (1600, 900, 2),
    (1159, 720, 1),
    (1160, 720, 1),
    (1161, 720, 1),
]


def intersect_rect(a, b):
    x1 = max(a["x"], b["x"])
    y1 = max(a["y"], b["y"])
    x2 = min(a["x"] + a["w"], b["x"] + b["w"])
    y2 = min(a["y"] + a["h"], b["y"] + b["h"])
    if x2 - x1 > 0.6 and y2 - y1 > 0.6:
        return {"x": x1, "y": y1, "w": x2 - x1, "h": y2 - y1}
    return None


def analyze_case(width, height, scale):
    label = f"matrix-{width}-scale{scale}"
    tree_lines = (PACKET / f"owner-{label}-number-after-ui.jsonl").read_text(encoding="utf-8").splitlines()
    tree = json.loads([line for line in tree_lines if line.strip()][-1])
    ppp = tree["pixels_per_point"]
    limit = 40.0 * ppp
    controls_by_id = {c["id"]: c for c in tree["layout"]["controls"]}

    raw_candidates = [
        n for n in tree["nodes"]
        if n.get("role") == "button"
        and not n.get("hidden")
        and n.get("bounds")
        and n["bounds"]["y"] < limit
        and n["bounds"]["y"] + n["bounds"]["h"] > 0
    ]

    actual_topbar = []
    clipped_scroll_false_positives = []

    for node in raw_candidates:
        ctrl = controls_by_id.get(node["id"])
        clip = ctrl["clip"] if ctrl else {"x": 0.0, "y": 0.0, "w": float(width), "h": float(height)}
        visible_in_clip = intersect_rect(node["bounds"], clip)
        if clip["y"] <= 1.0 and visible_in_clip is not None:
            actual_topbar.append({
                "id": node["id"],
                "name": node["name"],
                "bounds": node["bounds"],
                "clip": clip,
                "enabled": node["enabled"],
                "toggled": node.get("toggled"),
            })
        else:
            clipped_scroll_false_positives.append({
                "id": node["id"],
                "name": node["name"],
                "bounds": node["bounds"],
                "clip": clip,
                "visible_in_own_clip": visible_in_clip is not None,
            })

    actual_overlaps = []
    for index, first in enumerate(actual_topbar):
        for second in actual_topbar[index + 1:]:
            overlap = intersect_rect(first["bounds"], second["bounds"])
            if overlap is not None:
                actual_overlaps.append({
                    "a": first["name"],
                    "b": second["name"],
                    "overlap_width": overlap["w"],
                    "overlap_height": overlap["h"],
                    "a_bounds": first["bounds"],
                    "b_bounds": second["bounds"],
                })

    deselect = next((n for n in actual_topbar if n["name"] == "Deselect object"), None)
    maps = next((n for n in actual_topbar if n["name"] == "Toggle Maps section"), None)
    deselect_to_maps_gap_px = (
        maps["bounds"]["x"] - (deselect["bounds"]["x"] + deselect["bounds"]["w"])
        if deselect and maps else None
    )

    return {
        "case": label,
        "window": {"width": width, "height": height, "ui_scale": scale, "pixels_per_point": ppp},
        "actual_topbar_button_count": len(actual_topbar),
        "actual_topbar_buttons": actual_topbar,
        "actual_topbar_overlaps": actual_overlaps,
        "actual_topbar_fit": len(actual_overlaps) == 0,
        "deselect_to_maps_gap_px": deselect_to_maps_gap_px,
        "clipped_scroll_false_positives_in_naive_y_filter": clipped_scroll_false_positives,
    }


def main():
    results = [analyze_case(w, h, s) for w, h, s in CASES]
    canonical_six = [r for r in results if r["window"]["width"] in (1024, 1280, 1600)]
    breakpoint_three = [r for r in results if r["window"]["width"] in (1159, 1160, 1161)]
    payload = {
        "kind": "qa60-topbar-layer-oracle",
        "source_commit": "9498869e2e1577e939a10f0a1e0f9e07de3b7053",
        "canonical_six_actual_topbar_fit": all(r["actual_topbar_fit"] for r in canonical_six),
        "breakpoint_1159_actual_topbar_fit": breakpoint_three[0]["actual_topbar_fit"],
        "breakpoint_1160_actual_topbar_fit": breakpoint_three[1]["actual_topbar_fit"],
        "breakpoint_1161_actual_topbar_fit": breakpoint_three[2]["actual_topbar_fit"],
        "cases": results,
        "note": (
            "Separates actual unclipped topbar controls (clip.y <= 1.0) from scrolled Hierarchy/Inspector "
            "buttons whose virtual y < 40*ppp lies above their scroll clip (y=113 or y=191). "
            "All six canonical size/scale cases and 1159@1x have zero actual topbar overlaps. "
            "At 1160@1x and 1161@1x with an active selection, Deselect object (right edge 743.5) "
            "genuinely overlaps Toggle Maps section (x=670.6875/671.6875) and Toggle Settings section "
            "(x=734.25/735.25), confirmed both in geometry and in the 1160/1161 screenshots."
        ),
    }
    out = PACKET / "topbar-layer-oracle.json"
    with out.open("x", encoding="utf-8") as handle:
        json.dump(payload, handle, indent=2, allow_nan=False)
        handle.write("\n")
    script_copy = PACKET / "execution/qa60_topbar_oracle.py"
    with script_copy.open("xb") as handle:
        handle.write(Path(__file__).read_bytes())
    print(json.dumps({
        "canonical_six_actual_topbar_fit": payload["canonical_six_actual_topbar_fit"],
        "1159_fit": payload["breakpoint_1159_actual_topbar_fit"],
        "1160_fit": payload["breakpoint_1160_actual_topbar_fit"],
        "1161_fit": payload["breakpoint_1161_actual_topbar_fit"],
        "1280_scale1_gap_px": next(r["deselect_to_maps_gap_px"] for r in results if r["case"] == "matrix-1280-scale1"),
        "1160_scale1_gap_px": next(r["deselect_to_maps_gap_px"] for r in results if r["case"] == "matrix-1160-scale1"),
    }))


if __name__ == "__main__":
    main()
