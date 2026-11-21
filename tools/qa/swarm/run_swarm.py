#!/usr/bin/env python3
"""Prepare, drive and judge a swarm of `fe`-only agents against the local peers; see tools/qa/swarm/AGENTS.md."""
import argparse
import datetime
import json
import os
import random
import shlex
import shutil
import subprocess
import sys
import threading
import time

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)
import friction  # noqa: E402
import oracles  # noqa: E402
import swarm_lib  # noqa: E402

PASS_THROUGH_ENV = ("PATH", "SYSTEMROOT", "SystemRoot", "TEMP", "TMP", "HOME", "USERPROFILE", "LOCALAPPDATA", "APPDATA", "PYTHONIOENCODING", "LANG")
WALKER_COMMAND = [sys.executable, os.path.join(HERE, "walker.py"), "--agent-dir", "{agent_dir}"]

BRIEF = """# Agent brief: {agent}

## Goal
{story}

You act as the **{role}** of this verse on the local instance named `{instance}`.

## Rules
- Your only tool is the `fe` command line. Run `fe help` for the commands and `fe help --json` for the machine catalog; `fe <noun> <verb> --help` explains one command.
- Use only verbs from this list: {allow}. Anything else is refused by the tool.
- Do not read, list, open or copy any file other than through `fe`. Token files and anything under `.qa` are off limits, and you never need them.
- Never put a token, an invite or any other secret in a command line; if a command needs an invite, pass it as `--invite -` on standard input.
- You have at most **{budget} commands**. When `fe` answers exit 11 your budget is spent: stop.
- Stop as soon as the goal is reached, or when you are stuck. Say which in a final `fe note "..."`.
- Whenever something is confusing, missing or surprising, record it with `fe note "<what you expected, what happened>"`. Notes are how the team learns where the tool hurts, so use them freely.
- A refusal is an answer, not a failure: read its message, and do not retry the same refused command.

## Context
Preconditions: {preconditions}
Goal id: `{goal_id}`. Your commands are recorded in a journal for later replay.
"""


def now_utc():
    return datetime.datetime.now(datetime.timezone.utc)


def find_fe(explicit=None):
    """The `fe` binary: --fe, FE_BIN, a built target/{release,debug}/fe, else `fe` on PATH."""
    if explicit:
        return explicit
    if os.environ.get("FE_BIN"):
        return os.environ["FE_BIN"]
    suffix = ".exe" if os.name == "nt" else ""
    for profile in ("release", "debug"):
        candidate = os.path.join(swarm_lib.REPO_ROOT, "target", profile, "fe" + suffix)
        if os.path.isfile(candidate):
            return candidate
    return shutil.which("fe") or "fe"


def choose_assignments(goals, members, count, seed, only_goals=None, allow_unjoined=False):
    """Seeded, deterministic goal-to-peer assignment: one agent per peer (first `count` members), distinct goals while they last."""
    rng = random.Random(seed)
    pool = [g for g in goals if (not only_goals or g["id"] in only_goals) and (allow_unjoined or not g.get("needs_unjoined_peer"))]
    chosen, used = [], set()
    for member in members[:count]:
        fitting = [g for g in pool if member["role"] in g["role"]]
        fresh = [g for g in fitting if g["id"] not in used] or fitting
        if not fresh:
            raise ValueError("no goal fits role %s (goals: %s)" % (member["role"], ", ".join(g["id"] for g in pool) or "none"))
        goal = rng.choice(sorted(fresh, key=lambda g: g["id"]))
        used.add(goal["id"])
        chosen.append((member, goal))
    return chosen


