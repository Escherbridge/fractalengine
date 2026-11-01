"""Pure measured geometry report; see tools/qa/AGENTS.md layout-check."""


def outside(rect, clip, axis):
    start, size = ("x", "w") if axis == 0 else ("y", "h")
    return rect[start] < clip[start] - 1 or rect[start] + rect[size] > clip[start] + clip[size] + 1


def overlaps(a, b):
    return min(a["x"] + a["w"], b["x"] + b["w"]) - max(a["x"], b["x"]) > 2 and min(a["y"] + a["h"], b["y"] + b["h"]) - max(a["y"], b["y"]) > 2


def analyze(tree, state):
    nodes = tree.get("nodes", [])
    by_id = {n["id"]: n for n in nodes}
    layout = tree.get("layout", {})
    scrolls = {s["content_id"]: s for s in layout.get("scrolls", [])}
    controls = {c["id"]: c for c in layout.get("controls", [])}
    texts = {t["id"]: t for t in layout.get("texts", [])}
    window = tree.get("window") or {}
    width, height = window.get("width", 0), window.get("height", 0)
    window_rect = {"x":0,"y":0,"w":width,"h":height}
    violations, unknown, intentional, visible = [], [], [], []
    for node in nodes:
        if not node.get("interactive") or node.get("hidden") or not node.get("enabled"):
            continue
        bounds = node.get("bounds")
        if not bounds:
            unknown.append({"id":node["id"], "reason":"missing_bounds"})
            continue
        ancestors, parent, seen = [], node.get("parent"), set()
        while parent and parent not in seen:
            seen.add(parent)
            if parent in scrolls:
                ancestors.append(scrolls[parent])
            parent = by_id.get(parent, {}).get("parent")
        entry = {"id":node["id"],"name":node.get("name"),"bounds":bounds,"panel":ancestors[0]["label"] if ancestors else "window/chrome or uninstrumented region"}
        facts = controls.get(node["id"])
        if facts and not facts["allowed"]:
            intentional.append(dict(entry, reason="modal_background"))
            continue
        reachable_scroll = False
        blocked_clip = False
        for scroll in ancestors:
            for axis, size in [(0,"w"),(1,"h")]:
                if outside(bounds,scroll["clip"],axis):
                    if scroll.get("axes",[False,True])[axis] and scroll["content_size"][axis] > scroll["clip"][size] + 1:
                        reachable_scroll = True
                    else:
                        blocked_clip = True
        if blocked_clip:
            violations.append(dict(entry,reason="clipped_without_scroll_range"))
            continue
        if reachable_scroll:
            intentional.append(dict(entry,reason="scroll_content"))
            continue
        if outside(bounds,window_rect,0) or outside(bounds,window_rect,1):
            # Uninstrumented windows may contain a real scroll path. This cannot be proven here.
            if ancestors:
                violations.append(dict(entry,reason="outside_window_without_scroll_path"))
            else:
                unknown.append(dict(entry,reason="outside_window_scroll_or_overlay_uninstrumented"))
            continue
        visible.append(node)
        if facts and (facts["clickable"] or facts["draggable"]):
            if facts["hit"]["w"] < 20 or facts["hit"]["h"] < 20:
                violations.append(dict(entry,reason="small_hit_target",hit=facts["hit"]))
        else:
            unknown.append(dict(entry,reason="hit_rectangle_uninstrumented"))
    for index, left in enumerate(visible):
        for right in visible[index + 1:]:
            if left.get("parent") != right.get("parent") or not overlaps(left["bounds"],right["bounds"]):
                continue
            a,b = controls.get(left["id"]),controls.get(right["id"])
            if a and b and a["clickable"] and b["clickable"] and a["layer"] == b["layer"]:
                violations.append({"ids":[left["id"],right["id"]],"reason":"overlapping_sibling_controls"})
            else:
                unknown.append({"ids":[left["id"],right["id"]],"reason":"overlap_layer_or_composite_unknown"})
    for text in texts.values():
        if text["truncated"] and not text["hover"]:
            violations.append({"id":text["id"],"reason":"truncated_text_without_full_value"})
    unknown_texts = sum(n.get("role") == "label" and n["id"] not in texts and not n.get("hidden") for n in nodes)
    viewport = state.get("viewport") or {}
    share = viewport.get("w",0) / width if width else 0
    if share < 0.45 - 0.001:
        violations.append({"reason":"protected_viewport_width","actual_share":share,"minimum_share":0.45})
    panels = {}
    for kind, entries in [("violations",violations),("unknown",unknown),("intentional_scroll_content",intentional)]:
        for entry in entries:
            panel = panels.setdefault(entry.get("panel","window"),{"violations":0,"unknown":0,"intentional_scroll_content":0})
            panel[kind] += 1
    return {"frame":tree.get("frame"),"state_frame":state.get("frame"),"layout_pass":layout.get("pass"),"window":window,
        "viewport":viewport,"viewport_share":share,"panels":panels,"violations":violations,"unknown":unknown,
        "intentional_scroll_content":intentional,"observed_controls":len(controls),"observed_texts":len(texts),
        "unknown_texts":unknown_texts,"coverage_complete":False,
        "coverage":"same-pass shared helper hits/text and scroll ancestry measured; unvisited controls/text and floating-area occlusion remain unknown"}
