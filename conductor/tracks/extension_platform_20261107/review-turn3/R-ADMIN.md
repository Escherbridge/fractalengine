---
type: Review
track: extension_platform_20261107
gate: R-ADMIN
date: 2026-11-13
reviews: [A-PROTO, A-SRV, A-CLI, A-GUI]
security_review: true
---

# R-ADMIN: local admin channel (turn 3)

Scope: `docs/extensions/admin-channel.md`, `fe-plugin/src/admin/**`, `fractalengine/src/extension_host/admin_channel.rs`,
`tools/fe-ext/src/admin.rs`, `fe-ui/src/extensions/consent.rs`; the ops worker's `handle_admin` path
(`fractalengine/src/extension_host/ops_drain.rs`, S-HOST, R-GOV partition) was read for the consent and audit claims.
Mandatory security review, fresh context, prompted to refute: "only this OS user reaches the channel", "no path reaches
consent without a GUI click", "frames are capped", "no REST/MCP/token path", "audit actor is the local DID, channel cli",
"the CLI fails closed when the app is absent". No build or test run (code-first turn).

Verdict: **boundary sound in code; NOT live yet.** The transport admits only this OS user (protected owner-SID DACL +
client/server SID checks; 0700 dir + peer-uid on both ends), no verb or field can grant consent, caps hold before
allocation, and fe-api has no admin seam. Two gaps block "done": the listener is not compiled into the binary
(R-ADMIN-04) and the audit `channel`/refusal rows are not durable (R-ADMIN-05). Both are I-BIN's.

| # | Sev | Finding | Verdict |
|---|---|---|---|
| R-ADMIN-01 | MED | A CLI-origin consent dialog appears unasked; Enable armed one frame after it was drawn, so a click or double-click meant for what was under it could approve (consent.rs render). | FIXED: `CLI_ARM_DELAY_SECS` (1 s focused on screen, unfocus restarts; `confirm_enabled` folded with `armed` before input is read); journey extended with the arm leg; contract §5 states it. |
| R-ADMIN-02 | MED | `fe-ext admin` printed manifest-supplied text (name, version, status, messages) raw: ESC/OSC/bidi sequences could rewrite the terminal (e.g. fake "Approved", hide the digest). | FIXED: `terminal_safe` replaces control and bidi-override chars in every human line and the pending notice; `--json` was already escaped by serde; render test extended with a hostile reply. |
| R-ADMIN-03 | LOW | `MAX_PIPE_INSTANCES = 8` < 4 served + 4 busy responders + 1 waiting: the re-arm after a connect fails `ERROR_PIPE_BUSY`, accept loops every 250 ms with warnings and the connected client stalls. | FIXED: `2 * MAX_CLIENTS + 2` (pipe_windows.rs), rationale in `fe-plugin/src/admin/AGENTS.md` §pipe-instances. |
| R-ADMIN-04 | HIGH (completeness) | `admin_channel` is not declared in `fractalengine/src/extension_host/mod.rs` and `spawn` is never called: the listener is dead code and `fe-ext admin` always answers "not running". | HANDED-OFF (I-BIN / R-BIN): `pub(crate) mod admin_channel;` + `spawn(AdminChannelPorts::new(ops.admin_jobs(), gui_present))` inserted as a Resource; headless hosts pass `gui_present = false`. |
| R-ADMIN-05 | MED | Audit §7 unmet: `OpsPorts::audit_change_via` default drops `channel`, `BridgeOpsPorts` does not override it, `PolicyEntry` has no `channel` member, and `audit_admin_refusal` only logs (denials not durable). Actor is correct (`local_entry` stamps the local DID). | HANDED-OFF (I-BIN for plugin_bridge overrides; the fe-policy owner for the additive `channel` member). |
| R-ADMIN-06 | LOW | `read_package_file` opens the path before checking it is a regular file: a FIFO (Unix) or device path blocks the single ops worker (panel ops stall; the CLI gets `internal` after 5 min). Same-user only. | HANDED-OFF (S-HOST / R-GOV): `std::fs::metadata(path)?.is_file()` before `File::open`; `O_NONBLOCK` open on Unix. |
| R-ADMIN-07 | LOW | A CLI install arriving while another review is open is discarded by the panel (`DiscardStaged`) and the CLI is told `consent-declined: Declined in the app`, though the operator saw nothing. Fails closed. | HANDED-OFF (panel.rs owner, R-UI): answer `not-available` "another review is open in the app" for that case. |
| R-ADMIN-08 | INFO | `install-file` accepts any absolute path incl. UNC (`\\host\share`) and device paths; the app reads it as the same OS user. | KEPT: same user, no authority beyond §2 "what the channel trusts"; the package still goes through verify + GUI consent. |
| R-ADMIN-09 | INFO | Windows peer identity is PID → process token; a PID reused between connect and check could be misattributed. | KEPT: the DACL is the boundary (only this user can open the pipe); the SID check is defence in depth and fails closed when the process is gone. |
| R-ADMIN-10 | — | Refutation attempts that held: no consent/bind/override verb, `deny_unknown_fields` on envelope and args (token/actor/digest are `bad-request`); worker's stray `pending-consent` dropped for non-consent verbs and duplicates; reply id stamped by the listener; 64 KiB checked on the prefix; 30 s read timeout covers slow bodies; 4 clients + `busy`; headless `spawn` returns Off and `answer` refuses `consent-needs-gui`; already-consented update auto-activates only with unchanged digest AND publisher and no kill-list entry (ops_drain `consent_request`); `enable` without consent answers `consent-missing`; fe-api `administer` refuses every admin verb; no fe-api/MCP crate references `fe_plugin::admin`; CLI has no `--url`/`--token`, `NotRunning` exits non-zero. | KEPT (verified). |
