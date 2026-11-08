"""Run one simulation scenario end to end: stage check -> resource gate -> peers -> join -> drive + probe -> stop -> analyze; see tools/qa/AGENTS.md §sim."""
import argparse
import ctypes
import hashlib
import json
import math
import os
import shutil
import subprocess
import sys
import tempfile
import threading
import time
from ctypes import wintypes

HERE = os.path.dirname(os.path.abspath(__file__))
QA = os.path.dirname(HERE)
REPO_ROOT = os.path.abspath(os.path.join(QA, "..", ".."))
sys.path.insert(0, HERE)
sys.path.insert(0, QA)

import analyze  # noqa: E402
import scenario as scenario_mod  # noqa: E402
from client import PeerClient  # noqa: E402
from driver import Driver, select_steps  # noqa: E402
from prober import HostSampler, Prober  # noqa: E402
from runtime import Cadence, Clock, LiveStore, Recorder  # noqa: E402

RUNS_ROOT = os.path.join(REPO_ROOT, "qa", "sim", "runs")
TMP_ROOT = os.path.join(REPO_ROOT, ".qa", "tmp")  # PowerShell output spools: the harness writes only under .qa/ and qa/
STAGE_ROOT = os.path.join(REPO_ROOT, ".qa", "stage")
CURRENT_STAGE = "current"  # the stable launch directory: a real copy of the stage a run uses, never a link (QA-0153)
FILE_ATTRIBUTE_REPARSE_POINT = 0x400
STAGE_COPY_MARGIN_GB = 2.0  # free space kept beyond the files a publish copies
PEERS_POINTER = os.path.join(REPO_ROOT, ".qa", "peers-current")
ASSET_MANIFESTS = {"generated": os.path.join(REPO_ROOT, ".qa", "assets", "generated", "manifest.json"),
                   "khronos": os.path.join(REPO_ROOT, ".qa", "assets", "khronos", "manifest.json")}
# Disjoint 3-logical-core masks per peer (checkpoint steer, QA-0119); the rest stays for the harness and desktop.
AFFINITY_MASKS = [0x007, 0x038, 0x1C0, 0xE00]
RAM_PER_PEER_GB = 0.9
JOIN_ATTEMPTS = 2
JOIN_RETRY_PAUSE_S = 5.0
VERSE_NAMES = {"basic": "QA Basic", "terrain": "QA Terrain"}
CREATE_NO_WINDOW = getattr(subprocess, "CREATE_NO_WINDOW", 0)


def log(message):
    print(time.strftime("%H:%M:%S ") + message, flush=True)


def powershell(command, timeout=900, env=None):
    """Run a PowerShell -Command string; returns (code, stdout, stderr).

    Output goes to temp files and we wait on process exit, not pipe EOF: peers.ps1/launch.ps1 start
    fractalengine.exe children that inherit the handles and would hold a pipe open for their lifetime.
    """
    os.makedirs(TMP_ROOT, exist_ok=True)
    with tempfile.TemporaryFile(dir=TMP_ROOT) as out, tempfile.TemporaryFile(dir=TMP_ROOT) as err:
        process = subprocess.Popen(["powershell", "-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass",
                                    "-Command", command], stdin=subprocess.DEVNULL, stdout=out, stderr=err,
                                   cwd=REPO_ROOT, env=env, creationflags=CREATE_NO_WINDOW)
        try:
            code = process.wait(timeout)
        except subprocess.TimeoutExpired:
            process.kill()
            process.wait(30)
            raise
        out.seek(0)
        err.seek(0)
        return code, out.read().decode("utf-8", "replace"), err.read().decode("utf-8", "replace")


def ps_quote(text):
    return "'" + str(text).replace("'", "''") + "'"


def exit_label(code):
    """'3221225794 (0xC0000142)' for a Windows crash status, else the plain number."""
    return "%d (0x%08X)" % (code, code & 0xFFFFFFFF) if code < 0 or code >= 0x80000000 else str(code)


def died_at_spawn(code):
    """True for a crash status (NTSTATUS 0xC0000000+ or a signal): the helper died before any GUI work, so a retry is clean."""
    return code < 0 or code >= 0xC0000000


# -- stage + resource gate ------------------------------------------------------------------------
def newest_stage():
    stages = []
    for name in os.listdir(STAGE_ROOT) if os.path.isdir(STAGE_ROOT) else []:
        if name == CURRENT_STAGE:
            continue  # the launch mirror, never the provenance
        manifest = os.path.join(STAGE_ROOT, name, "stage.json")
        if os.path.isfile(manifest) and os.path.isfile(os.path.join(STAGE_ROOT, name, "fractalengine.exe")):
            stages.append((os.path.getmtime(manifest), name))
    if not stages:
        return None, None
    name = max(stages)[1]
    with open(os.path.join(STAGE_ROOT, name, "stage.json"), encoding="utf-8-sig") as handle:
        return name, json.load(handle)


