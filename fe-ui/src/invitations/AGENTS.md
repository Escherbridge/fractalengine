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
