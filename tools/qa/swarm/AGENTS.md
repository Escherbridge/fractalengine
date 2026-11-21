# tools/qa/swarm

A small swarm of agents gets ONLY the `fe` CLI (`tools/fe-cli/AGENTS.md`), each pursues a user-style goal against the
LOCAL peers, and the recorded sessions become regression material. Plan: `.omc/plans/fe-cli-20261120.md` phase 3.
Python stdlib only, like the rest of `tools/qa` (the sim's analysis additionally needs pandas, as it always did).

**Fit with the 2026-11-07 decision** (`tools/qa/AGENTS.md` §sim: scripts execute, agents design/explore/triage): agents
here never form the regression suite. They explore; a journal graduates into a scripted `tools/qa/sim` scenario only
after a no-LLM replay on fresh peers reproduces it. A swarm run is a finding generator; the sim scenario is the test.

## Files

| File | Owns |
|---|---|
| `goals.json` | The goal bank (`fe-qa-swarm-goals-v1`): id, area, story, roles, preconditions, `allow` verb families, `max_commands`, oracles, journal `success` predicates |
| `run_swarm.py` | `prepare` (briefs + env, starts nothing), `run` (prepare, drive, judge), `oracles <run-dir>` (re-judge) |
| `walker.py` | Built-in scripted driver: seeded random walker over `fe help --json` |
| `oracles.py` | Journal oracles + the opt-in convergence oracle |
| `friction.py` | Friction report and goal progress |
| `graduate.py` | Replay on fresh peers, `--emit-scenario`, `scenario.py validate` |
| `swarm_lib.py` | Paths, goal-bank validation, peers record and journal readers |
| `selftest.py` | Pure-logic checks (no instance, no `fe`): `python tools/qa/swarm/selftest.py` |

## Usage

```powershell
# 1. peers (as for the sim): owner/editor/viewer joined to the fixture verse
tools\qa\peers.ps1 -Action Start -Count 3 ; python tools/qa/peers_join.py
# 2. prepare only: read the briefs, start nothing
python tools/qa/swarm/run_swarm.py prepare --agents 3 --seed 7
# 3. run with the scripted walker (no LLM), then judge
python tools/qa/swarm/run_swarm.py run --agents 3 --seed 7 --budget-cap 40 --final-fingerprint
# 4. stop the peers, start FRESH ones, then graduate the clean journals
python tools/qa/swarm/graduate.py .qa/swarm/<run-id> --fresh
```

A run lives in `.qa/swarm/<run-id>/` (`.qa/` is gitignored): `run.json` (manifest, no secrets), `agents/<name>/`
(`brief.md`, `journal.jsonl`, `ctx.json`, `driver.log`, empty `work/`, `state/`), `drivers.json`, `report.json|md`,
`graduation/`. `--agents` takes the first N members of `peers.json`; agent id = peer name = `FE_INSTANCE`, which keeps
`fe replay --map agent=instance` an identity. `--goal a,b` restricts the draw; the draw is seeded (`random.Random(seed)`),
one goal per agent whose role fits, distinct goals while they last. `needs_unjoined_peer` goals are skipped unless
`--allow-unjoined`. The `fe` binary is `--fe`, else `FE_BIN`, else `target/{release,debug}/fe`, else PATH. Wall clock is
enforced by the runner (`--agent-timeout-s`, process tree killed by exact PID in `finally`), command budget by the CLI.

## Per-agent environment (§env)

`FE_INSTANCE`, `FE_CONTEXT`, `FE_JOURNAL`, `FE_AGENT_ID`, `FE_RUN_ID`, `FE_BUDGET_MAX_COMMANDS`, `FE_CLI_ALLOW` (goal
families + `help use ctx instance note`), `FE_WAIT_DEFAULT=peers`, `FE_STATE_DIR` (per-agent `fe x` cache), `FE_CWD` =
repo root (so `.qa/peers-current` discovery works while the process cwd is the agent's empty `work/`). The child
environment is built from scratch: PATH-like basics, the FE_* above, `SWARM_*`, and only the names given to
`--pass-env`. The runner never opens a token file and prints nothing from one.

## Drivers (§drivers)

A driver is a command, run once per agent, cwd = the agent's empty `work/`, stdin closed, output to `driver.log`.
`--driver "<template>"` is split like a shell line and `{brief} {agent} {agent_dir} {fe} {seed}` are substituted (the
brief path is appended when no template word names `{brief}`; literal braces in a template are not supported). The
driver reads the brief, runs `fe` commands (each is journaled by the CLI itself, so the runner needs no
transcript), and exits. Variables it also gets: `SWARM_BRIEF SWARM_AGENT SWARM_AGENT_DIR SWARM_FE SWARM_SEED`.

**Plugging in an LLM session.** The harness embeds no vendor call. Write a small wrapper that starts a headless agent
session with the brief as the prompt and ONLY a shell tool restricted to the `fe` binary (no file tools, no network
tools), a turn cap and a spend cap, and pass it as `--driver`. For Claude Code that wrapper is
`claude -p "$(cat {brief})" --allowedTools "Bash(fe:*)" --max-turns 40` run from `work/` (forward the API key name with
`--pass-env`); any other agent runtime works the same way. The restriction lives in the wrapper's tool allowlist, the
budget in `FE_BUDGET_MAX_COMMANDS`, the verb allowlist in `FE_CLI_ALLOW`; none of the three trusts the model.
Friction reporting is `fe note`, which the journal keeps.

**Walker (`--driver walker`, default).** `fe help --json` once, then until the budget: pick reads or writes 50/50 among
the goal's allowed catalog verbs (`SKIPPED_NOUNS` excluded), make sure `fe use verse|fractal|petal` provides any context
id the verb needs (listing first to harvest ids), invent arguments by type (words, small numbers, enum choice, ids from
what earlier replies showed), run it, harvest ids from the reply, and end with `fe note`. Verbs whose arguments cannot be
invented (invite recipient, JSON bodies, files, DIDs) are skipped; `fe x` is only listed. Seed =
`crc32("<run seed>:<agent>")`, so a walk repeats exactly when the replies repeat. It exists so the harness, the oracles and
graduation are exercisable with no LLM; it is a smoke driver, not a user model.

## §oracles

No LLM. Per agent, the oracles named by its goal run over `journal.jsonl`:

| Oracle | Rule |
|---|---|
| `role_refusals` | For a line whose tool's `min_role` (CLI catalog snapshot `tools/fe-cli/catalog/mcp-tools.json`) is above the line's recorded role: exit 0 is **fail** (S1 accepted-but-expected-refusal); exit 4 passes; 5 is info (uniform not-found); anything else fails; a refused line carrying `created_ids` or a settled `--wait` fails (no state change). A refusal (4) at a sufficient role is a warn. State-level "no change" is the convergence oracle's job: a refused write that leaked would diverge or show in the fingerprint |
| `no_server_faults` | Any exit 10 (5xx), 8 (unknown outcome) or 1 (CLI internal) fails |
| `no_secrets` | Any string under a token/invite/ticket/secret/credential/capability/password/bearer key that is not `<redacted ...>`, any `Bearer ...`, any 48+ char opaque run (digest keys exempt). Findings give path and length, never the text. Loaded token values are deliberately not compared: that would mean reading token files |
| `budget` | Journaled commands (exit 11 lines excluded) never exceed the goal budget; `seq` unique; torn lines warn |
| `convergence` | **Reuse, not reimplementation**: with `--final-fingerprint`, writes a one-step scenario `swarm-final-fingerprint` (`host fingerprint`, label `final`) and runs `tools/qa/sim/run.py <it> --attach`, then reads the sim run dir with `analyze.Analysis(...).fingerprint_diff()`. Divergent keys fail. Without the flag it is an info line, so an offline re-judge never touches instances |
| `friction` | Not a verdict: see below |

Goal success is journal-only: each `success` predicate `{verb, exit?, min}` counts non-dry-run, non-help lines.

## §friction

Per agent: `retry-with-different-arguments` (same verb failed, then succeeded within 5 commands with other argv),
`repeated-help-lookup` (same help path 3+ times), `high-usage-error-rate` (exit 6 on 20%+ and 3+), `abandoned-goal` /
`dead-end-budget-spent` (a success predicate unmet, budget left / spent), and every `fe note` text. These are UX
findings for the CLI and the product, never test failures.

## §graduation

`graduate.py <run-dir> --fresh` takes each agent journal that has no oracle failure, no redacted argv (replay cannot
rebind a capability) and at least one succeeding real command, and:

1. `fe replay <journal> --map agent=instance` against the CURRENT peers; exit 0 required. `--fresh` is an explicit
   acknowledgement that `.qa/peers-current` is a fresh peer set, and the script refuses when it is the run's own peers
   (replaying onto the state the journal already changed proves nothing).
2. `fe replay <journal> --emit-scenario <out> --scenario-name swarm-<goal>-<agent>-s<seed>` into
   `tools/qa/sim/scenarios/swarm-graduated/` (tracked; review the diff before committing).
3. `python tools/qa/sim/scenario.py validate <out>`; an invalid file is deleted and the row says why.

`--skip-replay` emits unconfirmed scenarios under `<run>/graduation/unconfirmed/` only. Existing names need `--force`.
Run emitted scenarios with `run.py` by path (`scenario.py` only auto-lists the flat `scenarios/` directory).

## Known limits and CLI gaps (§gaps)

- The sim fingerprint covers the fixture verse only (`$fixture.verse_id`), so verses an agent creates are outside the
  convergence oracle; goals therefore work inside the fixture verse.
- Replay on fresh peers: fixture verse/petal ids differ per launch and `fe replay` re-binds only ids created inside the
  journal (`--bind old=new` is manual), so journals that touch fixture-created nodes diverge. Wanted: replay re-binding
  fixture ids by name (verse/fractal/petal names) or a `--bind-fixture` mode.
- `fe replay --emit-scenario` does not run anything, so the determinism check and the emit are two invocations.
- `invite-and-join` needs an unjoined peer and the MCP `join_verse` path, unverified on a fresh peer (plan risk 6).
- Extension stories depend on an exposed command; consent stays a GUI click made by the sim, never an agent.
- The walker cannot drive `invite create`, `join`, `asset upload`, `terrain set` or `fe x` calls (no way to invent the
  arguments from the catalog alone); LLM drivers cover them.
- A journal line holds `unix_ms`, not the plan's `mono_ms`; per-verb convergence budgets from `wait.ms` are not judged yet.

## §selftest

`python tools/qa/swarm/selftest.py` checks the goal bank, seeded assignment, brief and env construction, every oracle on a
hand-written journal, the friction rules, walker argument building, that the fingerprint scenario is valid sim DSL, and
graduation eligibility. It starts nothing. It is a script check, not a cargo test, so no `conductor/test-batches.md`
batch exists for it; run it by name.
