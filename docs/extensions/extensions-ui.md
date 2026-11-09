# Extensions UI

Design text for the Extensions panel (spec R28), Browse and the registry sources behind it (R29, R32), and the update flow. It applies `C:/Users/atooz/.gemini/UI-UX-guidance.md`: one purpose per view, one obvious primary action, layout before copy, stable accessible names, and real empty, loading, error and disabled states.

Code: `fe-ui/src/extensions/{panel,details,permissions,browse}.rs` (UI), `fe-hexon/src/registry_source.rs` (sources). Consent itself is `consent.rs` (E11); this panel only opens it and routes its answer.

## Views

| View | Purpose | Primary action | Secondary |
|---|---|---|---|
| Installed | See what is installed and its state | Install from file | Open a row |
| Details > Overview | Understand one extension and run it | Enable / Disable a binding (Update when one is offered) | Uninstall (two-step) |
| Details > Permissions | See what it was allowed to do | none (read-only record) | Revoke consent (two-step) |
| Browse | Find a package in a source | Install on a row | Source picker, search, Refresh |

The consent dialog is not a view: it opens over the panel whenever the bridge reports `ConsentNeeded`.

## States

Every view renders each state from data, never from copy that explains the layout.

- **Empty.** Installed: `No extensions installed` above the install field, which stays usable. Browse without sources: `No extension sources are configured`. Bindings, counters and audit each say what is absent in one short line.
- **Loading.** A spinner row with the bridge's current task (`Verifying package...`, `Listing...`). While a task is in flight every state-changing button is disabled; read-only navigation stays live.
- **Error.** One red line with the bridge's message verbatim (`package-invalid (bad-signature)`), and `Dismiss`. Refusals are never paraphrased: they are the contract's reason strings.
- **Disabled.** A button that cannot act is disabled, not hidden. Blocked rows (revoked build, policy, role) show `Blocked: <reason>` and their binding buttons are disabled; Uninstall stays available.
- **Offline Browse.** `Offline: showing the last verified list` when the static index came from cache.

## Accessible names

Names are stable and carry the package id so an agent or screen reader can address one row among many.

`Installed extensions`, `Browse extensions`, `Package file path`, `Install from file`, `Open <package_id>`, `Back to installed extensions`, `Overview of <id>`, `Permissions of <id>`, `Enable|Disable <binding label> for <id>`, `Update <id> to <version>`, `Uninstall <id>` then `Confirm uninstall <id>` / `Keep <id>`, `Revoke consent for <id>` then `Confirm revoke consent for <id>` / `Keep consent for <id>`, `Extension source`, `Search extensions`, `Refresh extension list`, `Install <id> <version>`, `Dismiss extension error`.

## Copy rules

- Status words: Enabled, Disabled, Blocked: reason, Needs consent.
- Trust words: `trusted publisher`, `publisher not trusted`, `unsigned`. Never "safe".
- Destructive actions state the consequence once, at the confirm step: `Remove <name> and stop all its bindings?`, `Stops every binding until you allow it again.`
- No taglines. A row is `<name> <version>`, status, then `<publisher> · <trust>` and, when relevant, `update <v> available` or the update stage.

## Operations seam

fe-ui never touches the host. Buttons queue an `ExtensionOp`; the binary bridge drains `ExtensionsPanelState::take_ops()` (the Hexon Manager pattern, `terrain_bridge::drain_hexon_ops`) and answers with `ExtensionEvent`s fed to `apply_event`. The consent dialog's `ConsentDecision` is routed by `route_consent` into `Consent`, `KillListOverride` or `DiscardStaged`.

Install from file: `InstallFromFile{path}` -> bridge verifies sealed bytes (`verify_plugin_package`), stages, answers `ConsentNeeded{is_update: false}` -> dialog -> `Consent{digest}` -> `Rows` snapshot. Nothing runs before the consent op, and consent is recorded against the digest the dialog displayed.

## Update flow (P4.3)

1. **Offer.** A row with `update_available` shows `Update to <v>`. Pressing it queues `Update` and the row shows `verifying update`.
2. **Re-verify.** The bridge fetches or reads the newer sealed bytes and verifies seal, identity, trust and revocation exactly as for a fresh install. A failure arrives as `Failed` and nothing changes.
3. **Re-prompt on a delta.** `ConsentNeeded{is_update: true}` carries `consented_digest`. If it equals the new `capability_digest`, `ConsentState::request` returns false and the panel queues `ActivateUpdate` with no prompt (presentation-only change). Otherwise the dialog opens in re-prompt mode with the `+ / -` grant diff and the line `The running version stays on until you accept.`
4. **Keep the previous version until the new one is active.** The bridge starts the new build beside the old one: `Activating{version}` (row: `starting <v>, previous version still runs`), then `Active{version}` and a fresh `Rows` snapshot. If the new build fails to start the bridge reports `RolledBack{reason}`: the old version never stopped, the row shows `update failed, previous version kept`, and the error line carries the reason. Declining the dialog sends `DiscardStaged`; the running version is untouched.

## Registry sources

`RegistrySource` has three operations: `list(query)`, `header(entry)` (the `plugin/extension.json` document, so consent can render before download) and `fetch(entry)` (sealed bytes). `fetch` always re-verifies locally with `verify_plugin_package` and then requires the verified `version` and `content_hash` to equal the listing's. A source can therefore lie about a listing but cannot get unverified bytes installed.

| Source | Reads | Trust |
|---|---|---|
| `LocalSource` | one `.hexon` or a folder of them | packages verified at list time; unverifiable files are shown under their file name with the reason, not dropped |
| `StaticIndexSource` | `index.json` through a `BytesFetcher` (file here; the binary supplies http/https incl. loopback) | index signed by an index-signing trust root; offline cache re-verified on every use |
| `ProtocolV1Source` | a protocol-v1 registry through `ProtocolV1Transport` (adapts `fe-plugin` `RegistryClient` / `RemoteRegistryClient`) | `X-Hexon-Registry-Protocol: 1` checked before every operation |

Index signature rule: the revocation list's. Signed bytes are the canonical JSON (sorted keys, no whitespace) of `{entries, issued_at, schema_version}`; the signature is hex ed25519 by the key the trust roots map from `signer_key_id`. Because `entries` are signed, a tampered size, url or hash fails as `IndexSignature` before any entry is shown. `fetch` on a static index only serves entries the verified index lists, never a caller-supplied url, so a crafted `RegistryEntry` cannot redirect the download.

Distinct refusals (each shown verbatim in Browse): `IndexSignature`, `IndexSignerUnknown`, `IndexSchema`, `IndexMalformed`, `ProtocolMismatch`, `PackageInvalid` (includes bad seal / digest mismatch), `PackageUntrusted`, `PackageContract`, `VersionMismatch`, `ContentHashMismatch`, `TooLarge`, `NotFound`, `Transport`, `HeaderUnavailable`.

Licensed packages go through the entitlement path unchanged (R29); a source only moves sealed bytes.
