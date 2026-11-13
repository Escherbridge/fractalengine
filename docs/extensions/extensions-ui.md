# Extensions UI

Design text for the Extensions panel (spec R28), Browse and the registry sources behind it (R29, R32), and the update flow. It applies `C:/Users/atooz/.gemini/UI-UX-guidance.md`: one purpose per view, one obvious primary action, layout before copy, stable accessible names, and real empty, loading, error and disabled states.

Code: `fe-ui/src/extensions/{panel,details,permissions,browse}.rs` (UI), `fe-hexon/src/registry_source.rs` (sources). Consent itself is `consent.rs` (E11); this panel only opens it and routes its answer.

## Views

| View | Purpose | Primary action | Secondary |
|---|---|---|---|
| Installed | See what is installed and its state | Install from file | Open a row |
| Details > Overview | Understand one extension and run it | Enable / Disable a binding (Update when one is offered) | Uninstall (two-step) |
| Details > Permissions | See what it was allowed to do | none (read-only record) | Revoke consent (two-step) |
| Browse | Find a package in a source | Install on a row | Source picker, search, sort, Refresh |
| Store details (a Browse row) | Review one catalogue package: publisher, review state, licence | Install, or Get licence when it is licensed | Check licence, Back |
| Bindings | See which extension serves each hook point at the active petal, and where each binding comes from | Edit (Manager+) on a row; Install when a binding offers one | Refresh bindings, Clear |

Administration is GUI-only. The same operations are reachable from a terminal through `fe-ext admin` over the local admin channel ([admin-channel.md](admin-channel.md)); it never uses REST or MCP, and every grant is still confirmed in the consent dialog this panel owns. A request that arrives over the channel raises the dialog and waits for the operator.

The consent dialog is not a view: it opens over the panel whenever the bridge reports `ConsentNeeded`.

## States

Every view renders each state from data, never from copy that explains the layout.

- **Empty.** Installed: `No extensions installed` above the install field, which stays usable. Browse without sources: `No extension sources are configured`. Bindings, counters and audit each say what is absent in one short line.
- **Loading.** The first draw queues `Refresh`; until the rows arrive the list reads `Loading extensions`. A spinner row names the bridge's current task (`Verifying package...`, `Listing...`). A selected Browse source lists itself once; Refresh and Enter re-list it.
- **Busy.** While a task is in flight or a consent dialog is open, every state-changing button (Install, Enable/Disable, Update, Uninstall, Revoke consent and their confirm steps) is disabled; read-only navigation stays live.
- **Error.** One red line with the bridge's message verbatim (`package-invalid (bad-signature)`), and `Dismiss`. Refusals are never paraphrased: they are the contract's reason strings.
- **Disabled.** A button that cannot act is disabled, not hidden. Blocked rows (revoked build, policy, role) show `Blocked: <reason>` and their binding buttons are disabled; Uninstall stays available when the panel is not busy.
- **Needs consent.** Enable stays available: the bridge answers with `ConsentNeeded` (R19 `consent-missing`) and nothing runs until consent is given. This is how an extension is allowed again after Revoke consent.
- **Cached or stale Browse.** `Showing the last verified list` when the index came from the cache (unreachable endpoint, or a live index older than the cached one); `This list has expired and may be out of date` past the index's `expires_at`. Neither blocks an install.

## Accessible names

Names are stable and carry the package id so an agent or screen reader can address one row among many.

