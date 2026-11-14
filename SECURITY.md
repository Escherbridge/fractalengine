# Security Policy

## Reporting a Vulnerability

Please report vulnerabilities privately via **GitHub private vulnerability
reporting**: on the repository's **Security** tab, choose **Report a
vulnerability**. Do not open a public issue for anything you believe is a
security problem.

Include what you can: affected component (P2P networking, API gateway,
WebView, auth, plugin sandbox, storage), reproduction steps or a proof of
concept, and the impact you believe it has.

## Response Expectations

FractalEngine is pre-1.0 and maintained by a small team. Our targets:

- **Acknowledgement** within 7 days of a report.
- **Triage and initial assessment** within 14 days.
- **Fix or documented mitigation** on a timeline agreed with the reporter;
  we ask for coordinated disclosure until a fix ships.

## Supported Versions

Only the latest release and the `main` branch receive security fixes.
There are no backport branches before 1.0.

## Known Limitations (pre-release)

These are known, publicly acknowledged gaps in the current codebase. They
are tracked for resolution before a production-ready release; reports that
merely restate them will be deduplicated against this list.

- **Legacy op-log entries are unsigned.** The op-log format reserves an
  ed25519 signature field, but 36 production write paths in `fe-database`
  stamp a placeholder (all-zero) signature, session revocation among them.
  Per decision D-CL24(5), these pre-canonical `op_log` rows are permanently
  unsigned and untrusted: there is no signature backfill. Signed operations
  belong to the canonical log. Do not rely on `op_log` signatures for
  integrity or non-repudiation. The site-by-site register is in
  `conductor/tracks/oss_release_20260717/spec.md` (FR-2). It was rebuilt
  2026-11-13 with `git grep -n '"00"\.repeat(64)' -- 'fe-*' 'fractalengine*' tools extensions`,
  which returned 51 hits: 36 production sites, 14 test fixtures, 0 checks
  and 1 documentation mention. New write paths added since keep the same
  placeholder, so the statement above holds and the count is not a stable
  figure.
- **Known dependency advisories are triaged, not yet fixed.** `deny.toml`
  explicitly ignores these RustSec IDs, each with a reason:
  - None for `wasmtime`: the 19 ignores for 29.0.1 were retired when
    `fe-plugin` moved to 49.0.2 (2026-11-08). The WASM host executes untrusted
    code once enabled, so a new `wasmtime` advisory is not ignorable; bump
    instead.
  - Several transitive advisories whose patched release would re-resolve
    other crates: `h2`, `rustls`, and `webbrowser` via bevy_egui. (`ammonia`
    and `quinn-proto` were upgraded to fixed releases on 2026-10-13.)
  - Advisories with no fix in the line our dependencies pin:
    `quick-xml` 0.39 via tauri's plist reader; `rsa` (RUSTSEC-2023-0071,
    no upstream fix; reached only through jsonwebtoken, while FractalEngine
    signs and verifies EdDSA only); and `ruzstd` 0.8.3 (RUSTSEC-2026-0344,
    decompression bomb). Bevy 0.18 through 0.20 pins `ruzstd` ^0.8, and the
    fix exists only in 0.9.1. Unguarded, a GLB that embeds a KTX2 texture
    with Zstandard supercompression would be decoded by `ruzstd` when it
    loads, so a peer who can share a GLB into a verse you open could crash
    the GUI with an out-of-memory error (a denial of service, not code
    execution). **Interim mitigation (2026-10-13): KTX2 and Basis textures
    are refused.** A guard parses the GLB container and its glTF JSON (a
    JSON glTF too, whatever its file extension), percent-decodes image URIs
    as Bevy does, and refuses the asset if any image is `image/ktx2`, uses
    `KHR_texture_basisu`, points at a `.ktx2`/`.basis` file, or holds KTX2
    bytes. It runs at three points. The first is the `blob://` asset reader,
    which every blob-store asset passes before Bevy decodes it (REST and MCP
    uploads, peer sync, hexon installs, local imports, path-asset stamps).
    The other two are REST/MCP upload (HTTP 422, so the refused bytes are
    never stored) and local import. The guard refuses rather than strips,
    so a KTX2-textured model cannot be used until the fix ships. Assets
    loaded from the app's own `assets/` folder are trusted local files and
    are not guarded. We will lift the guard and the ignore once a Bevy
    release requires `ruzstd` >= 0.9.1.

  CI runs `cargo deny --all-features check advisories bans sources licenses`.
  A new advisory or a licence outside the `deny.toml` allow-list shows up as
  a warning on the lint job and in the run summary, but it does not block
  the build or the rolling download.
