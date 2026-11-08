"""Plan the mouse-wheel input that scrolls an enabled but clipped widget into view; see tools/qa/AGENTS.md §sim (off-clip rule)."""
import math

POINTS_PER_LINE = 40.0  # egui line_scroll_speed: points per wheel line (x pixels_per_point = physical px)
MIN_PIXELS_PER_LINE, MAX_PIXELS_PER_LINE = 5.0, 400.0  # bounds for the calibrated estimate
MAX_LINES = 12  # wheel lines in one input
MAX_STEPS = 12  # inputs per locate
MAX_STALLS = 3  # inputs that moved nothing before the planner gives up
EDGE_SLACK_PX = 8.0  # content-rect containment tolerance
RANGE_SLACK_PX = 1.0  # a scroll range at or below this is exhausted
MARGIN_FRACTION, MARGIN_MIN_PX, MARGIN_MAX_PX = 0.15, 16.0, 96.0  # how far inside the view the widget should land
SIDEBAR_ANCHOR = (0.10, 0.55)  # window fractions: the hierarchy panel, used when the tree exposes no scroll geometry
INSPECTOR_ANCHOR = (0.87, 0.55)  # window fractions: the right panel (scroll_until's default)
BLIND_LINES = 3  # lines per input when the widget has no rect to measure


def finite_rect(rect):
    return isinstance(rect, dict) and all(
        isinstance(rect.get(key), (int, float)) and not isinstance(rect.get(key), bool) and math.isfinite(rect[key])
        for key in ("x", "y", "w", "h"))


def finite_pair(value):
    return isinstance(value, list) and len(value) == 2 and all(
        isinstance(item, (int, float)) and not isinstance(item, bool) and math.isfinite(item) for item in value)


def intersect(first, second):
    left, top = max(first["x"], second["x"]), max(first["y"], second["y"])
    right = min(first["x"] + first["w"], second["x"] + second["w"])
    bottom = min(first["y"] + first["h"], second["y"] + second["h"])
    return {"x": left, "y": top, "w": right - left, "h": bottom - top} if right > left and bottom > top else None


def center(rect):
    return rect["x"] + rect["w"] / 2.0, rect["y"] + rect["h"] / 2.0


def contains(rect, point, slack=0.0):
    return (rect["x"] - slack <= point[0] <= rect["x"] + rect["w"] + slack
            and rect["y"] - slack <= point[1] <= rect["y"] + rect["h"] + slack)


def assess(entry, window):
    """{reason, axis, sign, view}: why the match is unreachable and which scroll (axis 0=x 1=y, sign +1 = offset grows) would fix it."""
    bounds = entry.get("bounds")
    if not finite_rect(bounds):
        return {"reason": "no_rect", "axis": None, "sign": 0, "view": None}
    facts = entry.get("facts")
    if facts is not None and facts.get("allowed") is not True:
        return {"reason": "modal_background", "axis": None, "sign": 0, "view": None}
    clip = (facts or {}).get("clip")
    view = intersect(window, clip) if finite_rect(clip) else dict(window)
    if view is None:
        return {"reason": "view_empty", "axis": None, "sign": 0, "view": None}
    for axis, sign, reason, outside in (
            (1, -1, "above_view", bounds["y"] + bounds["h"] <= view["y"]),
            (1, 1, "below_view", bounds["y"] >= view["y"] + view["h"]),
            (0, -1, "left_of_view", bounds["x"] + bounds["w"] <= view["x"]),
            (0, 1, "right_of_view", bounds["x"] >= view["x"] + view["w"])):
        if outside:
            return {"reason": reason, "axis": axis, "sign": sign, "view": view}
    return {"reason": "hidden_in_view" if entry.get("hidden") else "no_hit_area", "axis": None, "sign": 0, "view": view}


def pick_scroll(entry, axis, sign, scrolls):
    """(scroll, remaining_px, how): the innermost scroll area holding the widget that can still move the wanted way."""
    bounds = entry["bounds"]
    point = center(bounds)
    extent_key, other = ("w", 1) if axis == 0 else ("h", 0)
    holding, same_column = [], []
    for scroll in scrolls or []:
        axes = scroll.get("axes")
        viewport, content = scroll.get("viewport"), scroll.get("content_rect")
        offset, content_size = scroll.get("offset"), scroll.get("content_size")
        if not (isinstance(axes, list) and len(axes) == 2 and axes[axis] is True):
            continue
        if not (finite_rect(viewport) and finite_rect(content) and finite_pair(offset) and finite_pair(content_size)):
            continue
        if viewport["w"] <= 0 or viewport["h"] <= 0:
            continue
        remaining = content_size[axis] - (offset[axis] + viewport[extent_key]) if sign > 0 else offset[axis]
        if remaining <= RANGE_SLACK_PX:
            continue
        if contains(content, point, EDGE_SLACK_PX):
            holding.append((content["w"] * content["h"], scroll, remaining))
            continue
        start, span = ("x", "w") if other == 0 else ("y", "h")
        if viewport[start] <= point[other] <= viewport[start] + viewport[span]:
            same_column.append((viewport["w"] * viewport["h"], scroll, remaining))
    for how, group in (("holds_widget", holding), ("same_column", same_column)):
        if group:
            _area, scroll, remaining = min(group, key=lambda item: item[0])
            return scroll, remaining, how
    return None, 0.0, None