`Installed extensions`, `Browse extensions`, `Package file path`, `Install from file`, `Open <package_id>`, `Back to installed extensions`, `Overview of <id>`, `Permissions of <id>`, `Enable|Disable <binding label> for <id>`, `Update <id> to <version>`, `Uninstall <id>` then `Confirm uninstall <id>` / `Keep <id>`, `Revoke consent for <id>` then `Confirm revoke consent for <id>` / `Keep consent for <id>`, `Extension source`, `Search extensions`, `Refresh extension list`, `Install <id> <version>`, `Dismiss extension error`, `Sort extensions`, `Load more extensions`, `Details of <id>`, `Back to extension results`, `Reviewed <id>`, `Get licence for <id>`, `Check licence for <id>`, `Disable all extensions` then `Confirm disable all extensions` / `Keep extensions running`, `Enable all extensions`, `Toggle Extensions panel`, `Extension bindings`, `Refresh bindings`. Binding names are in the next section. J9 (`panel.rs::extensions_panel_journey`) asserts them through the accessibility tree.

## Bindings view and its names

The view lists the hook points at the active petal, grouped by family, with the winning scope and this peer's state ([bindings.md](bindings.md) §5, §9). Names come from `fe-ui/src/extensions/bindings.rs` and `bound_slots.rs`:

- `binding:<hook>` or `binding:<hook>:<key>` names a state row. The same name plus `:action` names its one action (Install, Allow, Edit).
- Edit-form controls take suffixes on the row name: `:edit`, `:scope:<petal|fractal|verse>`, `:mode:<extension|builtin>`, `:provider`, `:version`, `:field:<name>`, `:save`, `:cancel`, `:clear`.
- Surfaces a binding fills are named `binding:ui.panel:<key>` (extension panel) and `binding:egress.snippet:<key>` (egress card).
- `Remove binding at <scope>` is the two-step Clear. An unresolved binding keeps the built-in behaviour and shows its reason; the install offer opens the normal consent dialog (`Offered by the binding at <scope>`) and never installs by itself.

## Store flows

A Browse source that speaks the store contract adds three steps, all read-only until the operator acts:

1. **Details.** `Details of <id>` asks the bridge to verify the package's detail document and header (counter-signature, review state). The result shows `Reviewed <id>` or `Unavailable: <reason>` verbatim.
2. **Licence.** A licensed package shows `Licence not checked`. `Get licence for <id>` opens the package's licence page in the system browser (the bridge appends the licensee and device ids). `Check licence for <id>` asks the issuer for a token and the bridge verifies and keeps it; the row then reads `Licensed until <date>`, `Licensed` or `Licence refused: <reason>`.
3. **Install.** `Install <id> <version>` follows the normal consent path. A paid package installs only with a verified token; a source only moves sealed bytes.

Credentials never reach a source: catalogue reads are anonymous, and only the token request authenticates, by its own proof.

## Copy rules

- Status words: Enabled, Disabled, Blocked: reason, Needs consent.
- Trust words: `trusted publisher`, `publisher not trusted`, `unsigned`. Never "safe".
- Destructive actions state the consequence once, at the confirm step: `Remove <name> and stop all its bindings?`, `Stops every binding until you allow it again.`
- No taglines. A row is `<name> <version>`, status, then `<publisher> · <trust>` and, when relevant, `update <v> available` or the update stage.

## Operations seam

fe-ui never touches the host. Buttons queue an `ExtensionOp`; the binary bridge drains `ExtensionsPanelState::take_ops()` (the Hexon Manager pattern, `terrain_bridge::drain_hexon_ops`) and answers with `ExtensionEvent`s fed to `apply_event`. The consent dialog's `ConsentDecision` is routed by `route_consent` into `Consent`, `KillListOverride` or `DiscardStaged`.

Install from file: `InstallFromFile{path}` -> bridge verifies sealed bytes (`verify_plugin_package`), stages, answers `ConsentNeeded{is_update: false}` -> dialog -> `Consent{digest}` -> `Rows` snapshot. Nothing runs before the consent op, and consent is recorded against the digest the dialog displayed.

Disable all: below the installed list, `Disable all extensions` (two-step, shown while any row is Enabled) queues `SetHostSwitch{disabled: true}`; otherwise `Enable all extensions` queues `SetHostSwitch{disabled: false}`, which the bridge refuses with `host-off` while `FE_EXT_DISABLE_ALL=1` was set at startup. Both are audited with `channel: gui`, exactly like `fe-ext admin disable-all|enable-all` (`channel: cli`), and reach the portal resolver.

