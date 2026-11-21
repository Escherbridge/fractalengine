"""Friction report and goal progress over one agent's journal; see tools/qa/swarm/AGENTS.md §friction."""
import collections

import swarm_lib

RETRY_WINDOW = 5  # a success this many commands after a failure of the same verb counts as a retry
HELP_REPEAT = 3  # the same help path this often is a repeated lookup
USAGE_RATE = 0.2  # share of exit-6 outcomes that marks a confusing surface


def goal_progress(lines, success):
    """Each success predicate with its observed count; `exit` omitted means any exit."""
    rows = []
    for predicate in success:
        verb = predicate["verb"]
        count = sum(1 for line in lines
                    if not swarm_lib.is_dry_run(line) and not line.get("help")
                    and (swarm_lib.verb_of(line) == verb or swarm_lib.verb_of(line).startswith(verb + " ") or swarm_lib.noun_of(line) == verb)
                    and ("exit" not in predicate or line.get("exit") == predicate["exit"]))
        rows.append({**predicate, "count": count, "met": count >= predicate["min"]})
    return rows


def report(agent, lines, goal):
    """Friction items for one agent: retries, help lookups, usage errors, abandonment, notes."""
    parsed = [line for line in lines if "_unparsed" not in line]
    items = []
    for index, line in enumerate(parsed):
        if line.get("exit") in (0, 11, None) or line.get("help") or line.get("note"):
            continue
        verb = swarm_lib.verb_of(line)
        for later in parsed[index + 1:index + 1 + RETRY_WINDOW]:
            if swarm_lib.verb_of(later) == verb and later.get("exit") == 0 and later.get("argv") != line.get("argv"):
                items.append({"kind": "retry-with-different-arguments", "agent": agent, "seq": line.get("seq"),
                              "detail": "`%s` failed (exit %s %s) then succeeded at seq %s with other arguments" % (verb, line.get("exit"), line.get("code"), later.get("seq"))})
                break
    help_paths = collections.Counter(tuple(word for word in (line.get("argv") or []) if not word.startswith("-")) for line in parsed if line.get("help"))
    for path, count in help_paths.items():
        if count >= HELP_REPEAT:
            items.append({"kind": "repeated-help-lookup", "agent": agent, "seq": None, "detail": "`fe %s` looked up %d times" % (" ".join(path), count)})
    usage = sum(1 for line in parsed if line.get("exit") == 6)
    if parsed and usage / len(parsed) >= USAGE_RATE and usage >= 3:
        items.append({"kind": "high-usage-error-rate", "agent": agent, "seq": None, "detail": "%d of %d commands exited 6" % (usage, len(parsed))})
    progress = goal_progress(parsed, goal["success"]) if goal else []
    unmet = [row for row in progress if not row["met"]]
    if unmet:
        spent = sum(1 for line in parsed if line.get("exit") != 11)
        exhausted = any(line.get("exit") == 11 for line in parsed) or (goal and spent >= goal["max_commands"])
        items.append({"kind": "abandoned-goal" if not exhausted else "dead-end-budget-spent", "agent": agent, "seq": None,
                      "detail": "unmet: " + ", ".join("%s x%d (saw %d)" % (row["verb"], row["min"], row["count"]) for row in unmet)})
    for line in parsed:
        text = line.get("note")
        if text:
            items.append({"kind": "agent-note", "agent": agent, "seq": line.get("seq"), "detail": text if isinstance(text, str) else str(text)})
    return {"agent": agent, "items": items, "progress": progress}