def foreign_cargo_running():
    """True when a cargo/rustc runs that is not rust-analyzer's background check (its grandparent)."""
    code, out, _err = powershell(
        "$all = Get-CimInstance Win32_Process; $by = @{}; foreach ($p in $all) { $by[[int]$p.ProcessId] = $p }; "
        "foreach ($p in $all | Where-Object { $_.Name -in 'cargo.exe','rustc.exe' }) { "
        "$parent = $by[[int]$p.ParentProcessId]; $grand = if ($parent) { $by[[int]$parent.ParentProcessId] } else { $null }; "
        "$ra = ($parent -and $parent.Name -eq 'rust-analyzer.exe') -or ($grand -and $grand.Name -eq 'rust-analyzer.exe'); "
        "if (-not $ra) { 'FOREIGN ' + $p.ProcessId } }", timeout=60)
    return code == 0 and "FOREIGN" in out


def ensure_stage(args):
    name, manifest = newest_stage()
    head = subprocess.run(["git", "rev-parse", "HEAD"], capture_output=True, text=True, cwd=REPO_ROOT).stdout.strip()
    if args.stage:
        return args.stage, args.allow_stale_stage
    if name and (manifest.get("source_commit") == head or args.allow_stale_stage):
        return name, args.allow_stale_stage
    if foreign_cargo_running():
        raise SystemExit("HEAD moved since stage %s but another cargo build is running; rerun later or pass "
                         "--allow-stale-stage to reuse it with limited provenance" % name)
    log("staging HEAD %s (stage.ps1 -Jobs 4)" % head[:8])
    env = dict(os.environ, CARGO_PROFILE_DEV_DEBUG="0", RUST_MIN_STACK="134217728")
    code, out, err = powershell("& %s -Jobs 4" % ps_quote(os.path.join(QA, "stage.ps1")), timeout=7200, env=env)
    if code != 0:
        raise SystemExit("stage.ps1 failed:\n" + (out + err)[-2000:])
    name, _manifest = newest_stage()
    return name, False


def is_reparse_point(path):
    """True for a junction or symlink: Windows reports a link's target as a process image path, and the QA scripts refuse links."""
    try:
        return bool(os.lstat(path).st_file_attributes & FILE_ATTRIBUTE_REPARSE_POINT)
    except (OSError, AttributeError):
        return False


def stage_file_table(directory):
    """{relative path: (size, mtime_ns)} of every file under a stage directory; a link anywhere inside is refused."""
    table = {}
    for root, dirs, files in os.walk(directory):
        for name in dirs + files:
            if is_reparse_point(os.path.join(root, name)):
                raise SystemExit("stage %s contains a link (%s): QA stages are plain files" % (directory, name))
        for name in files:
            full = os.path.join(root, name)
            info = os.stat(full)
            table[os.path.relpath(full, directory)] = (info.st_size, info.st_mtime_ns)
    return table


def file_sha256(path):
    digest = hashlib.sha256()
    with open(path, "rb") as handle:
        for block in iter(lambda: handle.read(1 << 20), b""):
            digest.update(block)
    return digest.hexdigest().upper()


