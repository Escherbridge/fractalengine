"""Join events x observations into per-expectation replication latency, convergence, divergence and anomalies; see tools/qa/AGENTS.md §sim."""
import argparse
import bisect
import collections
import json
import math
import os
import sys

import pandas as pd

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)
import predicates  # noqa: E402
import timeline_svg  # noqa: E402

PEER_LOCAL_PROBES = {"role", "nav", "ui", "alive", "_tick", "_error"}
DIVERGENCE_S = 15.0
PARTIAL_MIN_S = 0.0


# -- generic helpers (lifted from the run-1 analyzer) ------------------------------------------
def load_json(path, default=None):
    try:
        with open(path, encoding="utf-8-sig") as handle:
            return json.load(handle)
    except (OSError, ValueError):
        return default


def load_jsonl(path):
    rows = []
    try:
        with open(path, encoding="utf-8") as handle:
            for line in handle:
                line = line.strip()
                if line:
                    try:
                        rows.append(json.loads(line))
                    except ValueError:
                        continue  # a torn last line from a killed writer
    except OSError:
        pass
    return rows


def pct(values, p):
    values = [v for v in values if v is not None and not (isinstance(v, float) and math.isnan(v))]
    if not values:
        return None
    ordered = sorted(values)
    return ordered[min(len(ordered) - 1, max(0, math.ceil(p / 100.0 * len(ordered)) - 1))]  # nearest-rank


def fmt(value, digits=2):
    if value is None or (isinstance(value, float) and math.isnan(value)):
        return "-"
    if isinstance(value, float):
        return ("%." + str(digits) + "f") % value
    return str(value)


def short(text, size=28):
    text = str(text)
    return text if len(text) <= size else text[: size - 1] + "…"


class Series:
    """Change-logged observations for one (peer, source probe, target): value at any time by bisect."""

    def __init__(self):
        self.times = []
        self.values = []
        self.digests = []
        self.known = []

    def add(self, t, value, digest, known):
        self.times.append(t)
        self.values.append(value)
        self.digests.append(digest)
        self.known.append(known)

    def index_at(self, t):
        return bisect.bisect_right(self.times, t) - 1

    def at(self, t):
        i = self.index_at(t)
        return (None, None, False) if i < 0 else (self.values[i], self.digests[i], self.known[i])