Bridge contract (the drain is `fractalengine/src/extension_host/ops_drain.rs` `OpsWorker::dispatch`; these are its obligations):

- `Refresh` answers `Rows` and `Browse(Sources)`.
- `InstallFromRegistry` for a package id that is already installed stages an update of that installation (`ConsentNeeded{is_update: true}`, previous version kept), never a second installation.
- `Enable` on an installation with no consent for its digest answers `ConsentNeeded`, not an activation.
- `KillListOverride` lifts only the kill-list refusal (audited, verse Owner, own host); consent is still owed, so the bridge answers with a fresh `ConsentNeeded` for the same installation.
- Every op answers with `Rows`, `UpdateStage`, `Browse` or `Failed`, so the panel's working state always clears.

## Consent cannot be bypassed

The UI only presents; the host decides. These hold whatever the UI sends:

1. **Digest pinning.** `Consent{installation_id, capability_digest}` is accepted only when the digest equals the staged build's capability digest (the REST "stale digest refused" rule, R20); anything else is `Failed`.
2. **Activation re-check.** `ActivateUpdate` and `Enable` run R19's preconditions at use: precondition 9 (`consent-missing`) compares the build's digest with the consent on record, so a UI that skips the dialog (a same-digest update) cannot activate a build whose grants differ.
3. **Fresh installs always prompt.** The panel strips any `consented_digest` from a fresh install's request, so only a running version's consent may skip the dialog.
4. **One review at a time.** While a dialog is open the panel is busy; a second `ConsentNeeded` for another installation is answered with `DiscardStaged` and never replaces the open review.
5. **Enable waits for the drawn list** (`consent.rs`, S3 AC-5), and a kill-listed build can never be enabled from the dialog, only overridden by the typed package id.

## Update flow (P4.3)

1. **Offer.** A row with `update_available` shows `Update to <v>`. Pressing it queues `Update` and the row shows `verifying update`.
2. **Re-verify.** The bridge fetches or reads the newer sealed bytes and verifies seal, identity, trust and revocation exactly as for a fresh install. A failure arrives as `Failed`; the panel clears the `verifying` stage so Update can be retried, and nothing else changes.
3. **Re-prompt on a delta.** `ConsentNeeded{is_update: true}` carries `consented_digest`. If it equals the new `capability_digest`, `ConsentState::request` returns false and the panel queues `ActivateUpdate` with no prompt (presentation-only change; the bridge re-checks it, rule 2 above). Otherwise the dialog opens in re-prompt mode with the `+ / -` grant diff and the line `The running version stays on until you accept.`
4. **Keep the previous version until the new one is active.** The bridge starts the new build beside the old one: `Activating{version}` (row: `starting <v>, previous version still runs`), then `Active{version}` and a fresh `Rows` snapshot. If the new build fails to start the bridge reports `RolledBack{reason}`: the old version never stopped, the row shows `update failed, previous version kept`, the error line carries the reason, and Update can be retried. Declining the dialog sends `DiscardStaged`; the running version is untouched.

## Registry sources

`RegistrySource` has three operations: `list(query)`, `header(entry)` (the `plugin/extension.json` document, so consent can render before download) and `fetch(entry)` (sealed bytes). `fetch` always re-verifies locally with `verify_plugin_package` (expected id, seal, trust roots) and then requires the verified `version` and `content_hash` to equal what the source vouched for. A source can therefore lie about a listing but cannot get unverified bytes installed; revocation is checked again at install and activation.

