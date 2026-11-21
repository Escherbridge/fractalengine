# fe-cli

`fe` (lib `fe_cli`, bin `fe`) is the local operator CLI for a running FractalEngine instance. It is table-driven over
the MCP tool catalog, talks MCP `tools/call` on `POST /mcp` first and REST where MCP has no tool or flattens the
refusal (`fe x` calls, §retry), and is built for
scripts and agents: no prompts, a stable `--json` envelope, stable exit codes, and a JSONL journal per invocation.
Entry point `fe_cli::run(Invocation{argv, env, stdout, stderr}) -> i32`; the environment is injected, never read
globally (the only exception is `std::env::current_dir` when `FE_CWD` is absent). Design and phase plan:
`.omc/plans/fe-cli-20261120.md`.

## Security properties

- **Loopback only.** `instance::assert_loopback` accepts `http://` plus an IP-literal loopback address (127.0.0.0/8 or
  `[::1]`) with no path, userinfo, `%` or whitespace; `localhost` is refused (hosts-file DNS). It runs twice per
  request: on the instance record (`Client::new`) and on the `api_base` re-read from the token file in
  `Client::send`, so a `--instance <tokens.json>` whose file names a remote host never sends. reqwest is built with
  `default-features = false` (no TLS stack: `https` is unreachable by construction), `.no_proxy()` and
  `redirect::Policy::none()`, so a redirect cannot carry the bearer elsewhere. A refusal exits 3 `not-loopback`.
- **Tokens are local files, never output.** The token file (`fe_api::dev_token`, owner-only DACL / 0600, 8 h TTL) is
  re-read on every request; the bearer goes only into the `Authorization` header. Journal argv blanks secret-named
  flag values and `journal::redact` replaces token/invite/secret/ticket/credential fields with
  `<redacted blake3:…>`. Transport errors are rendered `without_url()`.
- **Token file mode (§secrets).** On Unix `instance::check_token_file_private` refuses a token file whose mode grants
  group/other any access (exit 3 `token-file-permissions`), at resolution and on every re-read. On Windows the
  engine-written DACL is trusted; there is no `fe doctor` yet to report it, and no ACL dependency is added.
- **No loaded token leaves as an argument (§secrets).** `secrets::loaded` = every verse token plus the operator token.
  The spine refuses, before anything is sent, printed by `--dry-run` or journaled verbatim, any invocation whose words
  or parsed args contain one, or whose `--args @path` names the instance's token file (canonical compare): exit 6
  `token-in-args`, message never echoes the value. `fe x` re-checks after reading `--args @file`, and `Client::send`
  re-checks every body and path as the backstop. The journal replaces any string carrying a loaded token, and any
  args string over 256 chars, with `<redacted blake3:…>` (structure kept; an `--args-json` argv word is redacted
  as parsed JSON).
- **Ids in routes (§ids).** Every id interpolated into a URL path (petal, verse, node, command) must be
  `[A-Za-z0-9_-]{1,128}`; package ids follow the manifest grammar (fe-sdk `manifest_v2::is_package_id`). Ids never
  name a file: the `fe x` cache file is a digest.
- **Never executes extension code.** No `fe-plugin` and no wasm runtime in `[dependencies]` (dev-dependencies are the
  test engine only). Extension verbs live under `fe x` and cannot shadow core verbs; discovery text is untrusted and
  passes through `render::terminal_safe` (control/ANSI/OSC/bidi stripped, length-bounded) in human mode.
- **Role checks stay on the server.** Every scene verb goes through the role-checked MCP doors; the CLI never
  short-circuits authorization and never splits the uniform not-found (exit 5).

## Module map

