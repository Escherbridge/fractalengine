import json
import os
from pathlib import Path
import stat

from qa_packet_lib import PATTERNS, copy_exact, decode_text, describe_file, describe_log, manifest_files, scan

R = Path(__file__).resolve().parents[2]
P = R / "qa/passes/60"
RUN = R / ".qa/run/20261104-204745-332"
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
    copies.append({"source": str(src), "file": name, **describe_file(dst)})


for src in sorted(P.glob("owner-*-shot.jsonl")):
    result = json.loads(src.read_text(encoding="utf-8"))
    image = Path(result["path"])
    assert image.parent == RUN / "shots"
    cp(image, "screenshots/" + image.name)

for name in ["qa60_freeze.py", "qa_pair_drive.py", "qa_packet_lib.py", "check_qa60_cleanup.ps1"]:
    cp(R / ".omc/research" / name, "execution/" + name)

for name in ["qa60-owned-context.json", "qa60-cleanup-identities.json", "qa60-launch.log", "qa60-stop.log"]:
    cp(R / ".omc/research" / name, "provenance/" + name)

for name in ["build-stage-receipt.json", "build-stage.log"]:
    cp(BATCH / name, "provenance/" + name)

cp(STAGE / "stage.json", "provenance/stage.json")

logs = {n: describe_log(RUN / n) for n in ["stdout.log", "stderr.log"]}
write(
    "app-log-review.json",
    {
        "raw_logs_private": True,
        "logs": logs,
        "warning_classes": {"startup": 9},
        "actual_warn_total": 9,
        "error_total": 0,
        "limits": "Finite marker/credential checks are not universal absence proof.",
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
        "limits": "Exactly four patterns over listed strict-decoded files/private logs only. This scan predates its own output, packet summary and manifest. No universal absence claim.",
    },
)

assert all(describe_file(P / x["file"]) == {"bytes": x["bytes"], "sha256": x["sha256"]} for x in originals)

write(
    "packet-summary.json",
    {
        "kind": "qa60-focused-painted-icons-and-topbar-matrix",
        "source_commit": "9498869e2e1577e939a10f0a1e0f9e07de3b7053",
        "stage": ".qa/stage/9498869e-20261104204501360",
        "original_anchors": originals,
        "external_copies": copies,
        "matrix": {
            "six_canonical_size_scale_cases_pass_whole_numeric_row_actions": True,
            "all_nine_cases_pass_whole_numeric_row_actions": True,
            "all_nine_cases_selected_tree_row_whole_visible": True,
            "canonical_six_and_1159_actual_topbar_fit": True,
            "breakpoint_1160_and_1161_actual_topbar_fit": False,
            "breakpoint_1160_deselect_to_maps_gap_px": -72.8125,
            "breakpoint_1161_deselect_to_maps_gap_px": -71.8125,
            "window_sizes": [
                [1024, 720],
                [1280, 720],
                [1600, 900],
                [1159, 720],
                [1160, 720],
                [1161, 720],
            ],
            "scales": [1, 2],
            "layout_check_each_case": True,
            "layout_coverage_complete": False,
            "measured_violations": 0,
        },
        "root_visual_inspection": {
            "actual_pngs_inspected": 10,
            "painted_icons_verified": (
                "All 10 screenshots show clear thin vector icons in topbar tools/panels/Deselect/window controls, "
                "Inspector section rail, hierarchy visibility/type/disclosure controls, and stored-value disclosure "
                "with zero missing-glyph squares."
            ),
            "selected_tree_row_verified": (
                "Selected node QA60 icon probe is scrolled wholly into the Hierarchy clip and renders clear "
                "blue selection fill and outline at both 1x and 2x across all nine matrix cases."
            ),
            "residual": (
                "At 1160x720@1x and 1161x720@1x with an active selection, the expanded topbar switches on at "
                "the 1160.0 breakpoint while Deselect object (right edge 743.5) overlaps Toggle Maps section "
                "(x=670.6875/671.6875) and Toggle Settings section (x=734.25/735.25). At 1159x720@1x compact "
                "Tools/Panels dropdowns fit cleanly; at 1280x720@1x Deselect and Maps have a 47.1875px gap."
            ),
        },
        "owned_cleanup": {
            "recorded_identities": 8,
            "all_absent_at": "2026-11-05T06:19:38.1331795Z",
        },
        "limits": [
            "Focused GUI/visual evidence only; no FULL or universal visual acceptance.",
            "QA0075 painted icons are verified across all 10 screenshots, but the 1160/1161 expanded topbar overlap with Deselect active remains an open responsive breakpoint defect.",
            "Naive y<40*ppp button filter in qa60_matrix.py also matched scrolled Hierarchy/Inspector buttons above their scroll clips (y=113 and y=191); topbar-layer-oracle.json separates actual unclipped topbar buttons from clipped scroll content without rewriting raw artifacts.",
            "No data mutations beyond probe creation, no native UIA, peer, retained restart, tests/fmt/final integrated sweep/CI/tag/release executed in this pass.",
        ],
    },
)

manifest = {"kind": "qa60-packet-manifest", "files": manifest_files(P)}
write("packet-manifest.json", manifest)

receipt = {
    "kind": "qa60-packet-freeze",
    "manifest": describe_file(P / "packet-manifest.json"),
    "payload_count": len(manifest["files"]),
    "png_count": len(list(P.rglob("*.png"))),
    "original_anchor_count": len(originals),
    "source_copy_count": len(copies),
    "source_or_original_mismatches": 0,
    "note": "Frozen file-only packet; independent evidence review pending.",
}
(R / ".omc/research/qa60-packet-freeze-receipt.json").write_text(
    json.dumps(receipt, indent=2) + "\n", encoding="utf-8"
)

for p in P.rglob("*"):
    if p.is_file():
        os.chmod(p, stat.S_IREAD)

print(json.dumps(receipt))
