# Desktop invitation coordinators

This directory owns GUI-only state machines over existing local DB and Sync messages. It has
no async runtime and never blocks a frame waiting for another thread. The database owns
current authority, identity admission and stored role fences; Sync owns native capabilities,
subscription and connection evidence. No GUI success flag replaces either boundary.

## Owner issuance

The Access tab queues source preparation, native ticket export, then rechecked grant-and-sign
as separate phases. Each dialog state has one generation and one source selection. Exact target,
recipient, role, expiry, captured manifest and preparation nonce must survive each reply. Leaving
Access, refreshing, changing selection, dialog replacement, expiry or Owner loss drops retained
credentials and pending receivers. A bounded timeout leaves an explicit unconfirmed outcome;
there is no automatic issuance replay. Manager recipients are supported, while Manager issuers
remain unavailable until an Owner-rooted delegation proof can be checked.

Native ticket export requires an actually live replica and the actual local Owner identity.
Ordinary role mutations are disabled while issuance is pending. The final credential is retained
only in redacted types and is never placed in widget labels, tracing fields or diagnostic errors.
The Copy action uses the ordinary UI output path, where the host's QA clipboard suppression
continues to apply. Read tickets still carry whole-Verse bytes: UI scope is RBAC, not transport
confidentiality. Envelope expiry bounds acceptance, not the lifetime of native bearer capabilities.


## Addressed joining and navigation handoff

The masked dialog queues a sealed input with a monotonically changing input revision. The
coordinator binds a fresh generation to the exact text, actual recipient DID, signed nonce and
current navigation cursor. It validates before DB admission, then opens through the native
Sync pipeline. Read-only polls require actual issuer SyncFinished evidence, the normal stored
Owner grant at the signed HLC, exact-scope effective role and loaded ancestry. A final native
inspection follows the role read. These are explicit per-store observations, not a claim of a
cross-store atomic transaction. Admission alone only reports accepted metadata.

Final navigation uses the complete invited ancestry. A private one-use ReplicaHandoff tells the
normal navigation lifecycle to retain that exact namespace's already-live pipeline, rather than
reopening it and discarding its connection proof. Its source is only this ready coordinator;
the lifecycle checks the current hierarchy namespace before consuming it.

Cancel and replacement immediately discard input and pending receivers. Cleanup closes only a
replica requested by this join when the navigation cursor does not own that Verse. The same
Sync channel orders cleanup after an earlier open. Late replies cannot restore a dropped
request. The overall deadline is 30 seconds; false readiness polls are bounded read-only work,
while admission/import are never automatically repeated. At most 64 submitted nonces are kept
in memory for the session, without eviction, to refuse replay even after an unconfirmed outcome.
Stored membership or a cached bearer capability may survive cancellation or failure.

The legacy uncorrelated result handlers cannot complete a current addressed Join. Initial
hierarchy auto-navigation is suspended while that Join owns navigation, including on an empty
recipient store. The regular hierarchy handler still projects actual stored rows; the invitation
coordinator alone decides whether the exact current request can navigate.
