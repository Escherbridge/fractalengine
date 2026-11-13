---
type: reference
title: fe-plugin/src/admin — local extension-administration channel
track: extension_platform_20261107
slice: A-PROTO (turn 3)
---

# fe-plugin/src/admin — local admin channel

Contract: [`docs/extensions/admin-channel.md`](../../../docs/extensions/admin-channel.md). This module is the
shared half: the wire protocol and the OS-user transport. The app's listener
(`fractalengine/src/extension_host/admin_channel.rs`, A-SRV) and the `fe-ext admin` client
(`tools/fe-ext/src/admin.rs`, A-CLI) both build on it and on nothing else, so the two ends cannot drift.

| File | Owns |
|---|---|
| `mod.rs` | Re-exports, `FE_EXT_ADMIN_CHANNEL` switch, `MAX_CLIENTS`, `REQUEST_READ_TIMEOUT`, `CONSENT_WAIT`. |
| `protocol.rs` | `AdminRequest` / `AdminVerb` / `AdminReply` / `RefusalCode`, frame read/write (64 KiB cap), request decode + validation, reply encode. Pure, transport-agnostic. |
| `transport.rs` | `LocalEndpoint`, `listen_local`, `connect_local`, `AdminListener` (the one admission point), `AdminStream`, the in-memory listener, server helpers `next_request` / `write_reply`, client helper `exchange`. |
| `pipe_windows.rs` | Named pipe with an owner-SID-only protected DACL, `reject_remote_clients`, `first_pipe_instance`, client- and server-SID checks. |
| `socket_unix.rs` | Socket in a 0700 user-owned directory, socket 0600, peer-uid check on both ends. |
| `tests.rs` | Batch R11-ADMIN (`cargo test -p fe-plugin -- admin::tests::`). |

## §why-here

fe-plugin is the one crate both the app binary and `fe-ext` already depend on, and it pulls in neither
fe-api nor fe-ui. Keeping the admin protocol out of fe-api is the point: the channel is not an API surface
and must never be mounted on the network listener (user decision 2026-11-09 06:30).

## §admission — one place, fail closed

`AdminListener::accept` is the only place a connection is admitted, for every transport including the
in-memory one. The platform layer reports `PeerUser::Same` or `PeerUser::Other`; anything it cannot
identify (process gone, token unreadable, `peer_cred` failing) is `Other`. A-SRV's foreign-user tests use
`memory_listener()` + `MemoryConnector::connect(PeerUser::Other(..))` and so exercise the same branch the
pipe and socket hit.

Defence in depth, per platform:

- **Windows.** The DACL is the boundary (`D:P(A;;GA;;;<user SID>)`: protected, no inherited ACEs, nobody
  but the user — not Administrators, not SYSTEM). `reject_remote_clients` refuses SMB clients.
  `first_pipe_instance(true)` makes a squatted name an error instead of joining someone else's pipe. The
  server then checks the client process's token SID, and the client checks the server's
  (`GetNamedPipeServerProcessId`) before sending anything. Low-integrity and AppContainer processes of the
  same user are stopped by the default medium mandatory label and by the DACL (no package SID is granted).
  The client keeps tokio's default `SECURITY_IDENTIFICATION` QoS, so the server cannot impersonate it.
- **Unix.** The directory (`$XDG_RUNTIME_DIR/fractalengine`, else `<temp>/fractalengine-<uid>`) is
  created 0700 and re-checked with `lstat`: a symlink, another owner or group/other bits refuse the bind.
  A user-owned directory left looser by an older build is tightened to 0700 first. The socket is chmod 0600;
  both ends compare `peer_cred().uid()` with `geteuid()`.

## §frames

4-byte big-endian length, then a UTF-8 JSON body of 1..=65536 bytes. `read_frame` checks the prefix
before allocating, so a hostile length costs nothing. A bad frame cannot be resynchronised: the server
answers once (`frame-too-large`, `bad-request`) and closes. Replies that would exceed the cap go out as
`reply-too-large`, so the server always has something sendable.

## §consent

No verb grants consent or local acceptance; `deny_unknown_fields` on both the envelope and the verb
arguments means a `consent`, `bind`, `token` or `actor_did` field is a `bad-request`, never ignored.
`AdminVerb::may_raise_consent` marks the three verbs (install-file, install-registry, update) that may
answer `pending-consent`; `exchange` refuses a pending-consent for any other verb and a second one for the
same request, so a confused or hostile server cannot make the CLI wait forever on a dialog that does not
exist. The grant itself happens only through the GUI dialog (`ConsentOrigin::Cli`, A-GUI), whose Enable
arms only after a focused second on screen (R-ADMIN): an unsolicited dialog must not inherit a click.
`fe-ext admin` passes every reply string through `terminal_safe` before printing, because names and
messages come from package manifests (R-ADMIN).

## §pipe-instances

`MAX_PIPE_INSTANCES = 2 * MAX_CLIENTS + 2`: the listener holds up to `MAX_CLIENTS` served connections,
up to `MAX_CLIENTS` `busy` responders and one waiting instance. With fewer instances the re-arm after a
connect fails with `ERROR_PIPE_BUSY` and the next client stalls until a slot frees (R-ADMIN).

## §instances

Several apps of one OS user (QA peers, the simulator) each set `FE_EXT_ADMIN_INSTANCE=<name>`
(`[A-Za-z0-9_-]{1,32}`), which suffixes the pipe/socket name; `fe-ext admin` reads the same variable. A
second app on a live endpoint fails to listen (the pipe's first-instance flag, the socket's connect probe)
and runs without the channel; it never takes the first one's clients.

## §deps (owned by I-PLUGIN)

`windows-sys` 0.59 under `cfg(windows)` with `Win32_Foundation`, `Win32_Security`,
`Win32_Security_Authorization`, `Win32_System_Pipes`, `Win32_System_Threading`; `libc` 0.2 under
`cfg(unix)` (`geteuid`). tokio's `net` feature (workspace default here) provides the named pipe and the
Unix socket; `io-util` the frame helpers.