def lines_for(distance_px, pixels_per_line):
    return max(1, min(MAX_LINES, int(math.ceil(max(distance_px, 0.0) / max(pixels_per_line, 1.0)))))


def wheel_for(axis, sign, lines, anchor):
    """Pointer anchor plus the wheel deltas: positive wheel = content moves down/right, so a growing offset is negative."""
    delta = -float(lines) if sign > 0 else float(lines)
    return {"x": anchor[0], "y": anchor[1], "dx": delta if axis == 0 else 0.0, "dy": delta if axis == 1 else 0.0,
            "lines": lines}


def clamp_into(window, x, y):
    return (min(max(x, window["x"] + 4.0), window["x"] + window["w"] - 4.0),
            min(max(y, window["y"] + 4.0), window["y"] + window["h"] - 4.0))


def plan(diagnostics, entry, pixels_per_line, stage=0):
    """Next wheel input for a clipped match: {reason, axis, sign, via, scroll, content_id, offset, remaining, wheel|None, why_not}.

    stage 0 wheels the scroll area holding the widget; stage 1 the widget's own column at the middle of its observed
    clip (finds a scroll area the layout does not list); stage 2 the sidebar, only when the tree exposes no scroll
    geometry at all. A wheel never lands outside a panel the widget is known to live in: the 3D viewport zooms on wheel.
    """
    window = diagnostics["window"]
    verdict = assess(entry, window)
    result = {"reason": verdict["reason"], "axis": verdict["axis"], "sign": verdict["sign"], "via": None, "scroll": None,
              "content_id": None, "offset": None, "remaining": None, "wheel": None, "why_not": None}
    if verdict["axis"] is None:
        result["why_not"] = verdict["reason"]
        return result
    axis, sign, view, bounds = verdict["axis"], verdict["sign"], verdict["view"], entry["bounds"]
    extent_key, start_key = ("w", "x") if axis == 0 else ("h", "y")
    margin = min(max(view[extent_key] * MARGIN_FRACTION, MARGIN_MIN_PX), MARGIN_MAX_PX)
    if sign > 0:
        distance = bounds[start_key] + bounds[extent_key] - (view[start_key] + view[extent_key])
    else:
        distance = view[start_key] - bounds[start_key]
    distance += margin
    scroll, remaining, how = pick_scroll(entry, axis, sign, diagnostics.get("scrolls"))
    if stage == 0 and scroll is not None:
        anchor_rect = intersect(intersect(scroll["viewport"], window), view) or intersect(scroll["viewport"], window)
        if anchor_rect is not None:
            result.update(via=how, scroll=scroll.get("label"), content_id=scroll.get("content_id"),
                          offset=scroll["offset"][axis], remaining=remaining)
            result["wheel"] = wheel_for(axis, sign, lines_for(min(distance, remaining), pixels_per_line), center(anchor_rect))
            return result
    if stage <= 1 and finite_rect((entry.get("facts") or {}).get("clip")):
        anchor_x, anchor_y = center(view)
        point = center(bounds)
        anchor = clamp_into(window, point[0] if axis == 1 else anchor_x, point[1] if axis == 0 else anchor_y)
        result.update(via="blind_column")
        result["wheel"] = wheel_for(axis, sign, lines_for(distance, pixels_per_line), anchor)
        return result
    if stage <= 2 and not diagnostics.get("scrolls"):
        anchor = clamp_into(window, window["x"] + window["w"] * SIDEBAR_ANCHOR[0], window["y"] + window["h"] * SIDEBAR_ANCHOR[1])
        result.update(via="blind_sidebar")
        result["wheel"] = wheel_for(axis, sign, lines_for(distance, pixels_per_line), anchor)
        return result
    result["why_not"] = "no_scroll_area" if stage == 0 else "no_scroll_moved_the_widget"
    return result


def plan_unplaced(diagnostics, stage=0):
    """Fallback when the tree gives the match no rect: wheel the sidebar (stage 0), then the right panel (stage 1), down in small steps.

    With scroll geometry exposed the panels are the leftmost / rightmost vertical scroll areas that can still move down;
    without it the fixed window fractions stand in.
    """
    window = diagnostics.get("window")
    if not finite_rect(window) or stage > 1:
        return None
    movable = [scroll for scroll in diagnostics.get("scrolls") or []
               if scroll.get("axes") and scroll["axes"][1] is True and finite_rect(scroll.get("viewport"))
               and finite_pair(scroll.get("offset")) and finite_pair(scroll.get("content_size"))
               and scroll["viewport"]["w"] > 0 and scroll["viewport"]["h"] > 0
               and scroll["content_size"][1] - (scroll["offset"][1] + scroll["viewport"]["h"]) > RANGE_SLACK_PX
               and intersect(scroll["viewport"], window) is not None]
    if movable:
        movable.sort(key=lambda scroll: scroll["viewport"]["x"])
        anchor = center(intersect((movable[0] if stage == 0 else movable[-1])["viewport"], window))
    else:
        fraction = SIDEBAR_ANCHOR if stage == 0 else INSPECTOR_ANCHOR
        anchor = clamp_into(window, window["x"] + window["w"] * fraction[0], window["y"] + window["h"] * fraction[1])
    return {"reason": "no_rect", "axis": 1, "sign": 1, "via": "blind_unplaced", "scroll": None, "content_id": None,
            "offset": None, "remaining": None, "wheel": wheel_for(1, 1, BLIND_LINES, anchor), "why_not": None}