def publish_current(real_name):
    """Mirror stage `real_name` into .qa/stage/current so every run launches peers from one exe path (QA-0153).

    current is a real directory holding copies, not a junction: Windows Firewall matches the image path, which for a
    link is the target, and the QA scripts refuse reparse points. The real stage stays untouched for provenance.
    Returns the paths to record in run.json; a copy happens only when the stage differs from what current holds.
    """
    target = os.path.join(STAGE_ROOT, CURRENT_STAGE)
    launch_exe = os.path.join(target, "fractalengine.exe")
    paths = {"stage": real_name, "stage_dir": os.path.join(STAGE_ROOT, real_name),
             "stage_exe": os.path.join(STAGE_ROOT, real_name, "fractalengine.exe"), "launch_stage": CURRENT_STAGE,
             "launch_dir": target, "launch_exe": launch_exe, "published": {"unchanged": True}}
    if real_name == CURRENT_STAGE:  # --stage current: use the mirror as it is
        if not (os.path.isfile(launch_exe) and os.path.isfile(os.path.join(target, "stage.json"))) or is_reparse_point(target):
            raise SystemExit("%s holds no stage (fractalengine.exe + stage.json) or is a link" % target)
        return paths
    source = paths["stage_dir"]
    if not (os.path.isfile(paths["stage_exe"]) and os.path.isfile(os.path.join(source, "stage.json"))):
        raise SystemExit("stage %s has no fractalengine.exe + stage.json under %s" % (real_name, STAGE_ROOT))
    if is_reparse_point(source) or (os.path.lexists(target) and is_reparse_point(target)):
        raise SystemExit("%s or %s is a link; remove the link (rmdir) so run.py can keep a real copy there" % (source, target))
    wanted = stage_file_table(source)
    present = stage_file_table(target) if os.path.isdir(target) else {}
    changed = sorted((rel for rel, signature in wanted.items() if present.get(rel) != signature), key=lambda rel: (rel == "stage.json", rel))
    stale = sorted(rel for rel in present if rel not in wanted)
    if not changed and not stale:
        return paths
    if "stage.json" not in changed:  # the manifest is dropped first and written last whenever anything is copied
        changed.append("stage.json")
    need = sum(wanted[rel][0] for rel in changed)
    free = shutil.disk_usage(STAGE_ROOT).free
    if free < need + STAGE_COPY_MARGIN_GB * 1024 ** 3:
        raise SystemExit("not enough free disk to mirror stage %s into %s: %.1f GB needed, %.1f GB free" % (
            real_name, target, need / 1024 ** 3, free / 1024 ** 3))
    log("publishing stage %s -> %s (%d changed file(s), %.0f MB, %d stale)" % (real_name, target, len(changed), need / 1024 ** 2, len(stale)))
    started = time.time()
    os.makedirs(target, exist_ok=True)
    manifest = os.path.join(target, "stage.json")
    try:
        if os.path.exists(manifest):
            os.remove(manifest)  # a half-mirrored directory is then no valid stage: the QA scripts refuse it and newest_stage() skips it
        for rel in changed:  # stage.json sorts last
            destination = os.path.join(target, rel)
            os.makedirs(os.path.dirname(destination), exist_ok=True)
            shutil.copy2(os.path.join(source, rel), destination)
        for rel in stale:
            os.remove(os.path.join(target, rel))
    except OSError as error:
        raise SystemExit("could not mirror stage %s into %s (%s). If a peer from an earlier run still runs from "
                         ".qa/stage/current, stop it first (tools/qa/peers.ps1 -Action Stop, or stop.ps1 -Run)." % (real_name, target, error))
    with open(manifest, encoding="utf-8-sig") as handle:
        expected = json.load(handle).get("exe_sha256")
    actual = file_sha256(launch_exe)
    if expected and actual != str(expected).upper():
        raise SystemExit("mirrored fractalengine.exe does not match its stage.json hash (%s != %s)" % (actual, expected))
    paths["published"] = {"files": len(changed), "removed": len(stale), "bytes": need, "seconds": round(time.time() - started, 1)}
    return paths


def firewall_state(program):
    """Read-only Windows Firewall lookup for the exact program path (QA-0153); never creates or edits a rule.

    state: present (an enabled inbound Allow rule applies to an active profile) | blocked (an enabled inbound Block does)
    | absent | unknown (the lookup failed; `error` says why).
    """
    program = os.path.normpath(program)
    script = (
        "$program = %s; try { "
        "$rules = @(Get-NetFirewallApplicationFilter -ErrorAction Stop | Where-Object { $_.Program -ieq $program } | "
        "Get-NetFirewallRule -ErrorAction Stop | ForEach-Object { [pscustomobject]@{ name = $_.DisplayName; "
        "enabled = [string]$_.Enabled; direction = [string]$_.Direction; action = [string]$_.Action; profile = [string]$_.Profile } }); "
        "$active = @(Get-NetConnectionProfile -ErrorAction SilentlyContinue | ForEach-Object { [string]$_.NetworkCategory }); "
        "[pscustomobject]@{ rules = $rules; active_profiles = $active } | ConvertTo-Json -Compress -Depth 4 "
        "} catch { [pscustomobject]@{ error = $_.Exception.Message } | ConvertTo-Json -Compress }" % ps_quote(program))
    state = {"program": program, "state": "unknown", "rules": [], "active_profiles": [], "error": None,
             "checked_at": time.strftime("%Y-%m-%dT%H:%M:%S%z")}
    try:
        code, out, err = powershell(script, timeout=120)
        payload = json.loads(out.strip().splitlines()[-1]) if out.strip() else {}
    except (OSError, ValueError, subprocess.SubprocessError) as error:
        state["error"] = "%s: %s" % (type(error).__name__, str(error)[:200])
        return state
    if code != 0 or payload.get("error") or "rules" not in payload:
        state["error"] = str(payload.get("error") or err.strip() or "firewall lookup returned nothing")[:300]
        return state
    rules = payload["rules"] or []
    state["rules"] = [rules] if isinstance(rules, dict) else list(rules)
    active = payload.get("active_profiles") or []
    state["active_profiles"] = [active] if isinstance(active, str) else list(active)
    live = [rule for rule in state["rules"] if str(rule.get("enabled")).lower() == "true"
            and str(rule.get("direction")).lower() == "inbound" and _profile_applies(rule.get("profile"), state["active_profiles"])]
    if any(str(rule.get("action")).lower() == "block" for rule in live):
        state["state"] = "blocked"
    elif any(str(rule.get("action")).lower() == "allow" for rule in live):
        state["state"] = "present"
    else:
        state["state"] = "absent"
    return state


