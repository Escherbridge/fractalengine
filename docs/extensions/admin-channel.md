---
type: reference
title: Local admin channel contract (fe-ext admin)
track: extension_platform_20261107
status: draft (turn 3, slice A-PROTO; listener A-SRV, client A-CLI, consent origin A-GUI, gate R-ADMIN)
consumers: [extension_platform_20261107]
---

# Local admin channel

Extension administration is a **GUI surface**. The `fe-ext admin` command drives the same operations
from a terminal, but only through a **local channel to the running app**, bound to the OS user. It is
never the network API. REST and MCP keep refusing every administration verb for every caller
(user decision 2026-11-09 06:30). A capability grant always needs the operator's click in the app's
consent dialog. The channel can ask for that dialog. It can never answer it.

Code: `fe_plugin::admin` (protocol + transport, shared by both ends),
`fractalengine/src/extension_host/admin_channel.rs` (the app's listener),
`tools/fe-ext/src/admin.rs` (the CLI).

## 1. When the channel exists

| Host | Channel |
|---|---|
| GUI app running | On, unless `FE_EXT_ADMIN_CHANNEL=0` (exactly `0`; any other value or unset leaves it on). |
| Headless host (no GUI) | Off. Consent needs the GUI, so the channel would be half a feature. |
| Second app of the same OS user on the same endpoint | That app runs without a channel and logs why; it never takes over the first app's channel. Set `FE_EXT_ADMIN_INSTANCE=<name>` (1-32 of `A-Z a-z 0-9 _ -`) on each app and on the CLI to run several. |

If a listener is somehow running on a host with no GUI, consent-needing verbs answer `consent-needs-gui`.

## 2. Transport: this OS user only

| | Windows | Unix (Linux, macOS) |
|---|---|---|
| Endpoint | Named pipe `\\.\pipe\fractalengine-admin-<user SID>[-<instance>]` | Socket `$XDG_RUNTIME_DIR/fractalengine/admin[-<instance>].sock`; without `XDG_RUNTIME_DIR`, `<temp dir>/fractalengine-<uid>/…` |
| Who may connect | Protected DACL `D:P(A;;GA;;;<user SID>)`: the user's SID only, no inherited entries | Directory 0700 owned by the user (checked with `lstat`, so no symlink), socket 0600 |
| Remote clients | Rejected (`PIPE_REJECT_REMOTE_CLIENTS`) | Not possible (local socket) |
| Server checks client | Client process token SID equals the app's | `peer_cred().uid` equals the app's effective uid |
| Client checks server | Server process token SID equals the CLI's, before sending | `peer_cred().uid` equals the CLI's effective uid, before sending |
| Name squatting | `first_pipe_instance`: an existing pipe of that name is an error, never joined | A live socket at the path is an error; a stale one is removed (the directory is private) |

The app admits a connection only when the peer is this OS user. Anything it cannot identify is refused,
and refusals are logged and counted. A refused client sees the connection close. It gets no reply, so
it learns nothing about the host.

**What the channel trusts.** Any process running as this OS user can already read this user's key
files and run the app as itself. The channel adds no authority beyond that, and it hands out no
grant (§5). Low-integrity and AppContainer processes on Windows are outside the DACL and the default
mandatory label.

## 3. Frames

- Each frame is a 4-byte big-endian length followed by a UTF-8 JSON body of 1 to 65 536 bytes (64 KiB).
- The reader checks the length before it allocates.
- A frame of length 0, over the cap, or cut off mid-body is refused. The server answers once
  (`frame-too-large` or `bad-request`, `id` empty) and closes the connection, because the stream cannot
  be resynchronised.
- A connection carries requests one at a time. The client sends a request and reads until that
  request's final reply before it sends the next. There is no pipelining. `fe-ext admin` sends one
  request per invocation.
- The app closes a connection that sends no request within 30 s (`REQUEST_READ_TIMEOUT`).
- At most 4 connections are served at once (`MAX_CLIENTS`). A fifth gets `busy` and is closed.

## 4. Requests

```json
{"v": 1, "id": "c1", "verb": "install-file", "args": {"path": "C:\\pkgs\\fleet-sim.hexon"}}
```

- `v` is the protocol version, exactly `1`. Any other value answers `unsupported-version`.
- `id` is chosen by the client: 1 to 64 characters of `A-Z a-z 0-9 _ -`. Every reply frame echoes it.
- `args` is an object and may be omitted for verbs without arguments.
- Unknown members are refused (`bad-request`), at the top level and inside `args`. A request cannot
  carry a token, a DID, an actor or a consent digest, so a stray field is never silently ignored.

| `verb` | `args` | Does | May raise the consent dialog | Audit `action` |
|---|---|---|---|---|
| `list` | — | Every installation on this host | no | — |
| `status` | `installation_id` | One installation | no | — |
| `install-file` | `path` (absolute; the app does not share the CLI's working directory) | Verify and stage the package file, as the panel's Install from file | **yes** | `install` |
| `install-registry` | `source_id`, `package_id`, `version?` | Fetch, verify and stage from a configured registry source (store-client contract v1: anonymous fetch, pinned index key, `detail_hash`, kill list at install) | **yes** | `install` |
| `enable` | `installation_id`, `binding_id?` | Switch one binding on, or every binding of the installation when `binding_id` is absent | no | `enable` |
| `disable` | `installation_id`, `binding_id?` | Switch one binding off, or all of them | no | `disable` |
| `update` | `installation_id` | Stage the newer version. Same capability digest: activates with no prompt. Changed digest or publisher: raises the dialog, and the old version keeps running until consent (governance §Consent) | **yes** | `update` |
| `unbind` | `installation_id`, `verse_id` | Withdraw this host's local acceptance of the installation in that verse (bindings.md §5.3) | no | `unbind` |
| `uninstall` | `installation_id` | Remove the installation | no | `uninstall` |
| `disable-all` | — | The runtime host switch (governance §Host switch) | no | `disable`, `target: "*"` |
| `enable-all` | — | Clear the runtime switch (`FE_EXT_DISABLE_ALL=1` at startup still wins) | no | `enable`, `target: "*"` |

**Argument rules.**

- Ids (`installation_id`, `binding_id`, `verse_id`, `source_id`) are 1 to 256 bytes, with no
  whitespace and no control characters.
- `package_id` follows store-client contract v1: `^[A-Za-z0-9][A-Za-z0-9_.-]{0,127}$`, and it must
  not end in `.json`.
- `version` is SemVer with no build metadata.
- `path` is at most 4096 bytes and must be absolute.

A violation answers `invalid-input`.

**Not on the channel.** These actions are GUI only:

- consent;
- local acceptance ("Allow in <verse>", audited as `bind`);
- the kill-list Owner override;
- revoking consent;
- discarding a staged build;
- binding rows.

They are not verbs, so a request naming them is a `bad-request`. Withdrawing things (disable, unbind,
uninstall) is allowed from the terminal. Granting things is not.

## 5. Replies and the consent flow

```json
{"v": 1, "id": "c1", "status": "pending-consent", "consent": {"installation_id": "01J…", "package_id": "org.example.fleet-sim", "name": "Fleet sim", "version": "1.2.0", "capability_digest": "b71e…", "is_update": false}}
{"v": 1, "id": "c1", "status": "ok", "result": {"installations": [{"installation_id": "01J…", "package_id": "org.example.fleet-sim", "name": "Fleet sim", "version": "1.2.0", "status": "Running", "bindings": []}]}}
{"v": 1, "id": "c1", "status": "refused", "code": "consent-declined", "message": "Declined in the app"}
```

- Every request gets exactly one final reply: `ok` or `refused`. A request whose verb may raise the
  dialog can first get exactly one `pending-consent`. The client refuses any other interim frame.
- `ok.result.installations` lists what the verb touched after it ran:
  - every installation for `list`, `disable-all` and `enable-all`;
  - the one installation for the other verbs;
  - nothing for `uninstall`.
- `message` is an optional human line.
- **Consent.** When a staged build needs consent, the app does the following:
  1. It opens the same consent dialog the panel opens, with `ConsentOrigin::Cli`: "Requested from
     the command line", the window asks for attention, and the capability review is identical. The
     dialog never approves on its own.
  2. It answers `pending-consent`.
  3. It waits for the operator. Approve leads to the verb completing and `ok`. Decline (or Discard)
     answers `refused` / `consent-declined`.
- If the operator has not answered after 15 minutes (`CONSENT_WAIT`), the app answers
  `consent-timeout`. The dialog stays open, and the operator can still decide there.
- If the CLI disconnects while it waits, nothing is cancelled. The dialog and its outcome belong to
  the GUI.
- Already-consented digests take no prompt (same rule as the panel), so `update` with an unchanged
  digest answers `ok` directly.

## 6. Refusal codes

| Code | Meaning |
|---|---|
| `host-off`, `package-invalid`, `untrusted-signer`, `revoked`, `entitlement-required`, `policy-denied`, `role-denied`, `consent-missing`, `not-available`, `not-found`, `invalid-input` | As in [governance.md](governance.md) (activation preconditions and administration refusals). `consent-missing` answers `enable` on an installation with no consent in force. Consent is given in the app. |
| `bad-request` | Not JSON, missing `v`/`id`, unknown verb, unknown member, empty frame |
| `unsupported-version` | `v` is not 1 |
| `frame-too-large` | Request frame over 64 KiB (the connection then closes) |
| `reply-too-large` | The reply would exceed 64 KiB. Ask `status` per installation. |
| `consent-needs-gui` | No GUI to show the consent dialog |
| `consent-declined` | The operator declined or discarded in the app |
| `consent-timeout` | No answer in the app within `CONSENT_WAIT`. The dialog stays open. |
| `busy` | All connection slots are in use |
| `internal` | The app failed. The message says how. |

A client shows a code it does not know verbatim.

## 7. Identity and audit

- The CLI never names an identity. Every action runs as the host's **local DID**, the identity the
  extension writes as (governance §Local identity). The OS-user check on the channel stands in for
  the GUI's "the operator is at this machine".
- Every mutating verb appends exactly one governance §Audit entry, denials included. The entry has:
  - `actor_did`: the local DID;
  - `channel: "cli"`, where GUI actions record `"gui"`.

  `channel` is an additive audit member (see contract changes).
- Read-only verbs (`list`, `status`) are not audited.
- The channel has no rate limit. It is one local user, at most 4 connections, with one request in
  flight per connection.

## 8. What the CLI must do (A-CLI)

- Connect only through `fe_plugin::admin::transport::connect_local`. Offer no network fallback, and
  no `--url` or `--token` flag.
- When the app is not running, say so plainly (`ConnectError::NotRunning` names the endpoint) and exit
  non-zero.
- Turn a relative `install-file` path into an absolute one before sending it.
- On `pending-consent`, print where to look ("Review the request in the FractalEngine window") and
  keep waiting.
- Offer human output by default and `--json`, which prints the final reply frame as received.

## 9. Versioning

`v` changes only for a breaking change. A new verb, a new optional argument or a new reply member is
additive within v1. An older app refuses a newer verb as `bad-request` ("unknown variant"), and an
older CLI ignores reply members it does not know.
