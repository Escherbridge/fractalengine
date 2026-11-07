"""Per-peer probers at scenario cadences + a host CPU/RAM sampler; observations are logged on change; see tools/qa/AGENTS.md §sim."""
import concurrent.futures
import json
import os
import subprocess
import threading
import time

import predicates
from runtime import small
from scenario import source_probe, target_key

# GET /nodes/{id} returns position as GeoJSON {coordinates:[x,z]} + elevation and a rotation whose shape varies;
# the canonical [x,y,z] transform is the separate `transform` probe (GET /nodes/{id}/transform).
NODE_FIELDS = ("display_name", "position", "elevation", "rotation", "scale", "properties", "asset_id", "petal_id",
               "view", "edit_seq")
POOL_WORKERS = 4

HOST_SAMPLER_PS1 = r"""
param([string]$Csv, [string]$PidFile, [int]$IntervalMs = 2000, [int]$ParentPid, [int]$MaxSeconds = 3600)
$ErrorActionPreference = 'SilentlyContinue'
$cores = [Environment]::ProcessorCount
'wall_ms,cpu_total_pct,free_ram_mb,peer,peer_pid,peer_cpu_pct' | Set-Content -LiteralPath $Csv -Encoding ascii
$previous = @{}
$previousAt = [DateTime]::UtcNow
$start = [DateTime]::UtcNow
while (([DateTime]::UtcNow - $start).TotalSeconds -lt $MaxSeconds) {
    Start-Sleep -Milliseconds $IntervalMs
    if (-not (Get-Process -Id $ParentPid)) { break }
    $now = [DateTime]::UtcNow
    $elapsed = ($now - $previousAt).TotalMilliseconds
    $previousAt = $now
    $total = (Get-CimInstance Win32_PerfFormattedData_PerfOS_Processor -Filter "Name='_Total'").PercentProcessorTime
    $free = [math]::Round((Get-CimInstance Win32_OperatingSystem).FreePhysicalMemory / 1024)
    $wall = [DateTimeOffset]::UtcNow.ToUnixTimeMilliseconds()
    $lines = New-Object System.Collections.Generic.List[string]
    $lines.Add("$wall,$total,$free,_host,,")
    foreach ($entry in @(Get-Content -LiteralPath $PidFile)) {
        $parts = $entry -split '='
        if ($parts.Count -ne 2) { continue }
        $name = $parts[0]; $peerPid = [int]$parts[1]
        $process = Get-Process -Id $peerPid
        if (-not $process) { $lines.Add("$wall,$total,$free,$name,$peerPid,"); continue }
        $cpuMs = $process.TotalProcessorTime.TotalMilliseconds
        $key = "$name/$peerPid"
        $value = ''
        if ($previous.ContainsKey($key)) { $value = [math]::Round(($cpuMs - $previous[$key]) / $elapsed * 100 / $cores, 2) }
        $previous[$key] = $cpuMs
        $lines.Add("$wall,$total,$free,$name,$peerPid,$value")
    }
    $lines | Add-Content -LiteralPath $Csv -Encoding ascii
}
"""


class HostSampler:
    """One background PowerShell writing host.csv (CPU total, per-peer CPU, free RAM); stopped by its exact handle."""

    def __init__(self, run_dir, interval_ms, max_seconds):
        self.run_dir = run_dir
        self.csv = os.path.join(run_dir, "host.csv")
        self.pid_file = os.path.join(run_dir, "host-pids.txt")
        self.interval_ms = interval_ms
        self.max_seconds = max_seconds
        self.process = None

    def write_pids(self, pids):
        pending = self.pid_file + ".pending"
        with open(pending, "w", encoding="ascii") as handle:
            handle.write("".join("%s=%d\n" % (name, pid) for name, pid in sorted(pids.items())))
        os.replace(pending, self.pid_file)

    def start(self, pids):
        self.write_pids(pids)
        script = os.path.join(self.run_dir, "host_sampler.ps1")
        with open(script, "w", encoding="utf-8") as handle:
            handle.write(HOST_SAMPLER_PS1)
        self.process = subprocess.Popen(
            ["powershell", "-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-File", script,
             "-Csv", self.csv, "-PidFile", self.pid_file, "-IntervalMs", str(self.interval_ms),
             "-ParentPid", str(os.getpid()), "-MaxSeconds", str(int(self.max_seconds))],
            stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, creationflags=getattr(subprocess, "CREATE_NO_WINDOW", 0))

    def stop(self):
        if self.process and self.process.poll() is None:
            self.process.terminate()
            try:
                self.process.wait(10)
            except subprocess.TimeoutExpired:
                self.process.kill()
                self.process.wait(10)


