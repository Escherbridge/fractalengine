"""Shared paths, goal bank, peers record and journal readers for the agent swarm; see tools/qa/swarm/AGENTS.md."""
import json
import os

HERE = os.path.dirname(os.path.abspath(__file__))
REPO_ROOT = os.path.abspath(os.path.join(HERE, "..", "..", ".."))
RUN_ROOT = os.path.join(REPO_ROOT, ".qa", "swarm")
PEERS_POINTER = os.path.join(REPO_ROOT, ".qa", "peers-current")
SIM_DIR = os.path.join(REPO_ROOT, "tools", "qa", "sim")
GRADUATED_DIR = os.path.join(SIM_DIR, "scenarios", "swarm-graduated")
CATALOG_PATH = os.path.join(REPO_ROOT, "tools", "fe-cli", "catalog", "mcp-tools.json")
GOALS_PATH = os.path.join(HERE, "goals.json")

ROLE_RANK = {"viewer": 0, "editor": 1, "manager": 2, "owner": 3}
KNOWN_NOUNS = {"instance", "use", "ctx", "verse", "fractal", "petal", "node", "help", "note", "asset", "terrain", "group",
               "invite", "join", "role", "net", "identity", "sync", "query", "replay", "x"}


def read_json(path, default=None):
    """JSON file with an optional BOM (PowerShell writes one); `default` when missing."""
    if not os.path.isfile(path):
        return default
    with open(path, encoding="utf-8-sig") as handle:
        return json.load(handle)


def load_goals(path=GOALS_PATH):
    """The goal bank document, validated; raises ValueError listing every problem."""
    document = read_json(path)
    errors = validate_goals(document)
    if errors:
        raise ValueError("goal bank %s is invalid:\n  " % path + "\n  ".join(errors))
    return document


def validate_goals(document):
    """Structural check of the goal bank; returns error strings."""
    errors = []
    if not isinstance(document, dict) or document.get("schema") != "fe-qa-swarm-goals-v1":
        return ["schema must be fe-qa-swarm-goals-v1"]
    oracles = set(document.get("oracle_names", []))
    seen = set()
    for goal in document.get("goals", []):
        where = "goal %s" % goal.get("id")
        for key in ("id", "story", "role", "preconditions", "allow", "max_commands", "oracles", "success", "area"):
            if key not in goal:
                errors.append("%s: missing %r" % (where, key))
        if goal.get("id") in seen:
            errors.append("%s: duplicate id" % where)
        seen.add(goal.get("id"))
        if not set(goal.get("role", [])) <= set(ROLE_RANK):
            errors.append("%s: unknown role" % where)
        if not set(goal.get("allow", [])) <= KNOWN_NOUNS:
            errors.append("%s: unknown verb family %s" % (where, sorted(set(goal.get("allow", [])) - KNOWN_NOUNS)))
        if not set(goal.get("oracles", [])) <= oracles:
            errors.append("%s: unknown oracle" % where)
        if not isinstance(goal.get("max_commands"), int) or goal["max_commands"] < 1:
            errors.append("%s: max_commands must be a positive integer" % where)
        for predicate in goal.get("success", []):
            if "verb" not in predicate or "min" not in predicate:
                errors.append("%s: success predicate needs verb and min" % where)
    return errors


def read_peers(peers_run=None):
    """Members (name, role, api_base) of the peers record; never reads a token file."""
    if peers_run is None:
        if not os.path.isfile(PEERS_POINTER):
            raise FileNotFoundError("no .qa/peers-current pointer: run tools/qa/peers.ps1 -Action Start first")
        with open(PEERS_POINTER, encoding="utf-8-sig") as handle:
            peers_run = handle.read().strip()
    record = read_json(os.path.join(peers_run, "peers.json"))
    if not record or record.get("schema") != "fe-qa-peers-v1":
        raise ValueError("%s holds no fe-qa-peers-v1 record" % peers_run)
    members = [{"name": m["name"], "role": m.get("role", "viewer"), "api_base": m.get("api_base")} for m in record["members"]]
    return {"run": peers_run, "members": members}


def read_journal(path):
    """Journal lines as dicts in file order; a torn line becomes {"_unparsed": n}."""
    lines = []
    if not os.path.isfile(path):
        return lines
    with open(path, encoding="utf-8") as handle:
        for number, text in enumerate(handle, 1):
            text = text.strip()
            if not text:
                continue
            try:
                lines.append(json.loads(text))
            except ValueError:
                lines.append({"_unparsed": number})
    return lines


def verb_of(line):
    """`noun verb` of a journal line: resolved.verb, else the leading non-flag argv words."""
    resolved = line.get("resolved") or {}
    if resolved.get("verb"):
        return resolved["verb"]
    words = []
    for word in line.get("argv") or []:
        if word.startswith("-"):
            break
        words.append(word)
    return " ".join(words[:2])


def noun_of(line):
    return (verb_of(line) or "").split(" ")[0]


def is_dry_run(line):
    return "--dry-run" in (line.get("argv") or [])


def role_of(line):
    return str((line.get("instance") or {}).get("role") or "").lower() or None


def catalog_min_roles():
    """Tool name -> minimum role rank from the CLI's embedded MCP snapshot (read-only)."""
    rows = read_json(CATALOG_PATH, [])
    return {row["name"]: ROLE_RANK.get(str(row.get("min_role", "")).lower()) for row in rows if row.get("name")}


def agent_dirs(run_dir):
    """Agent id -> directory, from the run manifest order."""
    manifest = read_json(os.path.join(run_dir, "run.json"), {})
    return {a["agent"]: os.path.join(run_dir, "agents", a["agent"]) for a in manifest.get("agents", [])}