def prepare(args):
    """Create the run directory: manifest, per-agent brief, workdir, context and journal paths. Starts nothing."""
    bank = swarm_lib.load_goals()
    peers = swarm_lib.read_peers(args.peers_run)
    if args.agents > len(peers["members"]):
        raise ValueError("%d agents requested but only %d peers are recorded" % (args.agents, len(peers["members"])))
    only = set(args.goal.split(",")) if args.goal else None
    assignments = choose_assignments(bank["goals"], peers["members"], args.agents, args.seed, only, args.allow_unjoined)
    run_id = args.run_id or "swarm-%s-s%d" % (now_utc().strftime("%Y%m%d-%H%M%S"), args.seed)
    run_dir = os.path.join(swarm_lib.RUN_ROOT, run_id)
    os.makedirs(run_dir)
    fe_bin = find_fe(args.fe)
    agents = []
    for member, goal in assignments:
        agent_dir = os.path.join(run_dir, "agents", member["name"])
        workdir = os.path.join(agent_dir, "work")
        os.makedirs(workdir)
        allow = list(dict.fromkeys(bank["base_allow"] + goal["allow"]))
        budget = int(args.budget_cap) if args.budget_cap and args.budget_cap < goal["max_commands"] else goal["max_commands"]
        brief_path = os.path.join(agent_dir, "brief.md")
        with open(brief_path, "w", encoding="utf-8") as handle:
            handle.write(BRIEF.format(agent=member["name"], story=goal["story"], role=member["role"], instance=member["name"],
                                      allow=", ".join("`%s`" % noun for noun in allow), budget=budget, goal_id=goal["id"],
                                      preconditions="; ".join(goal["preconditions"])))
        env = {"FE_INSTANCE": member["name"], "FE_AGENT_ID": member["name"], "FE_RUN_ID": run_id, "FE_CWD": swarm_lib.REPO_ROOT,
               "FE_CONTEXT": os.path.join(agent_dir, "ctx.json"), "FE_JOURNAL": os.path.join(agent_dir, "journal.jsonl"),
               "FE_STATE_DIR": os.path.join(agent_dir, "state"), "FE_BUDGET_MAX_COMMANDS": str(budget),
               "FE_CLI_ALLOW": ",".join(allow), "FE_WAIT_DEFAULT": args.wait_default}
        agents.append({"agent": member["name"], "instance": member["name"], "role": member["role"], "goal": goal["id"],
                       "max_commands": budget, "brief": brief_path, "workdir": workdir, "env": env,
                       "journal": env["FE_JOURNAL"], "oracles": goal["oracles"]})
    manifest = {"schema": "fe-qa-swarm-run-v1", "run_id": run_id, "seed": args.seed, "created": now_utc().isoformat(),
                "fe_bin": fe_bin, "peers_run": peers["run"], "peers": peers["members"], "agents": agents}
    with open(os.path.join(run_dir, "run.json"), "w", encoding="utf-8") as handle:
        json.dump(manifest, handle, indent=1)
    return run_dir, manifest


def agent_environment(manifest, agent, extra_names):
    """Minimal child environment: PATH-like basics, the agent's FE_* variables, and named pass-throughs, nothing else."""
    env = {name: os.environ[name] for name in PASS_THROUGH_ENV + tuple(extra_names) if name in os.environ}
    env.update(agent["env"])
    env.update({"SWARM_BRIEF": agent["brief"], "SWARM_AGENT": agent["agent"], "SWARM_FE": manifest["fe_bin"],
                "SWARM_SEED": str(manifest["seed"]), "SWARM_AGENT_DIR": os.path.dirname(agent["brief"])})
    return env


def driver_argv(template, manifest, agent):
    """Expand {brief} {agent} {agent_dir} {fe} {seed} in a driver command; the brief path is appended when no placeholder names it."""
    values = {"brief": agent["brief"], "agent": agent["agent"], "agent_dir": os.path.dirname(agent["brief"]),
              "fe": manifest["fe_bin"], "seed": str(manifest["seed"])}
    words = [word.format(**values) for word in template]
    return words if "{brief}" in " ".join(template) else words + [agent["brief"]]


def kill_tree(process):
    """Stop exactly the process this runner started and its children."""
    if process.poll() is not None:
        return
    if os.name == "nt":
        subprocess.run(["taskkill", "/PID", str(process.pid), "/T", "/F"], capture_output=True)
    else:
        process.kill()
    process.wait(timeout=15)