class Reveal:
    """Scroll bookkeeping for one locate: inputs sent, stalls, the calibrated wheel speed and the note for a final refusal."""

    def __init__(self, pixels_per_line=None):
        self.pixels_per_line = pixels_per_line
        self.scrolled = 0
        self.stalls = 0
        self.stage = 0
        self.last = None
        self.note = None
        self.position = None

    def commands(self, wheel):
        return [{"kind": "pointer_move", "x": wheel["x"], "y": wheel["y"]},
                {"kind": "scroll", "dx": wheel["dx"], "dy": wheel["dy"]}]

    def settled(self, x, y):
        """True once two consecutive locates agree, so a click never lands on a row still sliding after a scroll."""
        previous, self.position = self.position, (x, y)
        return previous is not None and abs(previous[0] - x) < 1.5 and abs(previous[1] - y) < 1.5

    def step(self, diagnostics, allow=True):
        """The next wheel input for these diagnostics, or None (nothing clipped, nothing left to try, or allow=False); updates .note."""
        diagnostics = diagnostics or {}
        clipped, unplaced = diagnostics.get("clipped") or [], diagnostics.get("unplaced") or []
        window = diagnostics.get("window")
        if not finite_rect(window) or not (clipped or unplaced):
            self.note = None
            return None
        entry = next((item for item in clipped if assess(item, window)["axis"] is not None), clipped[0] if clipped else None)
        earlier = (self.note or {}).get("why_not")  # why the planner gave up on an earlier read outlives the spent budget
        self.note = {"name": (entry or unplaced[0])["name"], "reason": assess(entry, window)["reason"] if entry else "no_rect",
                     "scrolled": self.scrolled}
        if not allow:
            self.note["why_not"] = earlier or "wait_budget_spent"
            return None
        self._judge_progress(entry, diagnostics)
        planned = None
        if self.scrolled < MAX_STEPS and self.stalls < MAX_STALLS:
            if entry is not None:
                planned = plan(diagnostics, entry, self._pixels_per_line(diagnostics), self.stage)
            else:
                planned = plan_unplaced(diagnostics, 0 if self.scrolled < MAX_STEPS // 2 else 1)
        if planned is None or planned["wheel"] is None:
            self.note["why_not"] = (planned or {}).get("why_not") or ("scroll_budget_spent" if self.scrolled >= MAX_STEPS
                                                                      else "no_scroll_moved_the_widget")
            return None
        self.note["via"] = planned["via"]
        if planned["scroll"]:
            self.note["scroll"] = planned["scroll"]
        self.last = {"plan": planned, "bounds": dict(entry["bounds"]) if entry else None}
        self.scrolled += 1
        self.note["scrolled"] = self.scrolled
        return planned["wheel"]

    def _pixels_per_line(self, diagnostics):
        if self.pixels_per_line:
            return self.pixels_per_line
        ratio = diagnostics.get("pixels_per_point")
        ratio = ratio if isinstance(ratio, (int, float)) and not isinstance(ratio, bool) and ratio > 0 else 1.0
        return POINTS_PER_LINE * ratio

    def _judge_progress(self, entry, diagnostics):
        """After an input: no movement = a stall (try the next pointer anchor); movement away from the end of range calibrates the wheel speed."""
        if self.last is None:
            return
        before = self.last["bounds"]
        if entry is not None and before is not None:
            moved = max(abs(entry["bounds"]["x"] - before["x"]), abs(entry["bounds"]["y"] - before["y"]))
            if moved < 1.0:
                self.stalls += 1
                self.stage += 1
                return
        planned = self.last["plan"]
        if planned["content_id"] is not None:
            now = next((scroll for scroll in diagnostics.get("scrolls") or []
                        if scroll.get("content_id") == planned["content_id"] and finite_pair(scroll.get("offset"))), None)
            if now is not None and planned["wheel"]["lines"]:
                axis = planned["axis"]
                travelled = abs(now["offset"][axis] - planned["offset"])
                if travelled > RANGE_SLACK_PX and planned["remaining"] - travelled > RANGE_SLACK_PX:
                    self.pixels_per_line = min(max(travelled / planned["wheel"]["lines"], MIN_PIXELS_PER_LINE),
                                               MAX_PIXELS_PER_LINE)
