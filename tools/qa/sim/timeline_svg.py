"""Stdlib SVG timeline: per step x peer, fire -> converged bars (red past budget), action ticks and host CPU; see tools/qa/AGENTS.md §sim."""
import html
import math

import pandas as pd

ROW_H = 18
LEFT = 170
RIGHT = 30
TOP = 64
CPU_H = 40
COLORS = {"ok": "#2f9e44", "late": "#e03131", "never": "#e03131", "always": "#1c7ed6"}


def _x(t, origin, span, width):
    return LEFT + (t - origin) / span * (width - LEFT - RIGHT)


def write(path, analysis, rows, conv, frame, width=1400):
    origin, end = analysis.origin, max(analysis.end, analysis.origin + 1)
    span = end - origin
    keys = []
    if len(conv):
        for _i, c in conv.iterrows():
            keys.append((c["step"], c["peer"]))
    height = TOP + CPU_H + 30 + ROW_H * max(1, len(keys)) + 40
    out = ['<svg xmlns="http://www.w3.org/2000/svg" width="%d" height="%d" font-family="Segoe UI, Arial, sans-serif" font-size="11">' % (width, height),
           '<rect width="100%" height="100%" fill="#ffffff"/>',
           '<text x="12" y="20" font-size="14" font-weight="600">%s: replication timeline (fire to converged, per step and peer)</text>' % html.escape(analysis.scenario),
           '<text x="12" y="38" fill="#555">green = converged within budget; red = past budget or never (bar runs to step end); ticks = action fires; band = host CPU %%</text>']
    # time axis
    step = 10 ** max(0, int(math.log10(max(span, 1))))
    step = step / 2 if span / step < 4 else step
    t = 0.0
    while t <= span:
        x = _x(origin + t, origin, span, width)
        out.append('<line x1="%.1f" y1="%d" x2="%.1f" y2="%d" stroke="#eee"/>' % (x, TOP, x, height - 30))
        out.append('<text x="%.1f" y="%d" fill="#777" text-anchor="middle">+%ds</text>' % (x, height - 14, int(t)))
        t += step
    # host CPU band
    host = analysis.host[analysis.host["peer"] == "_host"] if len(analysis.host) else analysis.host
    if len(host):
        points = []
        for _i, sample in host.iterrows():
            value = pd.to_numeric(sample["cpu_total_pct"], errors="coerce")
            if pd.isna(value) or sample["t"] < origin or sample["t"] > end:
                continue
            points.append("%.1f,%.1f" % (_x(sample["t"], origin, span, width), TOP + CPU_H - CPU_H * min(100.0, float(value)) / 100.0))
        out.append('<rect x="%d" y="%d" width="%d" height="%d" fill="#f8f9fa"/>' % (LEFT, TOP, width - LEFT - RIGHT, CPU_H))
        out.append('<text x="12" y="%d" fill="#555">host CPU 0-100%%</text>' % (TOP + CPU_H / 2 + 4))
        if points:
            out.append('<polyline fill="none" stroke="#868e96" stroke-width="1.2" points="%s"/>' % " ".join(points))
    base = TOP + CPU_H + 20
    # step bands and bars
    for index, (sid, peer) in enumerate(keys):
        y = base + index * ROW_H
        c = conv[(conv["step"] == sid) & (conv["peer"] == peer)].iloc[0]
        window = analysis.step_window.get(sid, {})
        fire = origin + (c["fire_rel"] or 0)
        step_end = window.get("t", end)
        out.append('<text x="12" y="%d">%s</text>' % (y + 12, html.escape("step %s / %s" % (sid, peer))))
        if window.get("t_start"):
            out.append('<rect x="%.1f" y="%d" width="%.1f" height="%d" fill="#f1f3f5"/>' % (
                _x(window["t_start"], origin, span, width), y + 2, max(1.0, _x(step_end, origin, span, width) - _x(window["t_start"], origin, span, width)), ROW_H - 4))
        if c["converged_rel"] is not None and not pd.isna(c["converged_rel"]):
            stop = origin + c["converged_rel"]
            late = (c["convergence_s"] or 0) > c["budget_s"] or c["in_budget"] < c["expectations"]
            color = COLORS["late" if late else "ok"]
            label = "%.1fs" % (c["convergence_s"] or 0)
        else:
            stop = step_end
            color = COLORS["never"]
            label = "unmet %d/%d" % (c["expectations"] - c["met"], c["expectations"])
        x1, x2 = _x(fire, origin, span, width), _x(max(stop, fire), origin, span, width)
        out.append('<rect x="%.1f" y="%d" width="%.1f" height="%d" fill="%s" rx="2"><title>%s</title></rect>' % (
            x1, y + 4, max(2.0, x2 - x1), ROW_H - 8, color, html.escape("step %s %s: %s (budget %ss)" % (sid, peer, label, c["budget_s"]))))
        out.append('<text x="%.1f" y="%d" fill="#333">%s</text>' % (x2 + 4, y + 12, html.escape(label)))
        if len(frame):
            fires = frame[(frame["step"] == sid) & (frame["peer"] == peer) & frame["t_fire"].notna()]
            for _j, event in fires.iterrows():
                if event.get("lane") in ("macro",):
                    continue
                xf = _x(event["t_fire"], origin, span, width)
                tick = "#e03131" if str(event.get("verdict", "")).startswith(("accepted_but", "refused_but", "timed_out")) else "#343a40"
                out.append('<line x1="%.1f" y1="%d" x2="%.1f" y2="%d" stroke="%s"><title>%s</title></line>' % (
                    xf, y + 1, xf, y + ROW_H - 1, tick, html.escape("%s %s/%s %s" % (event["action_id"], event["via"], event["op"], event.get("verdict")))))
    if not keys:
        out.append('<text x="%d" y="%d" fill="#555">No expectations were evaluated.</text>' % (LEFT, base + 12))
    out.append("</svg>")
    with open(path, "w", encoding="utf-8") as handle:
        handle.write("\n".join(out))