def _profile_applies(profile, active_profiles):
    names = {part.strip().lower() for part in str(profile).split(",") if part.strip()}
    if "any" in names or "all" in names:
        return True
    active = {str(item).lower().replace("domainauthenticated", "domain") for item in active_profiles}
    return not active or bool(names & active)


class MemoryStatus(ctypes.Structure):
    _fields_ = [("dwLength", wintypes.DWORD), ("dwMemoryLoad", wintypes.DWORD), ("ullTotalPhys", ctypes.c_ulonglong),
                ("ullAvailPhys", ctypes.c_ulonglong), ("ullTotalPageFile", ctypes.c_ulonglong),
                ("ullAvailPageFile", ctypes.c_ulonglong), ("ullTotalVirtual", ctypes.c_ulonglong),
                ("ullAvailVirtual", ctypes.c_ulonglong), ("ullAvailExtendedVirtual", ctypes.c_ulonglong)]


def free_ram_gb():
    status = MemoryStatus()
    status.dwLength = ctypes.sizeof(MemoryStatus)
    ctypes.windll.kernel32.GlobalMemoryStatusEx(ctypes.byref(status))
    return status.ullAvailPhys / 1024 ** 3


def resource_gate(requested, reactive):
    cores = os.cpu_count() or 4
    cpu_cap = 8 if reactive else max(2, math.floor((cores - 4) / 2.5))
    ram_cap = math.floor(free_ram_gb() / RAM_PER_PEER_GB)
    cap = min(cpu_cap, ram_cap)
    note = ("resource gate: %d logical cores -> CPU cap %s, %.1f GB free -> RAM cap %d%s"
            % (cores, "off" if reactive else cpu_cap, free_ram_gb(), ram_cap,
               "" if reactive else " (--no-reactive: pre-QA-0119 CPU cap)"))
    return cap, note


def choose_peers(scenario, args, cap):
    declared = [p["name"] for p in scenario["peers"]]
    if args.peers:
        names = [n.strip() for n in args.peers.split(",") if n.strip()]
        unknown = [n for n in names if n not in declared]
        if unknown or not names or names[0] != "owner":
            raise SystemExit("--peers must start with owner and name scenario peers: %s" % declared)
        if len(names) > cap:
            raise SystemExit("--peers asks for %d peers but the resource gate allows %d" % (len(names), cap))
    else:
        names = declared[: min(cap, args.count or len(declared))]
    if len(names) < 2:
        raise SystemExit("resource gate leaves fewer than 2 peers (cap %d)" % cap)
    for peer in scenario["peers"]:
        if peer["name"] in names:
            implied = "owner" if peer["name"] == "owner" else ("editor" if peer["name"].startswith("editor") else "viewer")
            if implied != peer["role"]:
                raise SystemExit("peer %s: peers.ps1 derives role %s from the name, scenario says %s"
                                 % (peer["name"], implied, peer["role"]))
    return names