def run_driver(manifest, agent, template, timeout_s, extra_env, outcomes):
    """One agent's driver process: cwd is its empty workdir, output to driver.log, killed at the wall-clock deadline."""
    argv = driver_argv(template, manifest, agent)
    log_path = os.path.join(os.path.dirname(agent["brief"]), "driver.log")
    started = time.time()
    process = None
    try:
        with open(log_path, "w", encoding="utf-8") as log:
            process = subprocess.Popen(argv, cwd=agent["workdir"], env=agent_environment(manifest, agent, extra_env),
                                       stdin=subprocess.DEVNULL, stdout=log, stderr=subprocess.STDOUT)
            try:
                code = process.wait(timeout=timeout_s)
                outcomes[agent["agent"]] = {"exit": code, "seconds": round(time.time() - started, 1), "timed_out": False}
            except subprocess.TimeoutExpired:
                outcomes[agent["agent"]] = {"exit": None, "seconds": round(time.time() - started, 1), "timed_out": True}
    except OSError as error:
        outcomes[agent["agent"]] = {"exit": None, "seconds": 0, "timed_out": False, "error": str(error)}
    finally:
        if process is not None:
            kill_tree(process)


def drive(manifest, template, timeout_s, concurrency, extra_env):
    """Run every agent's driver, at most `concurrency` at once; always reaps what it started."""
    outcomes, gate, threads = {}, threading.BoundedSemaphore(concurrency), []

    def worker(agent):
        with gate:
            run_driver(manifest, agent, template, timeout_s, extra_env, outcomes)

    for agent in manifest["agents"]:
        thread = threading.Thread(target=worker, args=(agent,), daemon=True)
        threads.append(thread)
        thread.start()
    for thread in threads:
        thread.join()
    return outcomes


def evaluate(run_dir, final_fingerprint=False):
    """Run every oracle and the friction report over the run's journals; returns the report dict."""
    manifest = swarm_lib.read_json(os.path.join(run_dir, "run.json"))
    goals = {g["id"]: g for g in swarm_lib.load_goals()["goals"]}
    min_roles = swarm_lib.catalog_min_roles()
    findings, frictions, summary = [], [], []
    for agent in manifest["agents"]:
        lines = swarm_lib.read_journal(agent["journal"])
        goal, names = goals[agent["goal"]], set(agent["oracles"])
        if not lines:
            findings.append(oracles.finding("budget", "warn", agent["agent"], None, "empty journal: the driver ran no `fe` command"))
        if "no_secrets" in names:
            findings += oracles.no_secrets(agent["agent"], lines)
        if "no_server_faults" in names:
            findings += oracles.server_faults(agent["agent"], lines)
        if "budget" in names:
            findings += oracles.budget(agent["agent"], lines, agent["max_commands"])
        if "role_refusals" in names:
            findings += oracles.role_refusals(agent["agent"], lines, min_roles)
        friction_report = friction.report(agent["agent"], lines, goal)
        frictions.append(friction_report)
        summary.append({"agent": agent["agent"], "goal": agent["goal"], "commands": len(lines),
                        "progress": friction_report["progress"], "met": all(row["met"] for row in friction_report["progress"])})
    if any("convergence" in a["oracles"] for a in manifest["agents"]):
        findings += oracles.convergence(run_dir, {"members": manifest["peers"]}, final_fingerprint)
    report = {"run_id": manifest["run_id"], "seed": manifest["seed"], "summary": summary, "findings": findings,
              "friction": frictions, "failed": sorted({f["oracle"] for f in findings if f["severity"] == "fail"})}
    with open(os.path.join(run_dir, "report.json"), "w", encoding="utf-8") as handle:
        json.dump(report, handle, indent=1)
    with open(os.path.join(run_dir, "report.md"), "w", encoding="utf-8") as handle:
        handle.write(render_markdown(report))
    return report


def render_markdown(report):
    """Human report: per-agent goal progress, oracle findings, friction items."""
    out = ["# Swarm run %s (seed %s)" % (report["run_id"], report["seed"]), "", "## Goals"]
    for row in report["summary"]:
        out.append("- `%s` on %s: %s, %d commands" % (row["goal"], row["agent"], "goal met" if row["met"] else "goal NOT met", row["commands"]))
    out += ["", "## Oracle findings", ""]
    ordered = sorted(report["findings"], key=lambda f: ("fail", "warn", "info").index(f["severity"]))
    out += ["- **%s** `%s` %s%s: %s" % (f["severity"], f["oracle"], f["agent"] or "run", "" if f["seq"] is None else " #%s" % f["seq"], f["detail"]) for f in ordered] or ["- none"]
    out += ["", "## Friction", ""]
    items = [item for entry in report["friction"] for item in entry["items"]]
    out += ["- `%s` %s%s: %s" % (i["kind"], i["agent"], "" if i["seq"] is None else " #%s" % i["seq"], i["detail"]) for i in items] or ["- none"]
    return "\n".join(out) + "\n"


