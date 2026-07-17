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

- **Op-log entries are not yet signed.** The op-log format reserves an
  ed25519 signature field, but production write paths currently stamp a
  placeholder (all-zero) signature — including session-revocation entries.
  Do not rely on op-log signatures for integrity or non-repudiation yet.
- **The sync write gate is permissive.** Write-path policy enforcement
  (fe-policy) is wired but currently runs in a permissive mode pending the
  strict flip. Peers you sync with should be treated as trusted.
- **The plugin sandbox is young.** Rhai and WebAssembly plugins run with
  operation limits, fuel metering, and capability manifests, but the
  sandbox has not had an external audit. Only install plugins you trust.

## Scope

The attack surface includes: libp2p/iroh P2P networking, the HTTP/WebSocket
API gateway (default port 8765), the embedded WebView portal, JWT-based
auth and identity, the plugin host, and the embedded database. Reports on
any of these are in scope, as are supply-chain concerns in the dependency
tree.
