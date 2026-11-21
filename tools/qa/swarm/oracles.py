"""Journal oracles for a swarm run (no LLM, no instance access except the opt-in fingerprint); see tools/qa/swarm/AGENTS.md §oracles."""
import json
import os
import re
import subprocess
import sys
import time

import swarm_lib

SECRET_KEY = re.compile(r"token|invite|ticket|secret|credential|capability|password|bearer", re.I)
OPAQUE_RUN = re.compile(r"[A-Za-z0-9_\-+/=]{48,}")
BEARER = re.compile(r"bearer\s+\S{8,}", re.I)
DIGEST_KEYS = {"envelope_digest", "catalog_digest"}
ACCEPTED_REFUSAL_EXITS = (4,)  # role refusal; 5 (uniform not-found) is tolerated as info
SERVER_FAULT_EXITS = {1: "internal", 8: "unknown outcome", 10: "server fault"}


def finding(oracle, severity, agent, seq, detail):
    return {"oracle": oracle, "severity": severity, "agent": agent, "seq": seq, "detail": detail}


def _walk_strings(value, path=""):
    """Yield (json path, last key, string) for every string in a journal line."""
    if isinstance(value, dict):
        for key, item in value.items():
            yield from _walk_strings(item, path + "." + str(key))
    elif isinstance(value, list):
        for index, item in enumerate(value):
            yield from _walk_strings(item, "%s[%d]" % (path, index))
    elif isinstance(value, str):
        yield path, path.rsplit(".", 1)[-1].split("[")[0], value


def no_secrets(agent, lines):
    """No token, invite or other capability text survives in a journal; findings never echo the matched text."""
    found = []
    for line in lines:
        for path, key, text in _walk_strings(line):
            if key in DIGEST_KEYS or text.startswith("<redacted"):
                continue
            if SECRET_KEY.search(key) and text:
                found.append(finding("no_secrets", "fail", agent, line.get("seq"), "unredacted value under %s (%d chars)" % (path, len(text))))
            elif BEARER.search(text) or OPAQUE_RUN.search(text):
                found.append(finding("no_secrets", "fail", agent, line.get("seq"), "opaque or bearer-like text at %s (%d chars)" % (path, len(text))))
    return found


def server_faults(agent, lines):
    """No 5xx-class outcome (exit 10), no unknown outcome (8) and no CLI internal error (1)."""
    return [finding("no_server_faults", "fail", agent, line.get("seq"),
                    "exit %s %s on `%s`" % (line.get("exit"), SERVER_FAULT_EXITS[line["exit"]], swarm_lib.verb_of(line)))
            for line in lines if line.get("exit") in SERVER_FAULT_EXITS]


def budget(agent, lines, max_commands):
    """Counted commands never exceed the budget; seq is unique and the exit-11 line is the only one past it."""
    counted = [line for line in lines if line.get("exit") != 11 and "_unparsed" not in line]
    found = []
    if len(counted) > max_commands:
        found.append(finding("budget", "fail", agent, None, "%d commands journaled, budget %d" % (len(counted), max_commands)))
    seqs = [line.get("seq") for line in lines if "seq" in line]
    if len(seqs) != len(set(seqs)):
        found.append(finding("budget", "fail", agent, None, "duplicate seq values: the counter was bypassed"))
    found.extend(finding("budget", "warn", agent, None, "torn journal line %d" % line["_unparsed"]) for line in lines if "_unparsed" in line)
    return found