| Source | Reads | Trust |
|---|---|---|
| `LocalSource` | one `.hexon` or a folder of them | packages verified at list time; unverifiable files are shown under their file name with the reason, not dropped |
| `StaticIndexSource` | a static endpoint (folder or http(s) prefix) through a `BytesFetcher`: `config.json`, `index.json`, `packages/{id}.json`, `packages/{id}/{version}/attestation.json` and the download | index signed by a key pinned for this endpoint; detail documents pinned by hash; offline cache re-verified on every use |
| `StaticIndexSource` over a hosted store | the same documents plus `search` and entitlement operations when `config.json` `features` advertise them | a missing feature means search runs locally and no licence is offered; the client never probes |
| `ProtocolV1Source` | a protocol-v1 registry through `ProtocolV1Transport` (`fe-hexon/src/remote.rs`, E16) | `X-Hexon-Registry-Protocol` (or `/health`) checked before every operation |

### Static signed index (registry contract v1)

The public M0 shape of the registry seam: every GET is a plain file, so a static folder is a full endpoint.

- **Protocol.** `config.json` must carry `registry_protocol: 1`; no header is required. Any other value is `ProtocolMismatch` before the index is read. `config.json` is unsigned, so its `index_key_id` never selects or widens trust.
- **Signature.** Ed25519 over the domain line `fractalengine/catalog-index/v1`, a LF, then the canonical JSON (sorted keys, no whitespace) of the received document with only `signature` removed. It is verified over the generic JSON as received, so fields this build does not read are still covered and a signer in another language interoperates. The signature is hex (the revocation list's encoding); standard base64 is also accepted. Signed documents carry integers only: a float anywhere is `IndexMalformed`.
- **Index key.** Looked up by `signer_key_id` in the key set pinned for this endpoint (`SourceTrust::index_signer`, built per source by the binary). An index key is never a package or revocation signer: a key that is also a package trust root is refused as `IndexKeyMisused`; keeping it off the revocation roots is the bridge's configuration rule.
- **Schema and freshness.** `schema_version` other than 1 is `IndexSchema` (a newer one is refused). A live index whose `issued_at` is older than the cached copy is ignored and the cache is used (rollback guard). Past `expires_at` the list is shown as stale and still installs.
- **Entries.** Only kind `extension` is listed; other kinds are skipped. One package-id namespace across kinds: an id that fails `^[A-Za-z0-9][A-Za-z0-9_.-]{0,127}$`, ends in `.json`, or collides case-insensitively with an earlier entry is shown as a refusal line; so is a `latest_version` that is not SemVer without build metadata, or a hash that is not BLAKE3 hex.
- **Detail and download.** `fetch` and `header` resolve the version through `packages/{id}.json`, whose BLAKE3 must equal the index's `detail_hash` and whose `package_id` and `kind` must match (`DetailMismatch`); a yanked version is refused (`Yanked`). The download url comes from that document, never from the caller; a relative url may not climb out of the endpoint, `size_bytes` caps the read, and fetchers are anonymous (no credential to the endpoint or to any url a document names).
- **Header before download.** `header` reads the attestation and returns `plugin/extension.json` only after `verify_sealed_header` (manifest signature, every committed header digest), a check that the member is committed in `integrity.header`, and `content_hash(manifest)` equal to the vouched hash. A missing attestation is `HeaderUnavailable`: consent then renders after download, from the verified package.
- Leaving the index is not revocation, and kill-listed packages are refused at install by the host, not by the source.

Distinct refusals (each shown verbatim in Browse): `IndexSignature`, `IndexSignerUnknown`, `IndexKeyMisused`, `IndexSchema`, `IndexMalformed`, `ProtocolMismatch`, `DetailMismatch`, `Yanked`, `PackageInvalid` (includes bad seal / digest mismatch), `PackageUntrusted`, `PackageContract`, `VersionMismatch`, `ContentHashMismatch`, `TooLarge`, `NotFound`, `Transport`, `HeaderUnavailable`.

Licensed packages go through the entitlement path unchanged (R29); a source only moves sealed bytes.
