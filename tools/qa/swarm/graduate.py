#!/usr/bin/env python3
"""Graduate a swarm run's journals into sim scenarios: replay on fresh peers, emit, validate; see tools/qa/swarm/AGENTS.md §graduation."""
import argparse
import json
import os
import re
import subprocess
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)
import run_swarm  # noqa: E402
import swarm_lib  # noqa: E402

REPLAY_TIMEOUT_S = 900
RUN_ONLY_ENV = ("FE_BUDGET_MAX_COMMANDS", "FE_CLI_ALLOW", "FE_AGENT_ID")


def slug(text):
    return re.sub(r"[^a-z0-9-]+", "-", text.lower()).strip("-")


def ineligible_reason(agent, lines, report):
    """Why a journal cannot graduate, or None."""
    if not lines:
        return "empty journal"
    failed = [f for f in report["findings"] if f["agent"] == agent["agent"] and f["severity"] == "fail"]
    if failed:
        return "oracle failure: %s" % ", ".join(sorted({f["oracle"] for f in failed}))
    if any("<redacted" in " ".join(str(word) for word in line.get("argv") or []) for line in lines):
        return "argv carries a redacted capability (replay cannot rebind it)"
    if not any(line.get("exit") == 0 and not swarm_lib.is_dry_run(line) and swarm_lib.noun_of(line) not in ("help", "note", "use", "ctx") for line in lines):
        return "no state-changing or reading command succeeded"
    return None


def replay_environment(manifest, agent, scratch):
    """Child env for `fe replay`: the agent's identity, no run-only limits, and replay's own journal and context."""
    env = {name: os.environ[name] for name in run_swarm.PASS_THROUGH_ENV if name in os.environ}
    env.update({k: v for k, v in agent["env"].items() if k not in RUN_ONLY_ENV})
    env.update({"FE_JOURNAL": os.path.join(scratch, "replay-%s.jsonl" % agent["agent"]), "FE_CONTEXT": os.path.join(scratch, "ctx-%s.json" % agent["agent"]),
                "FE_STATE_DIR": os.path.join(scratch, "state-%s" % agent["agent"]), "FE_AGENT_ID": "replay-" + agent["agent"]})
    return env


def run_fe(fe, words, env, cwd):
    done = subprocess.run([fe, *words], env=env, cwd=cwd, capture_output=True, text=True, timeout=REPLAY_TIMEOUT_S)
    return done.returncode, done.stdout.strip().splitlines()[-1] if done.stdout.strip() else ""


def validate_scenario(path):
    done = subprocess.run([sys.executable, os.path.join(swarm_lib.SIM_DIR, "scenario.py"), "validate", path], capture_output=True, text=True)
    return done.returncode == 0, (done.stdout + done.stderr).strip()


def graduate(run_dir, confirm_replay, force):
    """Returns one result row per agent; scenarios of confirmed journals land in tools/qa/sim/scenarios/swarm-graduated/."""
    manifest = swarm_lib.read_json(os.path.join(run_dir, "run.json"))
    report = swarm_lib.read_json(os.path.join(run_dir, "report.json")) or run_swarm.evaluate(run_dir)
    scratch = os.path.join(run_dir, "graduation")
    os.makedirs(scratch, exist_ok=True)
    target_dir = swarm_lib.GRADUATED_DIR if confirm_replay else os.path.join(scratch, "unconfirmed")
    os.makedirs(target_dir, exist_ok=True)
    results = []
    for agent in manifest["agents"]:
        lines = swarm_lib.read_journal(agent["journal"])
        row = {"agent": agent["agent"], "goal": agent["goal"], "graduated": False}
        reason = ineligible_reason(agent, lines, report)
        name = "swarm-%s-%s-s%s" % (slug(agent["goal"]), slug(agent["agent"]), manifest["seed"])
        out_path = os.path.join(target_dir, name + ".json")
        if reason is None and os.path.exists(out_path) and not force:
            reason = "%s exists (use --force)" % os.path.relpath(out_path, swarm_lib.REPO_ROOT)
        env = replay_environment(manifest, agent, scratch)
        if reason is None and confirm_replay:
            code, last = run_fe(manifest["fe_bin"], ["replay", agent["journal"], "--map", "%s=%s" % (agent["agent"], agent["instance"]), "--json"], env, swarm_lib.REPO_ROOT)
            if code != 0:
                reason = "replay on fresh peers exited %d: %s" % (code, last[:200])
        if reason is None:
            code, last = run_fe(manifest["fe_bin"], ["replay", agent["journal"], "--emit-scenario", out_path, "--scenario-name", name, "--json"], env, swarm_lib.REPO_ROOT)
            valid, text = validate_scenario(out_path) if code == 0 and os.path.isfile(out_path) else (False, "emit exited %d: %s" % (code, last[:200]))
            if not valid:
                reason = "scenario invalid: %s" % text[:300]
                if os.path.isfile(out_path):
                    os.remove(out_path)
        row.update({"graduated": reason is None, "reason": reason, "scenario": None if reason else os.path.relpath(out_path, swarm_lib.REPO_ROOT),
                    "confirmed": confirm_replay})
        results.append(row)
    with open(os.path.join(scratch, "graduation.json"), "w", encoding="utf-8") as handle:
        json.dump(results, handle, indent=1)
    return results


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("run_dir", help=".qa/swarm/<run-id>")
    parser.add_argument("--fresh", action="store_true", help="confirm .qa/peers-current holds FRESH peers (not the run's own, whose state the journal already changed)")
    parser.add_argument("--skip-replay", action="store_true", help="emit without the determinism check, into <run>/graduation/unconfirmed only")
    parser.add_argument("--force", action="store_true", help="overwrite an existing scenario of the same name")
    args = parser.parse_args(argv)
    run_dir = os.path.abspath(args.run_dir)
    manifest = swarm_lib.read_json(os.path.join(run_dir, "run.json"))
    if not args.skip_replay:
        if not args.fresh:
            parser.error("replay mutates the peers: start fresh ones and pass --fresh (or --skip-replay for unconfirmed output)")
        current = swarm_lib.read_peers()["run"]
        if os.path.normcase(os.path.abspath(current)) == os.path.normcase(os.path.abspath(manifest["peers_run"])):
            parser.error(".qa/peers-current is the run's own peers; replaying onto them proves nothing. Stop them and start fresh peers")
    results = graduate(run_dir, not args.skip_replay, args.force)
    for row in results:
        print("%-8s %-28s %s" % (row["agent"], row["goal"], row["scenario"] if row["graduated"] else "not graduated: " + row["reason"]))
    return 0


if __name__ == "__main__":
    sys.exit(main())