- **Peer write admission has residual gaps.** Inbound replicated rows are
  admitted on the database thread only when their author holds Editor+ at
  the verse scope (fe-policy, deny-by-default), the row's references all
  resolve inside the replica's verse (a reference into another verse is
  denied; one not yet known locally waits), and every row other than the
  verse manifest waits until the manifest has landed. Residuals: the first
  manifest of a verse is accepted on trust at first use (it must name a
  did:key owner, and a store joined by invite already holds a pinned
  manifest); and gossip frames relayed through a neighbour are attributed by
  claim until signing is enforced (see S-1 below). Treat anyone holding a
  verse's namespace capability as able to write to it until its manifest is
  known.
- **S-1: gossip relayed attribution, closed under signing but not by
  default.** Each gossip frame (`FEG1`, `fe-sync/src/gossip_auth.rs`) now
  carries an ed25519 signature over verse, author, HLC, kind and payload.
  Ingress verifies it before the compute, tileset and reachability handlers
  run (`open_ingress`), so a relayed frame is attributed to its signer rather
  than to a claimed name. The default
  `FE_SYNC_GOSSIP_SIGNING=compat` still admits unsigned legacy frames under
  the older rules while their claimed author is not latched as a signer.
  Set `FE_SYNC_GOSSIP_SIGNING=require`, or `gossip_signing: "required"` in a
  verse manifest, to refuse unsigned frames. Compat is kept for one release
  so peers can upgrade together. The default flips to `require` at a named
  trigger: the next minor release, or once every declared peer is latched.
  Residuals after signing: a relay operator still sees which nodes connect
  and traffic metadata (see the next item). A node's identity is its endpoint
  key, so after a key rotation the old DID's frames stop being admitted once
  its role statements are revoked.
- **Embedded relay residuals.** The members-only relay
  (`docs/relay-operations.md` §6) admits only current members, but it runs
  the membership check after the connection opens. Its operator sees which
  members connect, their addresses and traffic metadata, and the start-up
  warning says so. FractalEngine does not set the upstream iroh-relay
  `accept_conn_limit`, so a connection flood is bounded only by the
  per-endpoint and client caps and by the firewall in front of the relay.
- **The plugin sandbox is young and has not had an external audit.** Rhai
  and WebAssembly plugins run with operation limits, fuel metering, and
  capability manifests. `network_scope` and `external_http` are denied at
  parse and at use, with no grant path in v1. The host thread runs in every
  build but admits no package unless the binary has the `plugin-host` build
  feature and starts with `FE_PLUGIN_HOST_ALLOW=1`. An admitted package must
  carry an ed25519 publisher signature from a configured trust root. The
  unsigned-package dev escape exists only in debug builds. Paid (licensed)
  packages are checked offline against a stored licence token from a pinned
  issuer, bound to this device's key; builds without the `entitlement`
  feature (the default) or without a device key refuse every licensed
  package. Each revocation-capable trust root's signed revocation list is
  pulled anonymously with a size cap, and kill-listed builds are refused at
  offer, install and load. Once enabled the host runs plugin code, so only
  install plugins you trust.

## Scope

The attack surface includes: libp2p/iroh P2P networking, the HTTP/WebSocket
API gateway (default port 8765), the embedded WebView portal, JWT-based
auth and identity, the plugin host, and the embedded database. Reports on
any of these are in scope, as are supply-chain concerns in the dependency
tree.
