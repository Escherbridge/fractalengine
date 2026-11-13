# tools/qa/sim

The scripted P2P simulator. The operator guide, scenario DSL and report format live in `tools/qa/AGENTS.md` §sim; this
file records module ownership and the extension (`ext`) design.

## Modules

| File | Owns |
|---|---|
| `run.py` | Stage, peers, loopback, extension staging (`stage_extension_packages`), driver and analysis in order. |
| `driver.py` | Action dispatch by `via` (`rest`, `mcp`, `gui`, `host`, `macro`, `ext`); extension REST doors; `_x_*` ext handlers. |
| `loopback.py` | Static site, HTTP sink, MQTT broker, three run-seeded keys (index, revocation root, publisher), `loopback.json`. |
| `scenario.py` / `scenario.schema.json` | DSL validation. `python scenario.py schema` regenerates the schema (never hand-edit). |
| `prober.py` | Probe loops; the `extension` probe reads `GET /api/v1/extensions[/{installation}]`. |

## §ext

**Why a separate via.** Extension administration is a GUI surface plus a local CLI channel, never REST or MCP (user
decision 2026-11-09; `docs/extensions/governance.md` §Local identity). A scenario that installed over REST would have
tested a door the product refuses. `via: "ext"` keeps the scenario text honest about the door, and keeps the old REST
ops (`request`, `extension_command`, `ingress_post`...) for what REST still serves.

**GUI path.** `_x_open_panel` presses `Toggle Extensions panel` only when no panel tab is visible, then the view tab.
Install is the Browse row `Install <package_id> <version>` (version from `$run.packages`), or the file field with
`via: "file"`. After the press the panel is classified by what it shows: `Enable <package_id>` (consent needed: ok),
`Override kill list for <package_id>` (kill-listed: refused `revoked`, dialog cancelled), or the `Dismiss extension error`
row (refused with the contract's kebab reason read verbatim). `ext_consent` clicks the dialog's `Enable <package_id>`
(`Update <package_id>` with `reprompt`) and then reads the installation id from `GET /api/v1/extensions`; later steps
use `$<consent id>.installation`. `ext_kill_override` types the package id into `Confirm package id to override kill
list` and presses the override, which leaves a fresh consent dialog for `ext_consent`.

**CLI path.** `_x_cli` runs `fe-ext admin --json <verb>` and turns the last stdout line (the final reply frame) into
an outcome: `ok` -> result, `refused` -> its code. The driver never answers `pending-consent`; the dialog is the app's.

**Staging.** `run.py` packs only the examples a scenario names. `EXAMPLE_PROJECTS` maps names to `extensions/<name>`;
`city-map` and `city-map-legacy` (ext-bindings) have no example crate yet, so staging stops with a clear error for
them. The publisher and index keys are seeded from the scenario name and seed, so a rerun of the same scenario signs
identically; they are written under `<run>/extensions/keys/` and are never the revocation root (an index key that is a
trust root is refused, fe-hexon `IndexKeyMisused`).

**Known gaps.** The panel has no binding config form, so `ext_bind` enables the manifest's binding rows and records the
scenario's `config` in `config_note` instead of sending it. `door: ws` commands still ride REST (no WS client).
Install, consent and update have no CLI route in the driver: `fe-ext admin install-*` blocks on `pending-consent` until
the dialog is answered, and the driver would have to click it from a second thread; the GUI route covers them.
`ext-install-gui` has no update leg: run.py packs one build per example, so there is no newer version to offer
(trigger: an example with a second staged version).

**CLI routing.** The CLI reaches whichever app owns the default admin endpoint (the first to start, the owner in
`peers.ps1` order) unless `instance` names one. A verb misrouted to another peer names an installation id that only
exists on the intended host, so it answers `not-found` (loud), never acts on the wrong host.

**`ext-install-gui`.** The GUI QA path in one run: file install (fleet-sim) and Browse install from the signed static
index (device-adapter), both consented by the dialog's own button, a CLI `disable` and a panel `enable`, then both
uninstalled; the viewer proves installations stay host-local. Two peers because `run.py` needs at least two.
