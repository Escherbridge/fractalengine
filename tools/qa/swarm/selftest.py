#!/usr/bin/env python3
"""Pure-logic self-test of the swarm harness: no instance, no `fe` binary, no network; see tools/qa/swarm/AGENTS.md §selftest."""
import argparse
import json
import os
import random
import sys
import tempfile
import unittest

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)
import friction  # noqa: E402
import graduate  # noqa: E402
import oracles  # noqa: E402
import run_swarm  # noqa: E402
import swarm_lib  # noqa: E402
import walker  # noqa: E402

sys.path.insert(0, swarm_lib.SIM_DIR)
import scenario as sim_scenario  # noqa: E402


def line(seq, argv, exit_code, role="viewer", tool=None, **extra):
    base = {"v": 1, "agent": "viewer", "seq": seq, "argv": argv, "exit": exit_code, "code": "ok" if exit_code == 0 else "x",
            "resolved": {"verb": " ".join(argv[:2]), "tool": tool}, "instance": {"name": "viewer", "role": role}, "created_ids": []}
    base.update(extra)
    return base


class SwarmLogic(unittest.TestCase):
    def test_goal_bank_is_valid_and_wide(self):
        bank = swarm_lib.load_goals()
        self.assertGreaterEqual(len(bank["goals"]), 12)
        areas = " ".join(goal["area"] for goal in bank["goals"])
        for needed in ("navigation", "node CRUD", "properties", "groups", "roles and refusals", "invites", "terrain", "fe x"):
            self.assertIn(needed, areas)

    def test_assignment_is_seeded(self):
        goals = swarm_lib.load_goals()["goals"]
        members = [{"name": "owner", "role": "owner"}, {"name": "editor", "role": "editor"}, {"name": "viewer", "role": "viewer"}]
        first = [g["id"] for _, g in run_swarm.choose_assignments(goals, members, 3, 7)]
        again = [g["id"] for _, g in run_swarm.choose_assignments(goals, members, 3, 7)]
        self.assertEqual(first, again)
        self.assertEqual(len(set(first)), 3)

    def test_prepare_writes_briefs_without_secrets(self):
        with tempfile.TemporaryDirectory() as scratch:
            peers_dir = os.path.join(scratch, "owner-run")
            os.makedirs(peers_dir)
            members = [{"name": n, "role": r, "api_base": "http://127.0.0.1:%d" % p} for n, r, p in (("owner", "owner", 8882), ("editor", "editor", 8883), ("viewer", "viewer", 8884))]
            with open(os.path.join(peers_dir, "peers.json"), "w", encoding="utf-8") as handle:
                json.dump({"schema": "fe-qa-peers-v1", "members": members}, handle)
            swarm_lib.RUN_ROOT = os.path.join(scratch, "runs")
            run_swarm.swarm_lib.RUN_ROOT = swarm_lib.RUN_ROOT
            args = argparse.Namespace(agents=3, seed=3, goal=None, peers_run=peers_dir, fe="fe", run_id="t", budget_cap=None, wait_default="peers", allow_unjoined=False)
            run_dir, manifest = run_swarm.prepare(args)
            for agent in manifest["agents"]:
                with open(agent["brief"], encoding="utf-8") as handle:
                    brief = handle.read()
                self.assertIn("only tool is the `fe` command line", brief)
                self.assertIn(str(agent["max_commands"]), brief)
                self.assertEqual(agent["env"]["FE_BUDGET_MAX_COMMANDS"], str(agent["max_commands"]))
                self.assertIn("note", agent["env"]["FE_CLI_ALLOW"].split(","))
                self.assertTrue(os.path.isdir(agent["workdir"]) and not os.listdir(agent["workdir"]))
            argv = run_swarm.driver_argv(["python", "d.py", "--brief", "{brief}"], manifest, manifest["agents"][0])
            self.assertEqual(argv[-1], manifest["agents"][0]["brief"])
            self.assertEqual(argv.count(manifest["agents"][0]["brief"]), 1)

    def test_secrets_oracle_flags_without_echoing(self):
        leaked = line(1, ["join", "--invite", "x"], 0, resolved={"verb": "join", "args": {"invite": "docaaa-real-looking-invite"}})
        opaque = line(2, ["node", "get"], 0, data_small={"blob": "A" * 60})
        clean = line(3, ["join"], 0, resolved={"verb": "join", "args": {"invite": "<redacted blake3:0123456789abcdef>"}}, envelope_digest="b" * 64)
        found = oracles.no_secrets("viewer", [leaked, opaque, clean])
        self.assertEqual([f["seq"] for f in found], [1, 2])
        self.assertNotIn("docaaa", json.dumps(found))

    def test_role_refusal_oracle(self):
        floors = {"update_transform": 1, "get_hierarchy": 0}
        accepted = line(1, ["node", "move"], 0, tool="update_transform")
        refused = line(2, ["node", "move"], 4, tool="update_transform")
        uniform = line(3, ["node", "move"], 5, tool="update_transform")
        leaked_state = line(4, ["node", "move"], 4, tool="update_transform", created_ids=["01XYZ"])
        wrong = line(5, ["node", "move"], 10, tool="update_transform")
        read = line(6, ["node", "get"], 0, tool="get_hierarchy")
        found = oracles.role_refusals("viewer", [accepted, refused, uniform, leaked_state, wrong, read], floors)
        by_seq = {}
        for item in found:
            by_seq.setdefault(item["seq"], []).append(item["severity"])
        self.assertEqual(by_seq[1], ["fail"])
        self.assertNotIn(2, by_seq)
        self.assertEqual(by_seq[3], ["info"])
        self.assertEqual(by_seq[4], ["fail"])
        self.assertEqual(by_seq[5], ["fail"])
        self.assertNotIn(6, by_seq)

    def test_faults_and_budget(self):
        lines = [line(i, ["node", "list"], 0) for i in range(1, 4)] + [line(4, ["node", "list"], 10), line(5, ["node", "list"], 8), line(6, ["node", "list"], 11)]
        self.assertEqual(len(oracles.server_faults("viewer", lines)), 2)
        self.assertEqual(oracles.budget("viewer", lines, 5), [])
        self.assertEqual(oracles.budget("viewer", lines, 4)[0]["severity"], "fail")
        self.assertEqual(oracles.budget("viewer", lines + [line(3, ["node", "list"], 0)], 9)[0]["detail"][:9], "duplicate")

    def test_friction_report(self):
        goal = {"success": [{"verb": "node create", "exit": 0, "min": 2}], "max_commands": 30}
        lines = [line(1, ["node", "create", "--name", "a"], 6), line(2, ["node", "create", "a", "--x", "1"], 0),
                 line(3, ["help"], 0, help=True), line(4, ["help"], 0, help=True), line(5, ["help"], 0, help=True),
                 line(6, ["note", "text"], 0, note="the flag name was a guess")]
        result = friction.report("editor", lines, goal)
        kinds = {item["kind"] for item in result["items"]}
        self.assertTrue({"retry-with-different-arguments", "repeated-help-lookup", "abandoned-goal", "agent-note"} <= kinds)
        self.assertEqual(result["progress"][0]["count"], 1)

    def test_walker_builds_valid_words(self):
        command = {"path": ["node", "move"], "via": "mcp", "supports": {"wait": ["local"]}, "args": [
            {"name": "node_id", "flag": "node-id", "type": "string", "required": True, "enum": [], "from_context": None, "positional": None},
            {"name": "x", "flag": "x", "type": "number", "required": False, "enum": [], "from_context": None, "positional": None},
            {"name": "petal_id", "flag": "petal", "type": "string", "required": True, "enum": [], "from_context": "petal", "positional": None}]}
        pools = walker.Pools()
        self.assertIsNone(walker.build_words(command, random.Random(1), pools, {"petal": True}))
        pools.harvest({"nodes": [{"id": "01NODE"}]})
        words = walker.build_words(command, random.Random(1), pools, {"petal": True})
        self.assertEqual(words[:4], ["node", "move", "--node-id", "01NODE"])
        self.assertNotIn("--petal", words)
        again = walker.build_words(command, random.Random(1), pools, {"petal": True})
        self.assertEqual(words, again)

    def test_fingerprint_scenario_is_valid_sim_dsl(self):
        with tempfile.TemporaryDirectory() as scratch:
            path = oracles.write_fingerprint_scenario(scratch, {"members": [{"name": "owner", "role": "owner"}, {"name": "viewer", "role": "viewer"}]})
            with open(path, encoding="utf-8") as handle:
                self.assertEqual(sim_scenario.validate(json.load(handle)), [])

    def test_graduation_refuses_redacted_argv_and_failed_oracles(self):
        agent = {"agent": "viewer"}
        report = {"findings": []}
        ok = [line(1, ["node", "list"], 0)]
        self.assertIsNone(graduate.ineligible_reason(agent, ok, report))
        self.assertIn("redacted", graduate.ineligible_reason(agent, ok + [line(2, ["join", "--invite", "<redacted blake3:00>"], 0)], report))
        failing = {"findings": [{"agent": "viewer", "severity": "fail", "oracle": "no_secrets"}]}
        self.assertIn("no_secrets", graduate.ineligible_reason(agent, ok, failing))


if __name__ == "__main__":
    unittest.main(argv=[sys.argv[0]] + sys.argv[1:])