class Analysis:
    def __init__(self, run_dir, divergence_s=DIVERGENCE_S):
        self.run_dir = run_dir
        self.divergence_s = divergence_s
        self.meta = load_json(os.path.join(run_dir, "run.json"), {})
        self.events = load_jsonl(os.path.join(run_dir, "events.jsonl"))
        self.observations = load_jsonl(os.path.join(run_dir, "observations.jsonl"))
        self.expectations = load_jsonl(os.path.join(run_dir, "expectations.jsonl"))
        self.steps = load_jsonl(os.path.join(run_dir, "steps.jsonl"))
        self.scenario = self.meta.get("scenario_name", "?")
        self.series = collections.defaultdict(Series)
        self.ticks = collections.defaultdict(list)
        self.tick_rows = []
        for obs in sorted(self.observations, key=lambda o: o["t"]):
            if obs["probe"] == "_tick":
                self.ticks[obs["peer"]].append(obs["t"])
                self.tick_rows.append(obs)
            elif obs["probe"] != "_error":
                known = obs.get("value_small") is not None or obs.get("digest") == predicates.digest(None)
                self.series[(obs["peer"], obs["probe"], obs["target"])].add(obs["t"], obs.get("value_small"),
                                                                              obs.get("digest"), known)
        starts = [s["t"] for s in self.steps if s.get("event") == "start"]
        self.origin = min(starts) if starts else (min((o["t"] for o in self.observations), default=0.0))
        self.end = max([o["t"] for o in self.observations] + [e.get("t_ack") or 0 for e in self.events] + [self.origin])
        self.step_window = {}
        for record in self.steps:
            if record.get("event") == "end":
                self.step_window[record["step"]] = record
        self.host = self._load_host()
        self.anomalies = []

    def rel(self, t):
        return None if t is None else round(t - self.origin, 3)

    def _load_host(self):
        path = os.path.join(self.run_dir, "host.csv")
        if not os.path.exists(path) or not self.meta.get("clock"):
            return pd.DataFrame(columns=["t", "cpu_total_pct", "free_ram_mb", "peer", "peer_cpu_pct"])
        try:
            frame = pd.read_csv(path)
        except (ValueError, pd.errors.EmptyDataError):
            return pd.DataFrame(columns=["t", "cpu_total_pct", "free_ram_mb", "peer", "peer_cpu_pct"])
        clock = self.meta["clock"]
        frame["t"] = clock["perf0"] + (frame["wall_ms"] / 1000.0 - clock["wall0"])
        return frame

    def cpu_mean(self, start, end):
        host = self.host[self.host["peer"] == "_host"] if len(self.host) else self.host
        if not len(host) or start is None or end is None:
            return None
        window = host[(host["t"] >= start - 1.0) & (host["t"] <= max(end, start) + 1.0)]
        if not len(window):
            return None
        return float(pd.to_numeric(window["cpu_total_pct"], errors="coerce").mean())

    def anomaly(self, severity, step, peers, title, evidence):
        self.anomalies.append({"id": "SIM-%s-%03d" % (self.scenario, len(self.anomalies) + 1),
                               "severity_hint": severity, "scenario": self.scenario, "step": step,
                               "peers": sorted(peers) if isinstance(peers, (list, set, tuple)) else peers,
                               "title": title, "evidence": evidence,
                               "repro": "python tools/qa/sim/run.py %s --steps %s" % (self.scenario, step)})

    # -- expectations ----------------------------------------------------------------------------
    def eval_times(self, peer, series, start, stop):
        times = [t for t in self.ticks.get(peer, []) if start <= t <= stop]
        times += [t for t in series.times if start <= t <= stop]
        return sorted(set(times))

    def expectation_rows(self):
        rows = []
        for record in self.expectations:
            if record.get("status") != "registered":
                if record.get("status") == "skipped_peers_absent":
                    continue  # every listed peer is outside this run's peer subset
                if (record.get("meta") or {}).get("optional") or (record.get("status") == "unresolved" and record.get("optional")):
                    continue  # e.g. a Khronos model that was not cached: declared optional by the scenario
                self.anomaly("harness", record["step"], record.get("peers", []),
                             "expectation %s could not be registered (%s)" % (record["exp_id"], record.get("status")),
                             {"error": record.get("error"), "probe": record.get("probe")})
                continue
            window = self.step_window.get(record["step"], {})
            step_end = window.get("t", self.end)
            # Regression/always checks stop at the step's end (+hold_s): later steps may legitimately change the row.
            horizon = step_end + record["hold_s"]
            if record["predicate"] == "agree":
                rows.extend(self._agree_rows(record, horizon))
            elif record["mode"] == "always":
                rows.extend(self._always_rows(record, horizon))
            else:
                rows.extend(self._eventually_rows(record, step_end, horizon))
        return pd.DataFrame(rows)

    def _value(self, record, series, t):
        value, digest, known = series.at(t)
        if not known:
            return predicates.MISSING, digest
        return predicates.derive(record["probe"], value), digest

    def _holds(self, record, value):
        return value is not predicates.MISSING and predicates.evaluate(record["predicate"], value, record.get("value"),
                                                                       record.get("path"))

    def _base_row(self, record, peer):
        return {"exp_id": record["exp_id"], "step": record["step"], "peer": peer, "probe": record["probe"],
                "target": short(record.get("target_key"), 40), "predicate": record["predicate"], "mode": record["mode"],
                "path": record.get("path"), "covers": record.get("covers"), "within_s": record["within_s"],
                "t0": record["t0"], "t0_rel": self.rel(record["t0"]), "severity": (record.get("meta") or {}).get("severity")}

    def _eventually_rows(self, record, step_end, horizon):
        rows = []
        matches = {}
        for peer in record["peers"]:
            series = self.series.get((peer, record["source"], record["target_key"]), Series())
            row = self._base_row(record, peer)
            pre_value, pre_digest = self._value(record, series, record["t0"])
            matched = None
            for t in self.eval_times(peer, series, record["t0"], self.end):
                value, digest = self._value(record, series, t)
                if self._holds(record, value):
                    matched = (t, digest)
                    break
            matches[peer] = matched
            row["matched"] = matched is not None
            row["matched_rel"] = self.rel(matched[0]) if matched else None
            row["latency_s"] = round(matched[0] - record["t0"], 3) if matched else None
            row["budget_ok"] = bool(matched and matched[0] - record["t0"] <= record["within_s"])
            row["cpu_mean"] = self.cpu_mean(record["t0"], matched[0] if matched else record["t0"] + record["within_s"])
            row["regressed_rel"] = None
            row["partial_states"] = 0
            if matched:
                for index, t in enumerate(series.times):
                    if matched[0] < t <= horizon:
                        value, _d = self._value(record, series, t)
                        if value is not predicates.MISSING and not self._holds(record, value):
                            row["regressed_rel"] = self.rel(t)
                            break
                row["_candidates"] = [(t, d) for t, d in zip(series.times, series.digests)
                                      if record["t0"] < t < matched[0] and d not in (pre_digest, matched[1])]
            rows.append(row)
        # A partial state is an intermediate digest the fastest-converging (reference) peer never had: a
        # replica-only view such as run 1's {sw_owner} bag during Clear Properties, not a sequential write.
        timed = [(m[0], p) for p, m in matches.items() if m]
        reference = min(timed)[1] if timed else None
        ref_series = self.series.get((reference, record["source"], record["target_key"]), Series()) if reference else Series()
        ref_digests = {d for t, d in zip(ref_series.times, ref_series.digests) if record["t0"] - 2 <= t <= horizon}
        for row in rows:
            candidates = row.pop("_candidates", [])
            partial = [] if row["peer"] == reference else [(t, d) for t, d in candidates if d not in ref_digests]
            row["partial_states"] = len({d for _t, d in partial})
            if partial:
                row["partial_first_rel"] = self.rel(partial[0][0])
                row["partial_span_s"] = round(self.origin + row["matched_rel"] - partial[0][0], 3)
            self._expectation_anomalies(record, row)
        return rows

    def _always_rows(self, record, horizon):
        rows = []
        for peer in record["peers"]:
            series = self.series.get((peer, record["source"], record["target_key"]), Series())
            row = self._base_row(record, peer)
            violations = []
            checks = [record["t0"]] + [t for t in series.times if record["t0"] < t <= horizon]
            for t in checks:
                value, _digest = self._value(record, series, t)
                if value is not predicates.MISSING and not self._holds(record, value):
                    violations.append(t)
            row.update(matched=not violations, budget_ok=not violations, latency_s=None, matched_rel=None,
                       violations=len(violations), first_violation_rel=self.rel(violations[0]) if violations else None,
                       horizon_rel=self.rel(horizon))
            rows.append(row)
            if violations:
                self.anomaly((record.get("meta") or {}).get("severity", "S1"), record["step"], [peer],
                             "%s: forbidden state observed on %s (%s %s %s)" % (
                                 record["exp_id"], peer, record["probe"], record["predicate"], record.get("path") or ""),
                             {"exp_id": record["exp_id"], "first_violation_rel_s": self.rel(violations[0]),
                              "violations": len(violations), "covers": record.get("covers"),
                              "target": record.get("target_key")})
        return rows

    def _agree_rows(self, record, horizon):
        peers = record["peers"]
        all_series = {p: self.series.get((p, record["source"], record["target_key"]), Series()) for p in peers}
        times = sorted({t for s in all_series.values() for t in s.times if t >= record["t0"]} |
                       {t for p in peers for t in self.ticks.get(p, []) if t >= record["t0"]})
        matched = None
        for t in times:
            values = {}
            for peer, series in all_series.items():
                value, _digest = self._value(record, series, t)
                values[peer] = value
            if predicates.agree(values, record.get("path")):
                matched = t
                break
        rows = []
        for peer in peers:
            row = self._base_row(record, peer)
            row.update(matched=matched is not None, matched_rel=self.rel(matched),
                       latency_s=round(matched - record["t0"], 3) if matched else None,
                       budget_ok=bool(matched and matched - record["t0"] <= record["within_s"]),
                       cpu_mean=self.cpu_mean(record["t0"], matched or record["t0"] + record["within_s"]))
            rows.append(row)
        if rows:
            self._expectation_anomalies(record, rows[0], peers_override=peers)
        return rows

    def _expectation_anomalies(self, record, row, peers_override=None):
        peers = peers_override or [row["peer"]]
        severity = record.get("meta", {}).get("severity") if isinstance(record.get("meta"), dict) else None
        evidence = {"exp_id": record["exp_id"], "probe": record["probe"], "target": record.get("target_key"),
                    "predicate": record["predicate"], "value": record.get("value"), "path": record.get("path"),
                    "within_s": record["within_s"], "latency_s": row.get("latency_s"), "covers": record.get("covers"),
                    "cpu_mean_pct": row.get("cpu_mean")}
        if not row["matched"]:
            self.anomaly(severity or "S2", record["step"], peers, "%s never held on %s (%s %s)" % (
                record["exp_id"], "/".join(peers), record["probe"], record["predicate"]), evidence)
        elif not row["budget_ok"]:
            self.anomaly(severity or ("S2" if row["latency_s"] > 2 * record["within_s"] else "S3"), record["step"], peers,
                         "%s held after %.1fs > budget %.0fs on %s" % (record["exp_id"], row["latency_s"],
                                                                       record["within_s"], "/".join(peers)), evidence)
        if row.get("regressed_rel") is not None:
            self.anomaly("S1", record["step"], peers, "%s held then regressed on %s at +%.1fs (flap/resurrection)" % (
                record["exp_id"], "/".join(peers), row["regressed_rel"]), evidence)
        # Intermediate states are reported for every row but only judged where the scenario declares the
        # write atomic (meta.atomic, e.g. Clear Properties): sequential writes legitimately replay in order.
        if row.get("partial_states") and (record.get("meta") or {}).get("atomic"):
            self.anomaly("S3", record["step"], peers, "%s: %d partial state(s) on %s before convergence (%.2fs)" % (
                record["exp_id"], row["partial_states"], "/".join(peers), row.get("partial_span_s") or 0), evidence)

    # -- actions ---------------------------------------------------------------------------------
    def action_frame(self):
        frame = pd.DataFrame(self.events)
        if not len(frame):
            return frame
        for column in ("verdict", "status", "fire_late_ms", "lane", "t_fire", "t_ack", "ok", "code", "error"):
            if column not in frame:
                frame[column] = None
        frame["rtt_ms"] = (frame["t_ack"] - frame["t_fire"]) * 1000.0
        return frame

    def action_anomalies(self, frame):
        if not len(frame):
            return
        for _i, event in frame.iterrows():
            verdict, status = event.get("verdict"), str(event.get("status"))
            if verdict == "accepted_but_expected_refusal":
                self.anomaly("S1", event["step"], [event["peer"]], "write accepted that the role forbids: %s %s/%s" % (
                    event["action_id"], event["via"], event["op"]),
                    {"action_id": event["action_id"], "http_status": event.get("http_status"),
                     "token_source": event.get("token_source"), "args": event.get("args_small")})
            elif verdict == "refused_but_expected_ok":
                self.anomaly("S2", event["step"], [event["peer"]], "permitted action refused: %s %s/%s (%s)" % (
                    event["action_id"], event["via"], event["op"], event.get("code")),
                    {"action_id": event["action_id"], "error": event.get("error"), "args": event.get("args_small")})
            elif verdict == "peer_down":
                self.anomaly("S1", event["step"], [event["peer"]], "peer did not answer during %s (crash?)" % event["action_id"],
                             {"action_id": event["action_id"], "error": event.get("error")})
            elif status.startswith("error") or event.get("code") == "harness_error":
                self.anomaly("harness", event["step"], [event["peer"]], "harness could not run %s: %s" % (
                    event["action_id"], event.get("error") or status), {"action_id": event["action_id"], "status": status})

    def barrier_table(self, frame):
        if not len(frame):
            return []
        rows = []
        barrier = frame[(frame["lane"] == "barrier") & frame["t_fire"].notna() & (frame["status"] == "done")]
        for step, group in barrier.groupby("step"):
            fires = group["t_fire"].astype(float)
            rows.append({"step": step, "actors": len(group), "spread_ms": round((fires.max() - fires.min()) * 1000, 3),
                         "late_ms_max": round(float(pd.to_numeric(group["fire_late_ms"], errors="coerce").max()), 3),
                         "rtt_ms_max": round(float(group["rtt_ms"].max()), 1)})
        return rows

    # -- cross-peer --------------------------------------------------------------------------------
    def divergence(self):
        episodes = []
        peers_alive = self._alive_windows()
        keys = collections.defaultdict(dict)
        for (peer, probe, target), series in self.series.items():
            if probe not in PEER_LOCAL_PROBES:
                keys[(probe, target)][peer] = series
        for (probe, target), by_peer in keys.items():
            if len(by_peer) < 2:
                continue
            times = sorted({t for s in by_peer.values() for t in s.times})
            open_since = {}
            for t in times + [self.end]:
                digests = {p: s.at(t)[1] for p, s in by_peer.items() if s.at(t)[1] is not None and peers_alive(p, t)}
                if len(digests) < 2:
                    continue
                counts = collections.Counter(digests.values())
                top = counts.most_common()
                majority = top[0][0] if len(top) == 1 or top[0][1] > top[1][1] else digests.get("owner", top[0][0])
                for peer, digest in digests.items():
                    if digest != majority and peer not in open_since:
                        open_since[peer] = t
                    elif digest == majority and peer in open_since:
                        start = open_since.pop(peer)
                        if t - start >= self.divergence_s:
                            episodes.append({"probe": probe, "target": target, "peer": peer, "start_rel": self.rel(start),
                                             "end_rel": self.rel(t), "duration_s": round(t - start, 3), "resolved": True})
            for peer, start in open_since.items():
                if self.end - start >= self.divergence_s:
                    episodes.append({"probe": probe, "target": target, "peer": peer, "start_rel": self.rel(start),
                                     "end_rel": None, "duration_s": round(self.end - start, 3), "resolved": False})
        for episode in episodes:
            step = self.step_at(self.origin + episode["start_rel"])
            self.anomaly("S2" if episode["resolved"] else "S1", step, [episode["peer"]],
                         "%s %s: %s disagreed with the majority for %.1fs%s" % (
                             episode["probe"], short(episode["target"], 30), episode["peer"], episode["duration_s"],
                             "" if episode["resolved"] else " (still diverged at run end)"), episode)
        return episodes

    def _alive_windows(self):
        alive = {peer: series for (peer, probe, _t), series in self.series.items() if probe == "alive"}

        def check(peer, t):
            series = alive.get(peer)
            if series is None:
                return True
            value, _d, _k = series.at(t)
            return not (isinstance(value, dict) and value.get("alive") is False)
        return check

    def step_at(self, t):
        current = "?"
        for record in self.steps:
            if record.get("event") == "start" and record["t"] <= t:
                current = record["step"]
        return current

    def restart_catchup(self, frame, rows):
        out = []
        if not len(frame):
            return out
        restarts = frame[(frame["op"] == "restart") & (frame["status"] == "done")]
        for _i, event in restarts.iterrows():
            peer_rows = rows[(rows["peer"] == event["peer"]) & (rows["step"] == event["step"])] if len(rows) else rows
            matched = peer_rows[peer_rows["matched"] == True]["matched_rel"] if len(peer_rows) else []  # noqa: E712
            caught = max(matched) if len(matched) and len(matched) == len(peer_rows) else None
            out.append({"step": event["step"], "peer": event["peer"], "restart_ack_rel": self.rel(event["t_ack"]),
                        "caught_up_rel": caught, "catchup_s": round(caught - self.rel(event["t_ack"]), 3) if caught is not None else None,
                        "expectations": len(peer_rows)})
        return out

    def asset_arrivals(self, rows):
        if not len(rows):
            return []
        assets = rows[rows["probe"] == "asset_blob"]
        out = []
        sizes = {r["exp_id"]: (r.get("meta") or {}).get("size_bytes") for r in self.expectations}
        for _i, row in assets.iterrows():
            size = sizes.get(row["exp_id"])
            out.append({"exp_id": row["exp_id"], "step": row["step"], "peer": row["peer"], "size_bytes": size,
                        "latency_s": row.get("latency_s"),
                        "mb_per_s": round(size / 1e6 / row["latency_s"], 3) if size and row.get("latency_s") else None})
        return out

    def fingerprint_diff(self):
        result = {}
        for name in sorted(os.listdir(self.run_dir)):
            if not (name.startswith("fingerprint-") and name.endswith(".json")):
                continue
            prints = load_json(os.path.join(self.run_dir, name), {})
            if len(prints) < 2:
                continue
            keys = sorted(set().union(*(p.get("rows", {}) for p in prints.values())))
            diverged = [{"key": k, **{p: v.get("rows", {}).get(k) for p, v in prints.items()}} for k in keys
                        if len({predicates.digest(v.get("rows", {}).get(k)) for v in prints.values()}) > 1]
            label = name[len("fingerprint-"):-5]
            result[label] = {"sha256": {p: v.get("sha256") for p, v in prints.items()}, "diverged": diverged,
                             "counts": {p: v.get("counts") for p, v in prints.items()}}
            if diverged and label == "final":
                self.anomaly("S1", "_final", list(prints), "final fingerprint differs across peers (%d keys)" % len(diverged),
                             {"keys": [d["key"] for d in diverged]})
        return result

    def prober_load(self):
        rows = []
        duration = max(1e-6, self.end - self.origin)
        by_peer = collections.defaultdict(lambda: collections.Counter())
        durations = collections.defaultdict(list)
        errors = collections.Counter()
        for obs in self.tick_rows:
            value = obs.get("value_small") or {}
            by_peer[obs["peer"]].update(value.get("req") or {})
            durations[obs["peer"]].append(value.get("dur"))
            errors[obs["peer"]] += value.get("err") or 0
        for peer in sorted(by_peer):
            counts = by_peer[peer]
            rows.append({"peer": peer, "ticks": len(durations[peer]), "req_per_s": round(sum(counts.values()) / duration, 2),
                         "hierarchy_per_s": round(counts.get("hierarchy", 0) / duration, 3),
                         "node_per_s": round(counts.get("node", 0) / duration, 2),
                         "ui_per_s": round(counts.get("ui", 0) / duration, 2),
                         "query_per_s": round((counts.get("query", 0) + counts.get("timeseries", 0)) / duration, 3),
                         "tick_p50_s": pct(durations[peer], 50), "tick_p95_s": pct(durations[peer], 95),
                         "errors": errors[peer]})
        return rows

    def cpu_correlation(self, rows):
        if not len(rows) or "cpu_mean" not in rows:
            return {}
        sample = rows[rows["latency_s"].notna() & rows["cpu_mean"].notna()][["latency_s", "cpu_mean"]].astype(float)
        if len(sample) < 3:
            return {"n": len(sample)}
        bins = pd.cut(sample["cpu_mean"], [0, 50, 80, 101], labels=["<50%", "50-80%", ">80%"])
        table = sample.groupby(bins, observed=False)["latency_s"].agg(["count", "median"]).reset_index()
        return {"n": len(sample), "pearson": round(float(sample.corr(method="pearson").iloc[0, 1]), 3),
                "spearman": round(float(sample.corr(method="spearman").iloc[0, 1]), 3),
                "bins": [{"cpu": str(r["cpu_mean"]), "count": int(r["count"]), "median_latency_s": None if pd.isna(r["median"]) else round(float(r["median"]), 3)}
                         for _i, r in table.iterrows()]}

    def convergence(self, rows):
        out = []
        if not len(rows):
            return pd.DataFrame(out)
        for (step, peer), group in rows[rows["mode"] == "eventually"].groupby(["step", "peer"], sort=False):
            window = self.step_window.get(step, {})
            t_actions = window.get("t_actions")
            all_met = bool(group["matched"].all())
            converged = group["matched_rel"].max() if all_met else None
            out.append({"step": step, "peer": peer, "expectations": len(group), "met": int(group["matched"].sum()),
                        "in_budget": int(group["budget_ok"].sum()), "fire_rel": self.rel(group["t0"].min()),
                        "converged_rel": converged,
                        "convergence_s": round(converged - self.rel(group["t0"].min()), 3) if converged is not None else None,
                        "budget_s": float(group["within_s"].max()), "step_actions_rel": self.rel(t_actions)})
        return pd.DataFrame(out)

    # -- output --------------------------------------------------------------------------------------
    def run(self):
        if self.meta.get("failure"):
            self.anomaly("harness", "_run", [p.get("name") for p in self.meta.get("peers", [])],
                         "run aborted before completion: %s" % str(self.meta["failure"])[:200],
                         {"failure": self.meta["failure"], "steps_started": [s["step"] for s in self.steps if s.get("event") == "start"]})
        rows = self.expectation_rows()
        frame = self.action_frame()
        self.action_anomalies(frame)
        episodes = self.divergence()
        conv = self.convergence(rows)
        barrier = self.barrier_table(frame)
        restarts = self.restart_catchup(frame, rows)
        assets = self.asset_arrivals(rows)
        fingerprints = self.fingerprint_diff()
        load = self.prober_load()
        cpu = self.cpu_correlation(rows)
        if len(rows):
            rows.drop(columns=["t0"]).to_csv(os.path.join(self.run_dir, "latency.csv"), index=False)
        else:
            pd.DataFrame().to_csv(os.path.join(self.run_dir, "latency.csv"), index=False)
        conv.to_csv(os.path.join(self.run_dir, "convergence.csv"), index=False)
        with open(os.path.join(self.run_dir, "anomalies.json"), "w", encoding="utf-8") as handle:
            json.dump(self.anomalies, handle, indent=1, default=str)
        timeline_svg.write(os.path.join(self.run_dir, "timeline.svg"), self, rows, conv, frame)
        summary = self.write_report(rows, frame, conv, episodes, barrier, restarts, assets, fingerprints, load, cpu)
        with open(os.path.join(self.run_dir, "summary.json"), "w", encoding="utf-8") as handle:
            json.dump(summary, handle, indent=1, default=str)
        return summary

    def write_report(self, rows, frame, conv, episodes, barrier, restarts, assets, fingerprints, load, cpu):
        eventually = rows[rows["mode"] == "eventually"] if len(rows) else rows
        latencies = list(eventually["latency_s"].dropna()) if len(eventually) else []
        host = self.host[self.host["peer"] == "_host"] if len(self.host) else self.host
        cpu_series = pd.to_numeric(host["cpu_total_pct"], errors="coerce") if len(host) else pd.Series(dtype=float)
        severities = collections.Counter(a["severity_hint"] for a in self.anomalies)
        summary = {"scenario": self.scenario, "run_dir": self.run_dir, "duration_s": round(self.end - self.origin, 1),
                   "expectations": int(len(rows)), "met": int(rows["matched"].sum()) if len(rows) else 0,
                   "in_budget": int(rows["budget_ok"].sum()) if len(rows) else 0,
                   "latency_p50_s": pct(latencies, 50), "latency_p95_s": pct(latencies, 95),
                   "latency_max_s": max(latencies) if latencies else None,
                   "cpu_mean_pct": None if not len(cpu_series) else round(float(cpu_series.mean()), 1),
                   "cpu_max_pct": None if not len(cpu_series) else round(float(cpu_series.max()), 1),
                   "anomalies": len(self.anomalies), "by_severity": dict(severities),
                   "actions": int(len(frame)), "report": os.path.join(self.run_dir, "report.md")}
        lines = ["# Simulation report: %s" % self.scenario, ""]
        meta = self.meta
        lines += ["| field | value |", "|---|---|",
                  "| run dir | `%s` |" % self.run_dir,
                  "| scenario file | `%s` |" % meta.get("scenario_path"),
                  "| stage | `%s` (matches source: %s) |" % (meta.get("stage"), meta.get("stage_matches_source")),
                  "| peers | %s |" % ", ".join("%s (%s)" % (p["name"], p.get("role")) for p in meta.get("peers", [])),
                  "| steps | %s |" % ", ".join(meta.get("steps", [])),
                  "| duration | %s s |" % summary["duration_s"],
                  "| cadence | %s |" % json.dumps(meta.get("cadence")),
                  "| host CPU mean / max | %s %% / %s %% |" % (fmt(summary["cpu_mean_pct"], 1), fmt(summary["cpu_max_pct"], 1)),
                  "| expectations met / in budget / total | %d / %d / %d |" % (summary["met"], summary["in_budget"], summary["expectations"]),
                  "| replication latency p50 / p95 / max | %s / %s / %s s |" % (fmt(summary["latency_p50_s"]), fmt(summary["latency_p95_s"]), fmt(summary["latency_max_s"])),
                  "| anomalies | %d %s |" % (len(self.anomalies), json.dumps(dict(severities))), ""]
        if meta.get("cpu_note"):
            lines += ["> " + meta["cpu_note"], ""]
        lines += ["## Prober load (read rates next to the latencies they measure)", "",
                  "Latency resolution is one prober round (`rest_ms`); hierarchy polling is capped at 0.5 Hz (QA-0125).", "",
                  "| peer | ticks | req/s | node/s | hierarchy/s | ui/s | query/s | tick p50 s | tick p95 s | 5xx/429 |",
                  "|---|---|---|---|---|---|---|---|---|---|"]
        for r in load:
            lines.append("| %s | %d | %s | %s | %s | %s | %s | %s | %s | %d |" % (
                r["peer"], r["ticks"], r["req_per_s"], r["node_per_s"], r["hierarchy_per_s"], r["ui_per_s"],
                r["query_per_s"], fmt(r["tick_p50_s"], 3), fmt(r["tick_p95_s"], 3), r["errors"]))
        lines += ["", "## Barrier alignment (measured, not assumed)", ""]
        if barrier:
            lines += ["| step | actors | fire spread ms | max late ms | max RTT ms |", "|---|---|---|---|---|"]
            lines += ["| %s | %d | %s | %s | %s |" % (b["step"], b["actors"], b["spread_ms"], b["late_ms_max"], b["rtt_ms_max"]) for b in barrier]
        else:
            lines.append("No barrier steps ran.")
        lines += ["", "## Per-step convergence", "", "| step | peer | met/total | in budget | fire +s | converged +s | convergence s | budget s |",
                  "|---|---|---|---|---|---|---|---|"]
        for _i, c in conv.iterrows() if len(conv) else []:
            lines.append("| %s | %s | %d/%d | %d | %s | %s | %s | %s |" % (
                c["step"], c["peer"], c["met"], c["expectations"], c["in_budget"], fmt(c["fire_rel"], 1),
                fmt(c["converged_rel"], 1), fmt(c["convergence_s"]), fmt(c["budget_s"], 0)))
        lines += ["", "## Expectations (every peer)", "",
                  "| exp | step | peer | probe | target | predicate | path | latency s | budget s | met | in budget | notes |",
                  "|---|---|---|---|---|---|---|---|---|---|---|---|"]
        for _i, r in rows.iterrows() if len(rows) else []:
            notes = []
            if r.get("mode") == "always":
                notes.append("always: %d violation(s)" % int(r.get("violations") or 0))
            if pd.notna(r.get("regressed_rel")):
                notes.append("regressed +%s" % fmt(float(r["regressed_rel"]), 1))
            if pd.notna(r.get("partial_states")) and r.get("partial_states"):
                notes.append("partial states %d" % int(r["partial_states"]))
            if isinstance(r.get("covers"), str):
                notes.append(r["covers"])
            lines.append("| %s | %s | %s | %s | `%s` | %s | %s | %s | %s | %s | %s | %s |" % (
                r["exp_id"], r["step"], r["peer"], r["probe"], r["target"], r["predicate"],
                r["path"] if isinstance(r.get("path"), str) else "",
                fmt(r.get("latency_s")), fmt(r["within_s"], 0), "yes" if r["matched"] else "**no**",
                "yes" if r["budget_ok"] else "**no**", "; ".join(notes)))
        lines += ["", "## Refusal correctness", ""]
        if len(frame):
            checked = frame[frame["expect"].isin(["ok", "refused"]) & (frame["status"] == "done")]
            table = checked.groupby(["expect", "verdict"]).size().reset_index(name="n")
            lines += ["| expected | verdict | actions |", "|---|---|---|"]
            lines += ["| %s | %s | %d |" % (r["expect"], r["verdict"], r["n"]) for _i, r in table.iterrows()]
            skipped = frame[frame["status"].astype(str).str.startswith("skipped")]
            if len(skipped):
                lines += ["", "Skipped actions: %d (%s)." % (len(skipped), ", ".join(sorted(set(skipped["status"]))))]
            lines += ["", "Action round-trip: p50 %s ms, p95 %s ms over %d actions." % (
                fmt(pct(list(frame["rtt_ms"].dropna()), 50), 0), fmt(pct(list(frame["rtt_ms"].dropna()), 95), 0), len(frame))]
        lines += ["", "## Divergence episodes (peer vs majority > %.0f s)" % self.divergence_s, ""]
        if episodes:
            lines += ["| probe | target | peer | start +s | end +s | duration s |", "|---|---|---|---|---|---|"]
            lines += ["| %s | `%s` | %s | %s | %s | %s |" % (e["probe"], short(e["target"], 30), e["peer"], e["start_rel"],
                                                          fmt(e["end_rel"]), e["duration_s"]) for e in episodes]
        else:
            lines.append("None.")
        lines += ["", "## Restart catch-up", ""]
        if restarts:
            lines += ["| step | peer | restart ack +s | caught up +s | catch-up s | expectations |", "|---|---|---|---|---|---|"]
            lines += ["| %s | %s | %s | %s | %s | %d |" % (r["step"], r["peer"], r["restart_ack_rel"], fmt(r["caught_up_rel"]),
                                                         fmt(r["catchup_s"]), r["expectations"]) for r in restarts]
        else:
            lines.append("No restarts in this run.")
        lines += ["", "## Asset arrival vs size", ""]
        if assets:
            lines += ["| exp | peer | bytes | arrival s | MB/s |", "|---|---|---|---|---|"]
            lines += ["| %s | %s | %s | %s | %s |" % (a["exp_id"], a["peer"], a["size_bytes"], fmt(a["latency_s"]),
                                                   fmt(a["mb_per_s"], 3)) for a in assets]
        else:
            lines.append("No asset expectations in this run.")
        lines += ["", "## Latency vs host CPU", ""]
        if cpu.get("n", 0) >= 3:
            lines += ["n=%d, Pearson %s, Spearman %s." % (cpu["n"], cpu["pearson"], cpu["spearman"]), "",
                      "| host CPU during wait | samples | median latency s |", "|---|---|---|"]
            lines += ["| %s | %d | %s |" % (b["cpu"], b["count"], fmt(b["median_latency_s"])) for b in cpu["bins"]]
        else:
            lines.append("Too few latency samples with host CPU coverage (%d)." % cpu.get("n", 0))
        lines += ["", "## Fingerprints", ""]
        for label, data in fingerprints.items():
            lines.append("- `%s`: %s; diverged keys: %d %s" % (
                label, ", ".join("%s=%s" % (p, (s or "-")[:12]) for p, s in data["sha256"].items()), len(data["diverged"]),
                [d["key"] for d in data["diverged"]][:20]))
        if not fingerprints:
            lines.append("No fingerprints recorded.")
        shots = [e for e in self.events if e.get("op") == "checkpoint"]
        lines += ["", "## Checkpoints (vision review input)", ""]
        if shots:
            lines += ["| step | peer | capture | file | visual expectation |", "|---|---|---|---|---|"]
            for e in shots:
                path = (e.get("result_small") or {}).get("path") if isinstance(e.get("result_small"), dict) else None
                lines.append("| %s | %s | %s | %s | %s |" % (e["step"], e["peer"], e.get("capture"),
                                                              "`%s`" % path if path else "failed: " + str(e.get("error"))[:80],
                                                              e.get("expect_visual") or ""))
        else:
            lines.append("No checkpoints.")
        lines += ["", "## Anomalies (%d, complete list; also anomalies.json)" % len(self.anomalies), ""]
        if self.anomalies:
            lines += ["| id | severity | step | peers | title |", "|---|---|---|---|---|"]
            lines += ["| %s | %s | %s | %s | %s |" % (a["id"], a["severity_hint"], a["step"], ",".join(a["peers"]) if isinstance(a["peers"], list) else a["peers"],
                                                   a["title"].replace("|", "/")) for a in self.anomalies]
        else:
            lines.append("None.")
        lines += ["", "Timeline: `timeline.svg`; raw: `events.jsonl`, `observations.jsonl`, `expectations.jsonl`, `host.csv`; "
                  "tables: `latency.csv`, `convergence.csv`.", ""]
        with open(os.path.join(self.run_dir, "report.md"), "w", encoding="utf-8") as handle:
            handle.write("\n".join(lines))
        return summary


def analyze(run_dir, divergence_s=DIVERGENCE_S):
    return Analysis(os.path.abspath(run_dir), divergence_s).run()


def main(argv=None):
    parser = argparse.ArgumentParser(description="Analyze one tools/qa/sim run directory.")
    parser.add_argument("run_dir")
    parser.add_argument("--divergence-s", type=float, default=DIVERGENCE_S)
    args = parser.parse_args(argv)
    summary = analyze(args.run_dir, args.divergence_s)
    print(json.dumps({k: summary[k] for k in ("report", "expectations", "met", "in_budget", "latency_p50_s",
                                               "latency_p95_s", "anomalies", "by_severity")}, default=str))
    return 0


if __name__ == "__main__":
    sys.exit(main())
