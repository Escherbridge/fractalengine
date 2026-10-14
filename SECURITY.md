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
  ed25519 signature field, but 21 production write paths in `fe-database`
  stamp a placeholder (all-zero) signature, session revocation among them.
  Per decision D-CL24(5), these pre-canonical `op_log` rows are permanently
  unsigned and untrusted: there is no signature backfill. Signed operations
  belong to the canonical log. Do not rely on `op_log` signatures for
  integrity or non-repudiation. The site-by-site register is in
  `conductor/tracks/oss_release_20260717/spec.md` (FR-2). It was
  re-verified 2026-10-11 with `git grep -n '"00"\.repeat(64)' -- '*.rs'`,
  which returned 22 hits: 21 production sites plus 1 test fixture.
- **Known dependency advisories are triaged, not yet fixed.** `deny.toml`
  explicitly ignores these RustSec IDs, each with a reason:
  - 19 for `wasmtime` 29 in the dormant WASM plugin host. Each needs a major
    upgrade.
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
  the verse scope (fe-policy, deny-by-default). Residuals: rows for a verse
  whose manifest has not arrived yet are admitted on the replica capability
  alone (bootstrap window); an admitted row's references into other verses
  are not yet checked; and compute/ledger gossip frames relayed through a
  neighbour cannot be attributed on the wire, so they fall back to
  claim-based admission. Treat anyone holding a verse's namespace
  capability as able to write to it.
- **The plugin sandbox is young.** Rhai and WebAssembly plugins run with
  operation limits, fuel metering, and capability manifests, but the
  sandbox has not had an external audit. Only install plugins you trust.

## Scope

The attack surface includes: libp2p/iroh P2P networking, the HTTP/WebSocket
API gateway (default port 8765), the embedded WebView portal, JWT-based
auth and identity, the plugin host, and the embedded database. Reports on
any of these are in scope, as are supply-chain concerns in the dependency
tree.