| File | Owns |
|---|---|
| `src/lib.rs` | `Invocation`, `run`, the spine: global flags, routing, instance + context resolution, dispatch, envelope, journal |
| `src/catalog.rs` | verb table, `route` (total routing + nearest-command refusals), `GLOBAL_FLAGS`, `flag_specs`, `args_json_only`, `wants_operator_token`, embedded `catalog/mcp-tools.json` |
| `src/commands/*.rs` | core verbs (`use`/`ctx`, `verse`, `fractal`, `petal`, `node`, `instance list`, `note`), arg merge, dry-run plan, `--wait local` / `--wait peers` probes |
| `src/commands/generic.rs` | every `phase == 2` catalog row (one table row = one MCP tool or REST route) plus the per-verb `prepare` hooks |
| `src/commands/replay.rs`, `src/commands/scenario.rs` | `fe replay`: child-invocation replay with id re-binding; journal -> sim scenario |
| `src/schema_flags.rs` | JSON-schema -> flags and parsing, shared by core verbs and `fe x` |
| `src/x.rs` | `fe x` list / help over the discovery route (stale cache keyed by caller), calls over the REST command route |
| `src/transport.rs` | loopback client, token pick, reply classification to exit codes, `retry_allowed` |
| `src/secrets.rs`, `src/ids.rs` | loaded-token argument guard; route-id and package-id grammar |
| `src/instance.rs`, `src/context.rs` | instance discovery + token file; per-agent context file |
| `src/envelope.rs`, `src/journal.rs`, `src/render.rs` | envelope + exit table, journal line, human rendering |

## Routing and global flags

Routing is explicit and total (`catalog::route`): the first word is the noun; `noun verb` wins when that row exists,
else a noun-only row (`ctx`, `help`, `note`) when the noun has one and it takes words or none follow. Anything else
exits 6 `unknown-command` naming the nearest valid commands (edit distance). `CommandRequest` carries `noun` so
dispatch is by noun, and `x` (no table row) is routed by the spine itself. `help`, `note` and `instance list` are
answered without resolving an instance.

One rule for global flags (`--json --dry-run --wait --wait-budget --retry --instance --verse --fractal --petal`):
they may appear before or after the command words. After `fe x` every token belongs to the extension command and is
passed verbatim; only `--json`, `--help`/`-h` and `--list` are read there (the schema-flag generator reserves those
spellings, so no command argument can claim them). Put the other global flags before `x`. A schema property whose
flag would be a global spelling, an unsafe name (`^[a-z][a-z0-9_-]{0,63}$`) or a collision is dropped from flag
generation and listed by help as `args_json_only`; it stays reachable through `--args-json` / `--args @file`.

## Context, required ids and waits

- Context file: `FE_CONTEXT`, else `<state>/ctx-<instance>.json` (`FE_STATE_DIR`, else `%LOCALAPPDATA%` /
  `$XDG_STATE_HOME` / `~/.local/state`, + `fractalengine/cli`). Written atomically. `--verse/--fractal/--petal` win
  over it. `fe use instance <name|dir|tokens.json>` stores the instance in the context, which selects it later only
  when neither `--instance` nor `FE_INSTANCE` is set (meaningful with a per-agent `FE_CONTEXT`).
- Context-filled ids (`verse_id`/`fractal_id`/`petal_id`) are not `required` at flag parse time; `commands::merge_args`
  refuses a missing one with exit 6 `missing-context` ("run `fe use petal <id|name>` or pass --petal"), and
  `catalog::validate_args` treats an empty string as missing, so no request leaves with an empty id. `fe x` refuses a
  missing petal the same way.
- `--wait local` reads back on the same instance (`read_node` / `get_properties` / token file), never the hierarchy.
  `--wait peers[=a,b]` runs the same read-back on the other instances too (§wait-peers).
  `FE_WAIT_DEFAULT` applies to writes only: `local` / `peers` / `peers=a,b` wait, `none`/`off` does not, and any other
  value is downgraded to `local` with `wait.requested` + `wait.note` in the envelope, so a harness default never
  makes writes fail. `peers` from the default with no other instance discoverable degrades the same way; an explicit
  `--wait peers` with none exits 6 `no-peers` before anything is sent.
- Hierarchy reads are spaced at least 2 s apart per context (`hierarchy_read_at`, QA-0125: each read rebuilds the GUI
  tree).

## Envelope (`--json`, v1) and exit codes

`{v, ok, exit, code, message, data, retry_after_ms, refusal{package_id, reason}, op{verb, via, tool, method, path},
context{instance, verse_id, fractal_id, petal_id}, cid, latency_ms, wait}`. `data` is the MCP payload, or the REST
body's `data` member. Extension and role-floor refusals from `fe x` carry the host text verbatim in `refusal.reason`.