def print_summary(run_dir, report):
    print("run dir: %s" % run_dir)
    for row in report["summary"]:
        print("  %-8s %-28s %3d commands  goal %s" % (row["agent"], row["goal"], row["commands"], "met" if row["met"] else "NOT met"))
    counts = {s: sum(1 for f in report["findings"] if f["severity"] == s) for s in ("fail", "warn", "info")}
    friction_items = sum(len(entry["items"]) for entry in report["friction"])
    print("findings: %d fail, %d warn, %d info; friction items: %d" % (counts["fail"], counts["warn"], counts["info"], friction_items))
    print("report: %s" % os.path.join(run_dir, "report.md"))


def add_prepare_arguments(parser):
    parser.add_argument("--agents", type=int, default=3, help="number of agents, one per peer in peers.json order (3-5)")
    parser.add_argument("--seed", type=int, default=1, help="seeds goal assignment and the walker driver")
    parser.add_argument("--goal", help="comma list of goal ids to draw from (default: whole bank)")
    parser.add_argument("--peers-run", help="owner run dir holding peers.json (default: .qa/peers-current)")
    parser.add_argument("--fe", help="path to the fe binary (default: FE_BIN, target/{release,debug}/fe, PATH)")
    parser.add_argument("--run-id", help="override the generated run id")
    parser.add_argument("--budget-cap", type=int, help="lower every agent's command budget to at most this")
    parser.add_argument("--wait-default", default="peers", help="FE_WAIT_DEFAULT for the agents (default peers)")
    parser.add_argument("--allow-unjoined", action="store_true", help="also draw goals that need a peer that has not joined the verse")


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    sub = parser.add_subparsers(dest="command", required=True)
    prep = sub.add_parser("prepare", help="write briefs and environments only; start nothing")
    add_prepare_arguments(prep)
    run = sub.add_parser("run", help="prepare, drive every agent, then judge the journals")
    add_prepare_arguments(run)
    run.add_argument("--driver", default="walker", help="'walker' or a command template; {brief} {agent} {agent_dir} {fe} {seed} are substituted")
    run.add_argument("--agent-timeout-s", type=float, default=900.0, help="wall-clock kill per agent")
    run.add_argument("--concurrency", type=int, default=0, help="agents running at once (default: all)")
    run.add_argument("--pass-env", default="", help="comma list of extra environment variable NAMES forwarded to the drivers (e.g. an LLM key)")
    run.add_argument("--final-fingerprint", action="store_true", help="run the sim's fingerprint against the live peers afterwards")
    judge = sub.add_parser("oracles", help="re-run the oracles over an existing run dir")
    judge.add_argument("run_dir")
    judge.add_argument("--final-fingerprint", action="store_true")
    args = parser.parse_args(argv)
    if args.command == "oracles":
        report = evaluate(os.path.abspath(args.run_dir), args.final_fingerprint)
        print_summary(args.run_dir, report)
        return 1 if report["failed"] else 0
    run_dir, manifest = prepare(args)
    print("prepared %s (%d agents)" % (run_dir, len(manifest["agents"])))
    if args.command == "prepare":
        return 0
    template = WALKER_COMMAND if args.driver == "walker" else shlex.split(args.driver, posix=(os.name != "nt"))
    extra = [name for name in args.pass_env.split(",") if name]
    outcomes = drive(manifest, template, args.agent_timeout_s, args.concurrency or len(manifest["agents"]), extra)
    with open(os.path.join(run_dir, "drivers.json"), "w", encoding="utf-8") as handle:
        json.dump(outcomes, handle, indent=1)
    report = evaluate(run_dir, args.final_fingerprint)
    print_summary(run_dir, report)
    return 1 if report["failed"] else 0


if __name__ == "__main__":
    sys.exit(main())