def role_refusals(agent, lines, min_roles):
    """A write below the caller's role is refused with exit 4 and changes nothing; an accepted one is S1."""
    found = []
    for line in lines:
        tool = (line.get("resolved") or {}).get("tool")
        role = swarm_lib.role_of(line)
        floor = min_roles.get(tool) if tool else None
        if floor is None or role not in swarm_lib.ROLE_RANK or swarm_lib.is_dry_run(line) or line.get("exit") in (11, 6):
            continue
        code, seq, verb = line.get("exit"), line.get("seq"), swarm_lib.verb_of(line)
        if swarm_lib.ROLE_RANK[role] < floor:
            if code == 0:
                found.append(finding("role_refusals", "fail", agent, seq, "`%s` accepted for role %s (needs rank %d): S1 accepted-but-expected-refusal" % (verb, role, floor)))
            elif code == 5:
                found.append(finding("role_refusals", "info", agent, seq, "`%s` refused with the uniform not-found (5), expected role refusal (4)" % verb))
            elif code not in ACCEPTED_REFUSAL_EXITS:
                found.append(finding("role_refusals", "fail", agent, seq, "`%s` below role floor ended in exit %s, expected 4" % (verb, code)))
            if code != 0 and (line.get("created_ids") or (line.get("wait") or {}).get("settled") is True and (line.get("wait") or {}).get("ms", 0) > 0):
                found.append(finding("role_refusals", "fail", agent, seq, "refused `%s` still reports a created id or a settled write" % verb))
        elif code == 4:
            found.append(finding("role_refusals", "warn", agent, seq, "`%s` refused (4) although role %s meets the tool floor: owner-only scope or stale role" % (verb, role)))
    return found


def convergence(run_dir, peers, execute, timeout_s=300):
    """Peers hold one state: the sim's own `host fingerprint` op (driver.fingerprint), compared by Analysis.fingerprint_diff."""
    if not execute:
        return [finding("convergence", "info", None, None, "skipped: re-run with --final-fingerprint while the peers are up")]
    scenario_path = write_fingerprint_scenario(run_dir, peers)
    run_py = os.path.join(swarm_lib.SIM_DIR, "run.py")
    started = time.time()
    try:
        done = subprocess.run([sys.executable, run_py, scenario_path, "--attach"], cwd=swarm_lib.REPO_ROOT, timeout=timeout_s,
                              capture_output=True, text=True)
    except subprocess.TimeoutExpired:
        return [finding("convergence", "fail", None, None, "sim fingerprint run timed out after %ds" % timeout_s)]
    sim_runs = os.path.join(swarm_lib.REPO_ROOT, "qa", "sim", "runs")
    fresh = sorted(d for d in (os.listdir(sim_runs) if os.path.isdir(sim_runs) else [])
                   if d.startswith("swarm-final-fingerprint-") and os.path.getmtime(os.path.join(sim_runs, d)) >= started - 1)
    if not fresh:
        return [finding("convergence", "fail", None, None, "sim run left no run dir (exit %s)" % done.returncode)]
    sys.path.insert(0, swarm_lib.SIM_DIR)
    import analyze  # noqa: E402 (needs pandas, like the sim itself)
    diff = analyze.Analysis(os.path.join(sim_runs, fresh[-1])).fingerprint_diff().get("final")
    if diff is None:
        return [finding("convergence", "fail", None, None, "no final fingerprint was written (sim exit %s)" % done.returncode)]
    if diff["diverged"]:
        return [finding("convergence", "fail", None, None, "%d keys differ across peers, first: %s" % (len(diff["diverged"]), [d["key"] for d in diff["diverged"][:5]]))]
    return [finding("convergence", "info", None, None, "peers agree: %s" % diff["sha256"])]


def write_fingerprint_scenario(run_dir, peers):
    """A one-step sim scenario whose only action is `host fingerprint` labelled `final`."""
    scenario = {
        "name": "swarm-final-fingerprint", "seed": 1, "fixture": "basic",
        "description": "Swarm convergence oracle: a final cross-peer fingerprint of the shared verse.",
        "peers": [{"name": m["name"], "role": m["role"]} for m in peers["members"]],
        "steps": [{"id": "final", "title": "Final fingerprint", "settle_s": 5,
                   "actions": [{"peer": "owner", "via": "host", "op": "fingerprint", "args": {"label": "final"}}]}],
    }
    path = os.path.join(run_dir, "final-fingerprint.json")
    with open(path, "w", encoding="utf-8") as handle:
        json.dump(scenario, handle, indent=1)
    return path