# -- peers lifecycle -------------------------------------------------------------------------------
class Host:
    """Owns the started peers: exact-identity affinity, tiling, restart and stop."""

    def __init__(self, args, names, fixture, stage, allow_stale):
        self.args = args
        self.names = names
        self.fixture = fixture
        self.stage = stage
        self.allow_stale = allow_stale
        self.owner_dir = None
        self.record = None
        self.sampler = None
        self.started_at = None

    def peers_cmd(self, action, extra=""):
        return "& %s -Action %s %s" % (ps_quote(os.path.join(QA, "peers.ps1")), action, extra)

    def start(self):
        self.started_at = time.time()
        extra = "-Count %d -Names %s -Fixture %s -BasePort %d -Stage %s%s" % (
            len(self.names), ",".join(self.names), self.fixture, self.args.base_port, ps_quote(self.stage),
            " -AllowStaleStage" if self.allow_stale else "")
        log("peers.ps1 Start %s" % extra)
        code, out, err = powershell(self.peers_cmd("Start", extra), timeout=1200)
        record = self._read_record()
        if code == 0 and record is None:
            # Start completed and wrote the pointer, but not the record expected: its peers are
            # still this run's to stop, by the exact identities that record holds.
            self.owner_dir = self._pointer_dir()
        if code != 0 or record is None:
            raise RuntimeError("peers.ps1 Start failed (%d): %s" % (code, (out + err)[-1500:]))
        self.record = record
        return record

    def _read_record(self):
        """Adopt only the peers.json this Start wrote: same member names, written after Start began."""
        try:
            with open(PEERS_POINTER, encoding="utf-8-sig") as handle:
                owner_dir = handle.read().strip()
            path = os.path.join(owner_dir, "peers.json")
            fresh = os.path.getmtime(path) >= self.started_at - 2
            with open(path, encoding="utf-8-sig") as handle:
                record = json.load(handle)
        except (OSError, ValueError):
            return None
        if [m.get("name") for m in record.get("members", [])] != self.names or not fresh:
            return None
        self.owner_dir = owner_dir
        return record

    @staticmethod
    def _pointer_dir():
        try:
            with open(PEERS_POINTER, encoding="utf-8-sig") as handle:
                return handle.read().strip() or None
        except OSError:
            return None

    def attach(self):
        with open(PEERS_POINTER, encoding="utf-8-sig") as handle:
            self.owner_dir = handle.read().strip()
        with open(os.path.join(self.owner_dir, "peers.json"), encoding="utf-8-sig") as handle:
            self.record = json.load(handle)
        return self.record

    def reload(self):
        with open(os.path.join(self.owner_dir, "peers.json"), encoding="utf-8-sig") as handle:
            self.record = json.load(handle)
        return self.record

    def member(self, name):
        return next(m for m in self.record["members"] if m["name"] == name)

    def pids(self):
        return {m["name"]: int(m["process_identity"]["pid"]) for m in self.record["members"]}

    def pin(self, names=None):
        """Disjoint affinity + AboveNormal by exact recorded identity (pid + creation ticks + exe)."""
        if self.args.no_affinity:
            return
        lines = [". %s" % ps_quote(os.path.join(QA, "_lib.ps1"))]
        for index, member in enumerate(self.record["members"]):
            if names and member["name"] not in names:
                continue
            identity = json.dumps(member["process_identity"]).replace("'", "''")
            mask = AFFINITY_MASKS[index % len(AFFINITY_MASKS)]
            lines.append("$p = Get-QaOwnedProcess -Identity ('%s' | ConvertFrom-Json); if ($p) { "
                         "$p.ProcessorAffinity = [IntPtr]%d; $p.PriorityClass = 'AboveNormal'; "
                         "'pinned %s pid ' + $p.Id + ' mask 0x%X'; $p.Dispose() } else { 'missing %s' }"
                         % (identity, mask, member["name"], mask, member["name"]))
        code, out, err = powershell("; ".join(lines), timeout=120)
        log("affinity: " + " | ".join(line.strip() for line in out.splitlines() if line.strip()) + (err.strip()[:200] if code else ""))

    def tile(self):
        if self.args.no_tile:
            return
        try:
            tile_windows(self.record["members"])
        except OSError as error:
            log("tiling skipped: %s" % error)

    def join(self, log_dir=None):
        """Run the GUI invite flow; each attempt's exit, stdout and stderr stay in <log_dir>/join-attempt-N.log.

        Only a helper that died before doing GUI work (crash status) is retried: peers_join.py is not idempotent.
        """
        verse_name = VERSE_NAMES[self.fixture]
        for attempt in range(1, JOIN_ATTEMPTS + 1):
            log("joining peers to %s through the GUI invite flow (attempt %d/%d)" % (verse_name, attempt, JOIN_ATTEMPTS))
            done = subprocess.run([sys.executable, os.path.join(QA, "peers_join.py"), "--verse-name", verse_name],
                                  capture_output=True, text=True, encoding="utf-8", errors="replace", timeout=900,
                                  stdin=subprocess.DEVNULL, cwd=REPO_ROOT, env=dict(os.environ, MSYS_NO_PATHCONV="1"))
            log("join: exit %s %s" % (exit_label(done.returncode), done.stdout.strip()[-600:]))
            if log_dir:
                with open(os.path.join(log_dir, "join-attempt-%d.log" % attempt), "w", encoding="utf-8") as handle:
                    handle.write("exit %s\n--- stdout\n%s\n--- stderr\n%s\n" % (exit_label(done.returncode), done.stdout, done.stderr))
            if done.returncode == 0:
                return
            if attempt == JOIN_ATTEMPTS or not died_at_spawn(done.returncode):
                raise RuntimeError("peers_join.py failed (exit %s, attempt %d/%d): %s" % (
                    exit_label(done.returncode), attempt, JOIN_ATTEMPTS, (done.stdout + done.stderr)[-1500:]))
            log("join: the helper died before any GUI work; retrying once in %.0f s" % JOIN_RETRY_PAUSE_S)
            time.sleep(JOIN_RETRY_PAUSE_S)

    def restart(self, name, no_bootstrap):
        extra = "-Peer %s -OwnerRun %s%s" % (name, ps_quote(self.owner_dir), " -NoBootstrap" if no_bootstrap else "")
        code, out, err = powershell(self.peers_cmd("Restart", extra), timeout=900)
        if code != 0:
            raise RuntimeError("peers.ps1 Restart %s failed: %s" % (name, (out + err)[-1000:]))
        self.reload()
        self.pin([name])
        if self.sampler:
            self.sampler.write_pids(self.pids())
        self.tile()
        return {"peer": name, "pid": self.member(name)["process_identity"]["pid"], "no_bootstrap": no_bootstrap}

    def stop_peer(self, name):
        """Stop one member by its exact recorded identity (stop.ps1 -Run), leaving its root for Restart."""
        member = self.member(name)
        code, out, err = powershell("& %s -Run %s" % (ps_quote(os.path.join(QA, "stop.ps1")), ps_quote(member["run_dir"])),
                                    timeout=300)
        if code != 0:
            raise RuntimeError("stop.ps1 %s failed: %s" % (name, (out + err)[-800:]))
        if self.sampler:  # a stopped peer's PID may be reused: never charge another process's CPU to it
            self.sampler.write_pids({peer: pid for peer, pid in self.pids().items() if peer != name})
        return {"peer": name, "stopped_pid": member["process_identity"]["pid"]}

    def stop(self):
        if not self.owner_dir:
            return
        code, out, err = powershell(self.peers_cmd("Stop", "-OwnerRun %s" % ps_quote(self.owner_dir)), timeout=600)
        log("peers.ps1 Stop: exit %d %s" % (code, (out + err).strip()[-400:]))
        if code != 0:
            raise RuntimeError("peers.ps1 Stop reported incomplete cleanup")