| Exit | Meaning |
|---|---|
| 0 | ok |
| 1 | CLI internal error |
| 2 | refused by policy, hook or extension (`refused`, `policy-denied`, `admin-gui-only`, `not-allowed-in-profile`) |
| 3 | transport: not running, non-loopback, token file missing/expired/rejected |
| 4 | role refused (`insufficient permissions`, a `… needs role X` role floor) |
| 5 | not found or not permitted (uniform by design; includes `delete_node`'s "operation failed or not permitted") |
| 6 | invalid input or usage (unknown command, missing context, bad flag, `--wait peers`, bad id, `token-in-args`) |
| 7 | retryable (429, write budget; `retry_after_ms`); `--retry` resends only per §retry |
| 8 | unknown outcome (504/timeout after send; reconcile by reading, never retried) |
| 9 | `--wait` budget expired (the write landed) |
| 10 | server fault / unexpected reply |
| 11 | command budget exhausted (`FE_BUDGET_MAX_COMMANDS`) |
| 12 | `fe replay`: a replayed step's exit code (or, under `--strict`, its data) differs from the journal |

Core verbs classify on stable server texts (`transport::classify`), because REST answers `200 {ok:false}` and MCP
errors are text only; a reworded server message can move an exit code, which the journey pins.

`fe x` calls use REST `POST /api/v1/petals/{p}/extensions/{pkg}/commands/{cmd}` with `{"args": …}`, not the MCP
`extension_command` tool (which flattens every `ExtError` to text), and `transport::classify_extension` maps the
HTTP status of fe-api's `ExtError` table (or a body `code`, when the API starts sending one): 400 → 6, 403 → 2
(`capability-denied`) or 4 when the text names a role floor (`… needs role X`, the one remaining text test), 404 →
5 for the uniform not-found text else 2 `not-available`, 409 → 2 `refused`, 429 → 7, 502 → 10, 504 → 8. The host
text is `refusal.reason` verbatim and `retry_after_ms` comes from the body or `Retry-After`, never invented.

## §retry

`--retry N` lives in the spine only (`transport::retry_allowed`): it resends an exit-7 outcome when the request was a
read, or when the reply proves the refusal happened before admission (`Outcome::pre_admission`). Only
`classify_extension` sets that: every 429 on the command route is a `HostRefusal` (the route gate, or `admit_call`
check 6 caller/command rate and check 8 queue full; fe-plugin `bus/broker.rs`), all taken before the call is queued,
while `CommandFailure` (post-admission) never maps to 429. A write whose 7 came from text, notably the MCP door's
"write budget exceeded; retry after N s" rewrite (fe-api `mcp/mod.rs`), is never resent: it may have run. Every
`fe x` call counts as a write. The rows are in `transport` `classify_maps_real_server_strings_to_the_exit_table`.

## Journal (JSONL, one line per invocation, `FE_JOURNAL`)

`{v, run, agent, seq, t (RFC3339 UTC), unix_ms (wall-clock epoch ms, same instant as t; not monotonic), latency_ms,
argv (secret flags blanked), resolved{verb, via, tool, args (redacted), method, path}, ctx_before, ctx_after,
instance{name, api_base, role}, exit, code, envelope_digest (blake3 hex of the printed envelope line), created_ids,
data_small (redacted, ≤2 KB else {truncated, bytes}), wait, help, note, cli_version, catalog_digest}`.
Capability values appear only as `<redacted blake3:<16 hex>>`.

**Budget counter.** `seq` and the budget come from a sidecar `<journal>.<blake3(agent)[..16]>.count` holding
`<counted> <lines>`, updated under an exclusive `File::lock` (std, Windows and Linux; bounded 5 s wait): lock, read,
refuse when `counted >= FE_BUDGET_MAX_COMMANDS`, count, unlock. Parallel invocations therefore cannot all pass, and
the journal is never scanned. With a budget set, any read/parse/lock/write failure (or a non-numeric budget) exits 11
`budget-unavailable` (fail closed); without one it only warns. A counter starts at zero beside an existing journal.

## Environment

| Variable | Effect |
|---|---|
| `FE_INSTANCE` | instance selector (name in `.qa/peers-current` peers.json, run dir, or tokens.json path) |
| `FE_CONTEXT` | per-agent context file path |
| `FE_JOURNAL` | append one journal line per invocation |
| `FE_AGENT_ID` | journal `agent`; selects this agent's budget counter |
| `FE_RUN_ID` | journal `run` |
| `FE_BUDGET_MAX_COMMANDS` | with `FE_JOURNAL`: exit 11 once this agent's counter reaches it (fail closed) |
| `FE_CLI_ALLOW` | comma list of allowed nouns (`note` always allowed); else exit 2 `not-allowed-in-profile` |
| `FE_WAIT_DEFAULT` | default wait for writes (see above) |
| `FE_CWD` | working directory for `.qa/` discovery and relative selectors |
| `FE_STATE_DIR` | CLI state root (context default, `fe x` cache keyed by api_base + petal + verse + caller role; served stale only when the instance is unreachable, never after a 401 or token expiry) |

## §wait-peers

`--wait peers[=a,b]` (and `FE_WAIT_DEFAULT=peers`) is the plan's "same read against every peer instance", not a server
digest: after a write the CLI polls the verb's read-back probe (`read_node` for create/move/delete, `get_properties`
for set/unset; `commands::settle`) on this instance and on every other member of `.qa/peers-current` (`peers.json`,
via `instance::list`), each with that member's own token file and the writer's verse id. No fe-api route reports a
cross-peer fingerprint (the fingerprint lives in the simulator driver), `GET /api/v1/debug/sync-state` is an
Owner-only operator gate that is off by default, and `GET /api/v1/network/diagnostics` reports connectivity, not
content; none of them answers "does this peer hold my write". `--wait-budget` (default 30 s) bounds the poll; expiry
exits 9 `wait-expired` (the write landed), naming the unseen peers, and `wait.peers[]` carries `{name, settled, ms}`.