class Prober:
    """One thread per peer polling the scenario's (probe, target) set at fixed cadences."""

    def __init__(self, scenario, clients, ctx, clock, recorder, store, cadence, steps=None):
        self.clients = clients
        self.ctx = ctx
        self.clock = clock
        self.recorder = recorder
        self.store = store
        self.cadence = cadence
        self.stop_event = threading.Event()
        self.threads = []
        self.templates = self._collect_templates({**scenario, "steps": steps if steps is not None else scenario["steps"]})

    @staticmethod
    def _collect_templates(scenario):
        """(first step index, source probe, target template); watch entries are active from the start."""
        templates = [(-1, w["probe"], w["target"]) for w in scenario.get("watch", [])]
        templates.append((-1, "role", "$fixture.verse_id"))
        for index, step in enumerate(scenario["steps"]):
            for expect in step.get("expect", []):
                templates.append((index, expect["probe"], expect["target"]))
        unique, seen = [], set()
        for index, probe, target in templates:
            key = (source_probe(probe), json.dumps(target, sort_keys=True))
            if key not in seen:
                seen.add(key)
                unique.append((index, source_probe(probe), target))
        return unique

    def resolved_targets(self):
        """{source probe: [(target, key)]} for every template whose $-references are bound by now."""
        out = {}
        current = self.cadence.step_index()
        for first_step, probe, template in self.templates:
            if first_step > current:
                continue
            target = self.ctx.try_resolve(template)
            if target in (None, "", [], {}):
                continue
            key = target_key(target)
            bucket = out.setdefault(probe, [])
            if all(existing_key != key for _t, existing_key in bucket):
                bucket.append((target, key))
        return out

    def start(self):
        for name, client in self.clients.items():
            thread = threading.Thread(target=self._loop, args=(name, client), daemon=True, name="probe-" + name)
            self.threads.append(thread)
            thread.start()

    def stop(self):
        self.stop_event.set()
        for thread in self.threads:
            thread.join(timeout=15)

    def _loop(self, name, client):
        previous = {}
        due = {}
        pool = concurrent.futures.ThreadPoolExecutor(max_workers=POOL_WORKERS)
        try:
            while not self.stop_event.is_set():
                round_start = self.clock.now()
                counts = {}
                errors = 0
                try:
                    errors = self._round(name, client, previous, due, pool, counts, round_start)
                except Exception as error:  # noqa: BLE001 - a prober defect is data, the loop continues
                    self.recorder.write("observations", {"t": round_start, "peer": name, "probe": "_error",
                                                         "target": "", "digest": "", "value_small": repr(error)[:300]})
                t_done = self.clock.now()
                self.store.tick(name, t_done)
                self.recorder.write("observations", {"t": t_done, "peer": name, "probe": "_tick", "target": "",
                                                     "digest": "", "value_small": {"dur": round(t_done - round_start, 4),
                                                                                   "req": counts, "err": errors}})
                interval = self.cadence.get("rest_ms") / 1000.0
                self.stop_event.wait(max(0.0, interval - (self.clock.now() - round_start)))
        finally:
            pool.shutdown(wait=False)

    def _is_due(self, due, family, cadence_key, now):
        interval = self.cadence.get(cadence_key)
        if interval == 0:
            return False
        if now >= due.get(family, 0.0):
            due[family] = now + interval / 1000.0
            return True
        return False

    def _emit(self, name, previous, probe, key, value, t):
        dig = predicates.digest(value)
        self.store.update(name, probe, key, t, value)
        if previous.get((probe, key)) != dig:
            previous[(probe, key)] = dig
            # Hierarchy views list whole petals (city-lite: 60+ rows), so they get a larger inline budget.
            limit = 65536 if probe == "hierarchy" else 4096
            self.recorder.write("observations", {"t": t, "peer": name, "probe": probe, "target": key,
                                                 "digest": dig, "value_small": small(value, limit)})

    def _round(self, name, client, previous, due, pool, counts, now):
        targets = self.resolved_targets()
        verse = self.ctx.try_resolve("$fixture.verse_id")
        token, _source = client.verse_token(verse)
        jobs = []

        def get(kind, path):
            counts[kind] = counts.get(kind, 0) + 1
            return client.send("GET", path, None, token, timeout=8.0)

        for target, key in targets.get("node", []):
            jobs.append(("node", key, pool.submit(self._node, get, target)))
        for target, key in targets.get("transform", []):
            jobs.append(("transform", key, pool.submit(get, "transform", "/api/v1/nodes/%s/transform" % target)))
        for target, key in targets.get("groups", []):
            jobs.append(("groups", key, pool.submit(get, "groups", "/api/v1/petals/%s/visibility-groups" % target)))
        for target, key in targets.get("get", []):
            jobs.append(("get", key, pool.submit(get, "get", str(target))))
        for target, key in targets.get("terrain", []):
            jobs.append(("terrain", key, pool.submit(get, "terrain", "/api/v1/petals/%s/terrain" % target)))
        if targets.get("hierarchy") and self._is_due(due, "hierarchy", "hierarchy_ms", now):
            jobs.append(("hierarchy", None, pool.submit(get, "hierarchy", "/api/v1/hierarchy")))
        if (targets.get("nav") or targets.get("ui")) and self._is_due(due, "ui", "ui_ms", now):
            debug = client.debug_token()
            counts["ui"] = counts.get("ui", 0) + 1
            jobs.append(("ui_state", None, pool.submit(client.send, "GET", "/api/v1/debug/ui-state", None, debug, 8.0)))
        if (targets.get("query") or targets.get("timeseries")) and self._is_due(due, "query", "query_ms", now):
            for target, key in targets.get("query", []):
                counts["query"] = counts.get("query", 0) + 1
                jobs.append(("query", key, pool.submit(client.send, "POST", "/api/v1/query", {"sql": target}, token, 15.0)))
            for target, key in targets.get("timeseries", []):
                counts["timeseries"] = counts.get("timeseries", 0) + 1
                jobs.append(("timeseries", key, pool.submit(client.mcp, "query_timeseries", target, 20.0)))
        errors = 0
        alive = None
        for kind, key, future in jobs:
            outcome = future.result()
            t = self.clock.now()
            if outcome.get("code") == "not_running":
                alive = False
                continue
            alive = True if alive is None else alive
            if outcome.get("status", 0) >= 500 or outcome.get("status", 0) == 429:
                errors += 1
                continue
            if kind == "node":
                self._emit(name, previous, "node", key, outcome.get("value"), t)
            elif kind == "transform":
                data = outcome.get("data") if outcome.ok else None
                value = predicates.normalize({k: data.get(k) for k in ("position", "rotation", "scale")}) if isinstance(data, dict) else {"absent": True, "status": outcome.get("status")}
                self._emit(name, previous, "transform", key, value, t)
            elif kind == "groups":
                groups = (outcome.get("data") or {}).get("groups") if outcome.ok else None
                value = sorted(({"id": g.get("id"), "name": g.get("name"), "visible": g.get("visible")}
                                for g in groups or [] if isinstance(g, dict)), key=lambda g: str(g["name"])) if outcome.ok else {"absent": True, "status": outcome.get("status")}
                self._emit(name, previous, "groups", key, value, t)
            elif kind == "terrain":
                data = outcome.get("data") if outcome.ok else None
                terrain = data.get("terrain") if isinstance(data, dict) else None
                value = {"sha": predicates.digest(data), "null": terrain is None,
                         "tileset_hexon_uris": terrain.get("tileset_hexon_uris") if isinstance(terrain, dict) else None} if outcome.ok else {"absent": True, "status": outcome.get("status")}
                self._emit(name, previous, "terrain", key, value, t)
            elif kind == "get":
                value = predicates.normalize(outcome.get("data")) if outcome.ok else {"absent": True, "status": outcome.get("status")}
                self._emit(name, previous, "get", key, value, t)
            elif kind == "hierarchy":
                for target, hkey in targets.get("hierarchy", []):
                    self._emit(name, previous, "hierarchy", hkey, self._hierarchy_view(outcome, target, verse), t)
            elif kind == "ui_state":
                data = outcome.get("data") if outcome.ok else None
                if isinstance(data, dict):
                    for target, ukey in targets.get("nav", []):
                        self._emit(name, previous, "nav", ukey, data.get("navigation"), t)
                    for target, ukey in targets.get("ui", []):
                        self._emit(name, previous, "ui", ukey, {"notice": data.get("notice"), "status": data.get("status"),
                                                               "open_windows": data.get("open_windows")}, t)
            elif kind == "query":
                rows = outcome.get("data") if outcome.ok else None
                value = {"count": len(rows), "digest": predicates.digest(rows), "rows": small(rows, 2048)} if isinstance(rows, list) else {"absent": True, "status": outcome.get("status"), "code": outcome.get("code")}
                self._emit(name, previous, "query", key, value, t)
            elif kind == "timeseries":
                data = outcome.get("data") if outcome.ok else None
                value = {"digest": predicates.digest(data), "small": small(data, 2048)} if outcome.ok else {"absent": True, "code": outcome.get("code")}
                self._emit(name, previous, "timeseries", key, value, t)
        t = self.clock.now()
        for target, key in targets.get("role", []):
            self._emit(name, previous, "role", key, client.role_in(target), t)
        for target, key in targets.get("asset_blob", []):
            path = client.blob_path(str(target))
            try:
                size = os.path.getsize(path)
                value = {"present": True, "size": size}
            except OSError:
                value = {"absent": True}
            self._emit(name, previous, "asset_blob", key, value, t)
        if alive is not None or targets.get("alive"):
            self._emit(name, previous, "alive", "peer", {"alive": bool(alive)} if alive is not None else {"alive": True}, t)
        return errors

    @staticmethod
    def _node(get, node_id):
        outcome = get("node", "/api/v1/nodes/%s" % node_id)
        if outcome.get("code") == "not_running":
            return outcome
        data = outcome.get("data") if outcome.ok else None
        if not isinstance(data, dict):
            outcome["value"] = {"absent": True, "status": outcome.get("status"), "code": outcome.get("code")}
            return outcome
        outcome["value"] = predicates.normalize({k: data.get(k) for k in NODE_FIELDS})
        return outcome

    @staticmethod
    def _hierarchy_view(outcome, target, verse):
        if not outcome.ok or not isinstance(outcome.get("data"), list):
            return {"absent": True, "status": outcome.get("status")}
        for verse_row in outcome["data"]:
            if not isinstance(verse_row, dict) or verse_row.get("id") != verse:
                continue
            if target == verse:
                return {"fractals": sorted(f.get("name") for f in verse_row.get("fractals", [])),
                        "petals": sorted("%s/%s" % (f.get("name"), p.get("name"))
                                         for f in verse_row.get("fractals", []) for p in f.get("petals", []))}
            for fractal in verse_row.get("fractals", []):
                for petal in fractal.get("petals", []):
                    if petal.get("id") == target:
                        nodes = petal.get("nodes", [])
                        return {"names": sorted(n.get("name") for n in nodes),
                                "nodes": {n["id"]: {"name": n.get("name"), "asset_path": n.get("asset_path"),
                                                    "has_asset": n.get("has_asset")} for n in nodes}}
        return {"absent": True, "status": 404}