# -- window tiling (OS-level SetWindowPos on the exact recorded process) ---------------------------
def tile_windows(members):
    user32, kernel32 = ctypes.windll.user32, ctypes.windll.kernel32
    user32.SetProcessDPIAware()
    work = wintypes.RECT()
    user32.SystemParametersInfoW(0x0030, 0, ctypes.byref(work), 0)  # SPI_GETWORKAREA
    width, height = work.right - work.left, work.bottom - work.top
    count = len(members)
    cols = 1 if count == 1 else 2
    rows = math.ceil(count / cols)
    cell_w, cell_h = width // cols, height // rows
    for index, member in enumerate(members):
        identity = member["process_identity"]
        if not _identity_matches(kernel32, int(identity["pid"]), int(identity["start_ticks"])):
            log("tiling: %s identity changed, skipped" % member["name"])
            continue
        hwnd = _main_window(user32, int(identity["pid"]))
        if not hwnd:
            log("tiling: no window for %s" % member["name"])
            continue
        user32.ShowWindow(hwnd, 9)  # SW_RESTORE
        x = work.left + (index % cols) * cell_w
        y = work.top + (index // cols) * cell_h
        user32.SetWindowPos(hwnd, 0, x, y, cell_w, cell_h, 0x0004 | 0x0010)  # NOZORDER | NOACTIVATE
    log("tiled %d windows in a %dx%d grid of %dx%d" % (count, cols, rows, cell_w, cell_h))


def _identity_matches(kernel32, pid, start_ticks):
    handle = kernel32.OpenProcess(0x1000, False, pid)  # PROCESS_QUERY_LIMITED_INFORMATION
    if not handle:
        return False
    try:
        created, exited, kernel, user = (wintypes.FILETIME() for _ in range(4))
        if not kernel32.GetProcessTimes(handle, ctypes.byref(created), ctypes.byref(exited), ctypes.byref(kernel), ctypes.byref(user)):
            return False
        filetime = (created.dwHighDateTime << 32) | created.dwLowDateTime
        return filetime + 504911232000000000 == start_ticks  # FILETIME epoch 1601 -> .NET ticks epoch 0001
    finally:
        kernel32.CloseHandle(handle)


def _main_window(user32, pid):
    found = []
    callback_type = ctypes.WINFUNCTYPE(wintypes.BOOL, wintypes.HWND, wintypes.LPARAM)

    def callback(hwnd, _param):
        owner = wintypes.DWORD()
        user32.GetWindowThreadProcessId(hwnd, ctypes.byref(owner))
        if owner.value == pid and user32.IsWindowVisible(hwnd) and user32.GetWindowTextLengthW(hwnd) > 0:
            found.append(hwnd)
            return False
        return True
    user32.EnumWindows(callback_type(callback), 0)
    return found[0] if found else None


# -- namespaces ----------------------------------------------------------------------------------------
def load_assets():
    assets = {}
    for kind, path in ASSET_MANIFESTS.items():
        try:
            with open(path, encoding="utf-8") as handle:
                manifest = json.load(handle)
        except (OSError, ValueError):
            continue
        assets[kind] = {entry["key"]: entry for entry in manifest.get("assets", [])}
    return assets


def namespaces(host, run_dir):
    owner = host.member("owner")
    with open(os.path.join(owner["run_dir"], "current.json"), encoding="utf-8-sig") as handle:
        fixture = json.load(handle).get("fixture_detail") or {}
    peers = {m["name"]: {"did": m.get("public_did"), "role": m.get("role"), "run_dir": m["run_dir"],
                         "api_base": m.get("api_base")} for m in host.record["members"]}
    return {"fixture": fixture, "peers": peers, "assets": load_assets(), "run": {"dir": run_dir}}


# -- main ---------------------------------------------------------------------------------------------
def run(args):
    scenario = scenario_mod.load(args.scenario)
    steps = select_steps(scenario["steps"], args.steps)
    cap, gate_note = resource_gate(len(scenario["peers"]), args.reactive)
    names = choose_peers(scenario, args, cap)
    log(gate_note + " -> peers " + ",".join(names))
    stage, allow_stale = (None, False) if args.attach else ensure_stage(args)
    launch = publish_current(stage) if stage else None  # peers launch from .qa/stage/current; stage stays the provenance
    firewall = None
    if launch:
        firewall = firewall_state(launch["launch_exe"])
        log(analyze.firewall_line(firewall))
    stamp = time.strftime("%Y%m%d-%H%M%S")
    run_dir = os.path.join(RUNS_ROOT, "%s-%s" % (scenario["name"], stamp))
    os.makedirs(os.path.join(run_dir, "shots"), exist_ok=True)
    host = Host(args, names, scenario["fixture"], launch["launch_stage"] if launch else None, allow_stale)
    clock = Clock()
    recorder = Recorder(run_dir)
    meta = {"schema": "fe-qa-sim-run-v1", "scenario_name": scenario["name"], "scenario_path": scenario["_path"],
            "seed": scenario["seed"], "steps": [str(s["id"]) for s in steps], "stage": stage,
            "stage_matches_source": not allow_stale, "gate": gate_note,
            **({k: launch[k] for k in ("stage_dir", "stage_exe", "launch_stage", "launch_dir", "launch_exe", "published")} if launch else {}),
            "firewall": firewall,
            "cpu_note": None if args.reactive else "--no-reactive (pre-QA-0119 CPU cap): latencies are upper bounds under host contention; read them with the CPU table.",
            "clock": {"perf0": clock.perf0, "wall0": clock.wall0}, "argv": sys.argv[1:]}
    prober = sampler = None
    started = False
    failure = None
    try:
        if args.attach:
            host.attach()
            attached_exe = host.record["members"][0]["process_identity"].get("exe_path")
            meta["launch_exe"] = attached_exe
            if attached_exe:
                meta["firewall"] = firewall_state(attached_exe)
                log(analyze.firewall_line(meta["firewall"]))
        else:
            host.start()
            started = True
            host.pin()
            host.tile()
            if not args.skip_join:
                host.join(run_dir)
        present = [m["name"] for m in host.record["members"]]
        meta["peers"] = [{"name": m["name"], "role": m.get("role"), "run_dir": m["run_dir"], "api_base": m.get("api_base"),
                          "pid": m["process_identity"]["pid"], "exe": m["process_identity"].get("exe_path")} for m in host.record["members"]]
        wrong = [p["name"] for p in meta["peers"] if launch and p["exe"] and os.path.normcase(os.path.normpath(p["exe"])) != os.path.normcase(os.path.normpath(launch["launch_exe"]))]
        if wrong:  # the firewall rule matches the image path: a peer running from elsewhere is not covered by it
            log("WARNING: %s did not launch from %s; the firewall state above does not cover them" % (", ".join(wrong), launch["launch_exe"]))
            meta["launch_exe_mismatch"] = wrong
        meta["owner_run"] = host.owner_dir
        cadence = Cadence({**scenario_mod.DEFAULT_CADENCE, **scenario.get("probe", {})})
        meta["cadence"] = cadence.as_dict()
        with open(os.path.join(run_dir, "run.json"), "w", encoding="utf-8") as handle:
            json.dump(meta, handle, indent=1)
        ctx = scenario_mod.Context(namespaces(host, run_dir))
        verse = ctx.lookup("fixture.verse_id")
        clients = {m["name"]: PeerClient(m, verse) for m in host.record["members"] if m["name"] in present}
        store = LiveStore()
        sampler = HostSampler(run_dir, cadence.get("host_ms") or 2000, args.max_uptime_s + 120)
        host.sampler = sampler
        sampler.start(host.pids())
        prober = Prober(scenario, clients, ctx, clock, recorder, store, cadence, steps)
        prober.start()
        driver = Driver(scenario, clients, ctx, clock, recorder, store, cadence, run_dir, host=host, steps=steps, log=log)
        watchdog = threading.Timer(args.max_uptime_s, lambda: (log("max uptime reached: aborting the driver"), driver.abort.set()))
        watchdog.daemon = True
        watchdog.start()
        try:
            driver.run()
        finally:
            watchdog.cancel()
    except BaseException as error:  # noqa: BLE001 - always fall through to exact-identity cleanup
        failure = error
        log("run failed: %s: %s" % (type(error).__name__, error))
    finally:
        # Each teardown step is isolated: a raising prober/sampler/recorder (disk full, a hung sampler,
        # Ctrl+C mid-join) must never skip the exact-identity peer stop below.
        for label, teardown in (("prober", prober and prober.stop), ("sampler", sampler and sampler.stop),
                                ("recorder", recorder.close)):
            if not teardown:
                continue
            try:
                teardown()
            except BaseException as error:  # noqa: BLE001
                log("cleanup error (%s): %s: %s" % (label, type(error).__name__, error))
                failure = failure or error
        if started or (host.owner_dir and not args.attach):
            try:
                host.stop()
            except Exception as error:  # noqa: BLE001
                log("cleanup error: %s" % error)
                failure = failure or error
    # Record the outcome so the analyzer reports an aborted run as a harness anomaly, never as a clean 0.
    meta["failure"] = "%s: %s" % (type(failure).__name__, failure) if failure else None
    with open(os.path.join(run_dir, "run.json"), "w", encoding="utf-8") as handle:
        json.dump(meta, handle, indent=1)
    summary = analyze.analyze(run_dir, args.divergence_s)
    print(json.dumps({"report": summary["report"], "anomalies": summary["anomalies"],
                      "by_severity": summary["by_severity"], "met": summary["met"],
                      "expectations": summary["expectations"], "failure": str(failure) if failure else None}, default=str))
    return 1 if failure else 0


def build_parser():
    parser = argparse.ArgumentParser(description="FractalEngine scripted P2P simulator (tools/qa/AGENTS.md §sim).")
    parser.add_argument("scenario", help="scenario name (qa/sim/scenarios or tools/qa/sim/scenarios) or path, "
                                         "or a subcommand: validate|schema|analyze|assets|fetch-khronos")
    parser.add_argument("--peers", help="explicit peer subset, e.g. owner,editor (must fit the resource gate)")
    parser.add_argument("--count", type=int, help="first N scenario peers")
    parser.add_argument("--steps", help="step ids or ranges, e.g. 0-3 or 1,5,13a")
    parser.add_argument("--stage", help="explicit .qa/stage directory name")
    parser.add_argument("--allow-stale-stage", action="store_true", help="reuse the newest stage even if HEAD moved")
    parser.add_argument("--base-port", type=int, default=8882)
    parser.add_argument("--reactive", action=argparse.BooleanOptionalAction, default=True,
                        help="QA-0119 is verified: no CPU peer cap (default); --no-reactive restores the pre-QA-0119 cap and banner")
    parser.add_argument("--no-tile", action="store_true")
    parser.add_argument("--no-affinity", action="store_true")
    parser.add_argument("--skip-join", action="store_true", help="peers stay unjoined (harness smoke only)")
    parser.add_argument("--attach", action="store_true", help="drive the peers in .qa/peers-current; never starts or stops them")
    parser.add_argument("--max-uptime-s", type=float, default=1800.0, help="driver abort deadline after peers start")
    parser.add_argument("--divergence-s", type=float, default=analyze.DIVERGENCE_S)
    return parser


def main(argv=None):
    argv = list(sys.argv[1:] if argv is None else argv)
    if argv and argv[0] in ("validate", "schema"):
        return scenario_mod.main(argv)
    if argv and argv[0] == "analyze":
        return analyze.main(argv[1:])
    if argv and argv[0] == "assets":
        import assets
        return assets.main(argv[1:])
    if argv and argv[0] == "fetch-khronos":
        import fetch_khronos
        return fetch_khronos.main(argv[1:])
    return run(build_parser().parse_args(argv))


if __name__ == "__main__":
    sys.exit(main())
