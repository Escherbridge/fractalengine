#!/usr/bin/env python3
"""Built-in scripted driver: a seeded random walker over `fe help --json`, no LLM; see tools/qa/swarm/AGENTS.md §drivers."""
import argparse
import json
import os
import random
import subprocess
import sys
import zlib

WORDS = ["north hall", "library", "annex", "lab", "depot", "garden", "tower", "workshop", "gate", "atrium"]
SKIPPED_NOUNS = {"replay", "note", "help", "instance", "ctx", "use", "join", "asset", "query"}
UNFILLABLE = {"to_instance", "recipient_did", "invite", "file", "terrain", "config", "peer_did"}
LEVELS = ("verse", "fractal", "petal")
LIST_BY_LEVEL = {"verse": ["verse", "list"], "fractal": ["fractal", "list"], "petal": ["petal", "list"]}
KIND_KEYS = {"verses": "verse", "fractals": "fractal", "petals": "petal", "nodes": "node", "groups": "group"}


class Pools:
    """Ids and names the walker has seen in replies, by kind, in first-seen order."""

    def __init__(self):
        self.ids = {kind: [] for kind in ("verse", "fractal", "petal", "node", "group")}

    def add(self, kind, value):
        if kind in self.ids and isinstance(value, str) and value and value not in self.ids[kind]:
            self.ids[kind].append(value)

    def harvest(self, value, kind_hint=None):
        """Collect every `id` found under a plural key (nodes, petals...) or at the top of a create reply."""
        if isinstance(value, dict):
            if kind_hint and "id" in value:
                self.add(kind_hint, value["id"])
            for key, item in value.items():
                self.harvest(item, KIND_KEYS.get(key, kind_hint if key == "data" else None))
        elif isinstance(value, list):
            for item in value:
                self.harvest(item, kind_hint)


def build_words(command, rng, pools, have_context):
    """Argument words for one catalog command, or None when a required value cannot be invented."""
    path, positional, flags, optional = list(command["path"]), {}, [], []
    for arg in command["args"]:
        name, kind = arg["name"], arg["type"]
        if arg.get("from_context") and have_context.get(arg["from_context"]):
            continue
        if name in UNFILLABLE or name.endswith("_did"):
            if arg["required"]:
                return None
            continue
        value = None
        if arg.get("enum"):
            value = rng.choice(arg["enum"])
        elif kind == "json":
            if arg["required"]:
                return None
            continue
        elif name.endswith("_id") or name in KIND_KEYS.values():
            pool = pools.ids.get(name[:-3] if name.endswith("_id") else name) or []
            if not pool:
                if arg["required"]:
                    return None
                continue
            value = rng.choice(pool)
        elif kind == "integer":
            value = str(rng.randint(1, 5))
        elif kind == "number":
            value = str(round(rng.uniform(-20.0, 20.0), 1))
        elif kind == "boolean":
            value = rng.random() < 0.3
        else:
            value = rng.choice(WORDS) if name == "name" else "walker-" + str(rng.randint(1, 99))
        if arg["required"] or arg.get("positional") is not None:
            if arg.get("positional") is not None:
                positional[arg["positional"]] = str(value)
            else:
                flags += _flag(arg, value)
        else:
            optional.append(_flag(arg, value))
    if not flags and not positional and optional:
        flags += optional[rng.randrange(len(optional))]
    else:
        flags += [word for group in optional if rng.random() < 0.4 for word in group]
    return path + [positional[index] for index in sorted(positional)] + flags


def _flag(arg, value):
    if arg["type"] == "boolean":
        return ["--" + arg["flag"]] if value else []
    return ["--" + arg["flag"], str(value)]


