"""Pure measured geometry report; see tools/qa/AGENTS.md layout-check."""

import math


def scroll_geometry_gap(scroll):
    axes = scroll.get("axes")
    if not isinstance(axes, list) or len(axes) != 2 or any(type(axis) is not bool for axis in axes):
        return "scroll_geometry_invalid"
    for key in ("viewport", "content_rect", "clip"):
        rect = scroll.get(key)
        if not isinstance(rect, dict) or any(type(rect.get(field)) not in (int, float)
                or not math.isfinite(rect[field]) for field in ("x", "y", "w", "h")):
            return "scroll_geometry_invalid"
    for axis, size in enumerate(("w", "h")):
        extent = scroll["viewport"][size]
        if extent <= 0:
            return "scroll_enabled_axis_degenerate" if axes[axis] else "scroll_disabled_axis_degenerate"
    if any(not isinstance(scroll.get(key), list) or len(scroll[key]) != 2
           or any(type(value) not in (int, float) or not math.isfinite(value) or value < 0
                  for value in scroll[key]) for key in ("content_size", "offset")):
        return "scroll_geometry_invalid"
    return None


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
        ancestors, clip_sources, unresolved_scrolls, parent, seen = [], [], [], node.get("parent"), set()
        while parent and parent not in seen:
            seen.add(parent)
            if parent in scrolls:
                scroll = scrolls[parent]
                container = by_id.get(parent)
                if not all(key in scroll for key in ("viewport", "content_rect", "parent_id")):
                    unresolved_scrolls.append("legacy_scroll_geometry")
                    if container:
                        # Enabled-axis historical viewport intersections bound the child clip from within.
                        clip_sources.append(scroll)
                elif not container or container.get("parent") != scroll["parent_id"]:
                    unresolved_scrolls.append("scroll_parent_unresolved")
                else:
                    ancestors.append(scroll)
                    clip_sources.append(scroll)
                    gap = scroll_geometry_gap(scroll)
                    if gap:
                        unresolved_scrolls.append(gap)
            parent = by_id.get(parent, {}).get("parent")
        entry = {"id":node["id"],"name":node.get("name"),"bounds":bounds,"panel":ancestors[0]["label"] if ancestors else "window/chrome or uninstrumented region"}
        facts = controls.get(node["id"])
        if facts and not facts["allowed"]:
            intentional.append(dict(entry, reason="modal_background"))
            continue
        for reason in sorted(set(unresolved_scrolls)):
            unknown.append(dict(entry, reason=reason))
        if unresolved_scrolls:
            continue
        if facts and any(scroll["layer"] != facts["layer"] for scroll in clip_sources):
            unknown.append(dict(entry, reason="scroll_layer_unresolved"))
            ancestors = [scroll for scroll in ancestors if scroll["layer"] == facts["layer"]]
            clip_sources = [scroll for scroll in clip_sources if scroll["layer"] == facts["layer"]]
        reachable_scroll = False
        blocked_clip = False
        local_control_clip = False
        inherited_path_unresolved = False
        if facts:
            for axis, size, start in [(0,"w","x"),(1,"h","y")]:
                if not outside(facts["rect"], facts["clip"], axis):
                    continue
                axis_sources = [scroll for scroll in clip_sources if "viewport" in scroll or scroll.get("axes", [False, False])[axis]]
                if not axis_sources:
                    unknown.append(dict(entry, reason="control_clip_scroll_path_uninstrumented"))
                    continue
                clip_start = max(scroll["clip"][start] for scroll in axis_sources)
                clip_end = min(scroll["clip"][start] + scroll["clip"][size] for scroll in axis_sources)
                rect_end = facts["rect"][start] + facts["rect"][size]
                actual_end = facts["clip"][start] + facts["clip"][size]
                if ((facts["rect"][start] < facts["clip"][start] - 1 and facts["clip"][start] > clip_start + 1)
                        or (rect_end > actual_end + 1 and actual_end < clip_end - 1)):
                    # A local scissor moves with its control; scrolling an ancestor cannot reveal it.
                    local_control_clip = True
                    blocked_clip = True
        effective = dict(bounds)
        for index, scroll in enumerate(ancestors):
            for axis, size in [(0,"w"),(1,"h")]:
                if outside(effective,scroll["clip"],axis):
                    if scroll.get("axes",[False,True])[axis] and scroll["content_size"][axis] > scroll["viewport"][size] + 1:
                        reachable_scroll = True
                        start = "x" if axis == 0 else "y"
                        end = min(effective[start] + effective[size],scroll["clip"][start] + scroll["clip"][size])
                        effective[start] = max(effective[start],scroll["clip"][start])
                        effective[size] = max(0,end-effective[start])
                    elif any(outer.get("axes", [False, True])[axis] and outer["content_size"][axis] > outer["viewport"][size] + 1 for outer in ancestors[index + 1:]):
                        # An inherited parent scissor may move into reach; do not certify an unmeasured path.
                        inherited_path_unresolved = True
                    else:
                        blocked_clip = True
        if blocked_clip:
            violations.append(dict(entry,reason="clipped_without_scroll_range",clip_source="control" if local_control_clip else "scroll"))
            continue
        if inherited_path_unresolved:
            unknown.append(dict(entry,reason="inherited_scroll_clip_path_unresolved"))
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
                if outside(facts["rect"],facts["clip"],0) or outside(facts["rect"],facts["clip"],1):
                    unknown.append(dict(entry,reason="partial_clip_scroll_path_uninstrumented",hit=facts["hit"]))
                else:
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
    return {"frame":tree.get("frame"),"state_frame":state.get("frame"),"layout_pass":layout.get("pass"),"scroll_geometry_version":layout.get("scroll_geometry_version"),"window":window,
        "viewport":viewport,"viewport_share":share,"panels":panels,"violations":violations,"unknown":unknown,
        "intentional_scroll_content":intentional,"observed_controls":len(controls),"observed_texts":len(texts),
        "unknown_texts":unknown_texts,"coverage_complete":False,
        "coverage":"same-pass shared helper hits/text and scroll ancestry measured; unvisited controls/text and floating-area occlusion remain unknown"}