- A peer is "another member": own instance excluded by token-file identity (peers on one machine have distinct token
  files). `peers=a,b` filters by name; a name nobody has exits 6 `unknown-peer` before the write.
- `NodeGone` only counts as converged when the peer holds a token for the verse (the uniform not-found would otherwise
  read as "deleted").
- A create-verse write waits locally (a verse reaches a peer only through an invite); a write with no read-back probe
  (fractal/petal create, phase-2 writes) is confirmed by its own reply and says `peers were not checked`.

## §phase-2 verbs

Rows with `phase == 2` in `catalog::VERBS` all run through `commands::generic::run`: `merge_args` (positionals,
context ids, vector normalisation), `tool_plan` (schema validation), `run_tool_verb`. Verbs: `node props|history`,
`asset upload --file`, `terrain get|set|clear`, `group list|set|delete`, `invite create`, `join`, `role
list|assign|revoke`, `net diag`, `identity`, `sync state`, `query`, `replay`. Per-verb shaping lives in
`generic::prepare` only:

- `asset upload`: `--file` is read, size-checked (11 MiB MCP cap) and base64-encoded in-crate; the instance's own token
  file is refused; a dry run elides the bytes.
- `terrain clear` = `set_petal_terrain` with `terrain: null`; `terrain set` refuses a missing config (it would clear).
- `invite create --to-instance X` reads X's DID with `get_identity` on X (loopback client, no token printed).
- `join --invite -|@file|<text>`: the capability never needs to sit in a process listing; journal argv blanks it.
- Global-scope and token-scope tools pick their own bearer (`join` uses the operator token); `sync state` needs a verse.
- `--dry-run` redacts secret-named keys of the planned args (`journal::redact`).
- `resolved.args` in the journal is now the final tool arguments (positionals and context merged), which is what
  `fe replay --emit-scenario` converts; phase-1 journals hold flag-only args and emit weaker scenarios.

## §replay