def choose_command(commands, rng, allow):
    """Weighted pick: reads half the time, writes half the time, among allowed non-local verbs."""
    usable = sorted((c for c in commands if c["path"][0] in allow and c["path"][0] not in SKIPPED_NOUNS and c["via"] != "local"),
                    key=lambda c: c["path"])
    if not usable:
        return None
    writes = [c for c in usable if c["supports"]["wait"]]
    reads = [c for c in usable if not c["supports"]["wait"]]
    pick = writes if (writes and (not reads or rng.random() < 0.5)) else reads
    return rng.choice(pick)


STATE = {"issued": 0, "exhausted": False}  # every invocation counts, helpers included; exit 11 ends the walk


def run_fe(fe, words, env, timeout_s=90):
    """One `fe` invocation with --json; returns (exit code, parsed envelope or None)."""
    STATE["issued"] += 1
    done = subprocess.run([fe, *words, "--json"], env=env, capture_output=True, text=True, timeout=timeout_s)
    envelope = None
    for text in reversed(done.stdout.strip().splitlines()):
        try:
            envelope = json.loads(text)
            break
        except ValueError:
            continue
    STATE["exhausted"] = STATE["exhausted"] or done.returncode == 11
    return done.returncode, envelope


def ensure_context(fe, env, rng, pools, have_context, needed):
    """Select verse -> fractal -> petal down to `needed` with `fe use`, listing first when no id is known; False when impossible."""
    for level in LEVELS[:LEVELS.index(needed) + 1]:
        if have_context.get(level):
            continue
        if not pools.ids[level]:
            code, envelope = run_fe(fe, LIST_BY_LEVEL[level], env)
            if envelope:
                pools.harvest(envelope.get("data"), level)
        if not pools.ids[level]:
            return False
        code, envelope = run_fe(fe, ["use", level, rng.choice(pools.ids[level])], env)
        if code != 0:
            return False
        have_context[level] = True
    return True


def walk(fe, env, seed, budget, allow):
    """The walk itself; returns the number of commands issued."""
    rng, pools, have_context = random.Random(seed), Pools(), {}
    STATE.update(issued=0, exhausted=False)
    code, help_doc = run_fe(fe, ["help"], env)
    document = (help_doc or {}).get("data") if help_doc and "data" in help_doc else help_doc
    commands = (document or {}).get("commands", [])
    while commands and STATE["issued"] < budget - 1 and not STATE["exhausted"]:
        if "x" in allow and rng.random() < 0.15:  # extension commands are dynamic: the walker only lists them
            run_fe(fe, ["x", "--list"], env)
            continue
        command = choose_command(commands, rng, allow)
        if command is None:
            break
        levels = [arg["from_context"] for arg in command["args"] if arg.get("from_context") in LEVELS]
        if levels and not ensure_context(fe, env, rng, pools, have_context, max(levels, key=LEVELS.index)):
            continue
        words = build_words(command, rng, pools, have_context)
        if words is None:
            STATE["issued"] += 1  # an unfillable pick still spends a turn, so the loop always ends
            continue
        code, envelope = run_fe(fe, words, env)
        if envelope and code == 0:
            pools.harvest(envelope.get("data"), command["path"][0] if command["path"][1:] == ["create"] else None)
    if not STATE["exhausted"]:
        run_fe(fe, ["note", "walker finished after %d commands (seed %s)" % (STATE["issued"], seed)], env)
    return STATE["issued"]


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--agent-dir", help="the agent directory (informational; configuration comes from the environment)")
    parser.add_argument("brief", nargs="?", help="brief path (unused by the walker)")
    parser.parse_args(argv)
    env = dict(os.environ)
    seed = zlib.crc32((env.get("SWARM_SEED", "1") + ":" + env.get("SWARM_AGENT", "agent")).encode())
    budget = int(env.get("FE_BUDGET_MAX_COMMANDS", "20"))
    allow = {noun for noun in env.get("FE_CLI_ALLOW", "").split(",") if noun}
    issued = walk(env.get("SWARM_FE", "fe"), env, seed, budget, allow)
    print("walker issued %d commands" % issued)
    return 0


if __name__ == "__main__":
    sys.exit(main())