`fe replay <journal.jsonl> [--map agent=instance] [--bind old=new] [--strict] [--emit-scenario out.json
[--scenario-name n]]`. No LLM: every journal line is re-run as a child `fe` invocation (`execute_boxed`, in-process,
output captured) from its recorded `argv`, in recorded order, with `--verse/--fractal/--petal` from `ctx_before` so the
step needs no context file (a per-agent temp context file only holds the 2 s hierarchy spacing).

- Skipped (change no instance state): `help`, `use *`, local verbs (`ctx`, `instance`, `note`, `replay`), and lines
  refused before dispatch. A line whose argv carries `<redacted …>` exits 6 `replay-unbindable`.
- Id re-binding: `created_ids[0]` of a recorded create is paired with the id the replayed create returns; every later
  argv word and context id containing the recorded id (8+ chars) is rewritten. `--bind old=new` seeds the map (e.g. a
  fixture verse of the instance replayed against).
- Instances: `--map agent=instance` runs that agent's steps on that instance (`FE_INSTANCE`); unmapped agents use the
  invoking instance (`--instance`/`FE_INSTANCE`). Journals of replay steps go to `FE_JOURNAL` like any run.
- First exit-code mismatch stops the run with exit 12 `replay-diverged`; `data.diverged_at` names the step.
  `--strict` also compares `data_small` after normalising each id pair to the same token (use on deterministic reads).
- `--emit-scenario` (`commands::scenario::emit`) writes the `tools/qa/sim` DSL (scenario.py `SCHEMA`): peers from the
  journal agents (first owner is `owner`, others editor/viewer, an `observer` viewer is added when only one agent
  exists), one action per mutating or reading line (`via: mcp`, `op` = tool, `c<n>` ids, `expect` ok/refused/any from the
  recorded exit; hierarchy reads and `fe x` lines are skipped), created ids as `$c<n>.id`, the first verse and first two
  petals as `$fixture.verse_id|petal_a_id|petal_b_id`, a step per write with an `agree` expectation (node, transform,
  properties, groups or hierarchy probe). Other ids stay verbatim and are listed in `data.unbound_ids`. Validate with
  `python tools/qa/sim/scenario.py validate <path>`.

## Deliberately absent

- No remote mode: no TLS, no hostnames, no override flag.
- No consent, local acceptance or kill-list override: GUI only by design.
- No verse/fractal/petal delete or rename: the API has no such route.
- No moving the GUI's view: `fe use` changes only the CLI context.
- No automatic write retries (no idempotency keys exist); `--retry N` re-sends only exit-7 pre-admission refusals.

## Known limits

- The headless relay does not override `ExtensionHost::exposed`, so `fe x` lists nothing there (phase 5).
- Exit codes for non-extension verbs still depend on server message text until the API carries a machine `code`.

## §journey (R9-CLI)

`tests/cli_journey.rs` is one journey through `fe_cli::run` against a real `build_router_with_hosts` router served on
`127.0.0.1:0`, a real DB thread (`fe_database::spawn_db_thread_shared`, SurrealKV in a tempdir), the production
`spawn_api_command_relay` + `PendingApiRequests`, and token files written by the engine's own
`fe_api::dev_token::emit_dev_tokens`. The only stand-ins are a blob directory and the plugin host (`ExtensionHost`,
mirroring the bus's role-floor text). Legs: non-loopback refusal, hierarchy + node lifecycle with read-back, wait
default downgrade, viewer role refusal, out-of-scope not-found, missing petal context, `fe x` list/help/call/role
floor plus guest refusal (2), pre-admission rate refusal (7, resent by `--retry`), unknown outcome (8, never
resent), a forged help line and a bad petal id, the `token-in-args` refusals, `--wait peers` (6b: a viewer peer
converges, a peer with no verse token expires with 9 while the write landed, an unknown peer is refused, phase-2 dry
runs), replay (6c: record, replay with id re-binding, a tampered exit stops at 12, `--emit-scenario`), help routing for
every listed path, journal parse + no token text, and the snapshot drift leg (the embedded
`catalog/mcp-tools.json` must equal `fe_api::mcp::tool_specs()`; run once with `FE_CLI_BLESS=1` to re-bless, then
rebuild so the compiled-in copy matches). Batch commands: `conductor/test-batches.md` §R9-CLI.
