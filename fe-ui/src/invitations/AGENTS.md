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

Cancel and replacement immediately discard input and pending receivers. Cleanup conditionally
closes only the exact native lease requested by this join; cursor equality never proves
lifecycle adoption. The same
Sync channel orders cleanup after an earlier open. Late replies cannot restore a dropped
request. The overall deadline is 30 seconds; false readiness polls are bounded read-only work,
while admission/import are never automatically repeated. At most 64 submitted nonces are kept
in memory for the session, without eviction, to refuse replay even after an unconfirmed outcome.
Stored membership or a cached bearer capability may survive cancellation or failure.

The legacy uncorrelated result handlers cannot complete a current addressed Join. Initial
hierarchy auto-navigation is suspended while that Join owns navigation, including on an empty
recipient store. The regular hierarchy handler still projects actual stored rows; the invitation
coordinator alone decides whether the exact current request can navigate.


## Bounded command queues

Every coordinator submission uses try_send. A full queue means that exact command was never
accepted: issuance and join phases fail visibly without replaying accepted effects. Cleanup is
the exception that retains one definitely-unsent close until queue space is available; it blocks
a new Join until the close has entered the same FIFO behind the old open. Disconnection keeps
cleanup unconfirmed. Cancel and deadline checks remain runnable even when either worker queue
is full. Existing GUI journeys use actual full bounded DB/native channels, without workers.


## Late hierarchy loads after cancellation

A successfully queued admission retains the navigation intent revision independently of the
active dialog/attempt. Cancel, failure and replacement do not erase that guard. Every initial
hierarchy selection consults it, so a late joined row cannot select itself after the dialog closes
on an empty store. NavigationManager records explicit navigation/back intents; a changed revision releases only
the Join-specific guard. The independent startup latch still honors explicit Back, so ordinary
reloads cannot re-enter afterward. Read-only suppression does not advance the intent revision.
Terminal read-model reset clears the Join guard.


## Exact native lease ownership

A joined pipeline has a local lease naming its request generation, Verse and namespace. Sync
records it only after successful native open; ordinary reopen/close invalidates it. Readiness
and CloseJoinedReplica check that exact lease. Conditional cleanup then uses the ordinary close
path (pump, gossip, provider and blob-access invalidation), without any new network protocol.

ReplicaHandoff retains its lease when Verse/namespace does not match. Before a navigation
transition, superseded handoff cleanup must enter the FIFO; Full retains the lease and returns
from the frame. Normal navigation close/open also uses try_send and records only accepted
commands. A matching handoff is consumed only after the previous navigation close is accepted.
New Join admission waits while a previous handoff remains owned, under its input deadline.
The existing lifecycle journey covers ready B superseded by C before adoption, including a
full bounded queue; the existing sync lifetime journey covers stale generation/namespace and
ordinary-reopen refusal of conditional cleanup.


Canceled attempts retain their exact native lease until CloseJoinedReplica is accepted, even
when the current cursor temporarily selects that Verse. Actual ordinary adoption/reopen clears
Sync's prior lease, making a late conditional close harmless to the independent pipeline. The
existing lifecycle journey now also covers canceled B, transient cursor B, then independently
owned C before lifecycle observes B, under a full bounded queue.


## Joining the currently open Verse

A new addressed Join cannot replace an existing navigation pipeline. The UI refuses its current
Verse before admission, nonce consumption or native submission, with explicit leave-then-accept
recovery. Sync independently refuses any already-open target before capability import/open.
This also covers a navigation-open racing the UI preflight: admission metadata may have been
accepted, but the existing native pipeline remains owned and no new join lease is installed.
A following canceled-request conditional close cannot match that independent pipeline. Existing
GUI lifecycle and actual cold-read native journeys cover the refusal and preserved live document.


## One-use QA transfer

IssueState keeps a separate fifteen-second consume lease after confirmed generation, bounded
by envelope expiry. Public metadata names the exact current generation and source selection;
no credential enters UI labels. The host obtains only a source snapshot to perform a fresh
readonly Owner preparation, then consume_transfer compares its full manifest/target/recipient/
role/expiry before moving the credential out and invalidating the generation. A second consume
cannot return it. Existing Access lifecycle reset/selection/Owner guards apply to every read.


## Guarded Join input

The recipient Observe response exposes only opaque actual widget IDs and clipped physical
geometry, preserving blank PasswordInput names/values in the generic accessibility tree.
A short one-use input lease binds the actual dialog generation, input revision, recipient DID,
navigation intent, primary-window entity/size/scale/focus, and real widget/layer/clip geometry.
The existing authenticated input door accepts its opaque constraint in X-FE-Join-Lease; it is
an additional restriction, never an authorization subject or a direct Join operation.

Admission permits only a left click inside the masked input, bounded ASCII TextEntry chunks,
and a left click inside the captured Join control, with no keys, captures or settle frames.
Every replay frame rechecks current domain, window, focus and widget identity. Text frames
use a private capsule rather than global keyboard events: the renderer inserts a temporary
egui Text event immediately before the actual password TextEdit, then removes it immediately
afterward. The existing TextBuffer adapter validates the current-pass response geometry,
layer, clip, focus and exact append at insert_text. It never assigns invite_buf on behalf of
the host. A plain editor rendered before or afterward cannot receive that private event.

The normal Join button response must be actually clicked under the matching lease and current
geometry before its existing handler queues JoinState. Only that observed response permits a
successful input receipt; there is no direct Join/auth/state-completion shortcut. Focus moving
to the submit button is permitted, including its native pointer-down ownership when keyboard
focus is surrendered. Foreign focus/layout/dialog changes abort and drop private pending text.
The existing real egui Join journey covers success and stale layout/focus/dialog interleavings
with a deliberately focused plain sink, without claiming transport or role readiness.


Prepared/issued source correlation and one-use fresh-source consumption now include typed
InviteAuthority identity as well as the display manifest. A changed authority/proof cannot
release the old credential under unchanged labels. Manager UI production activation remains
separate from these correlation guards until the v2 DB and coordinator seams are reviewed.


## V2 proof-finalization coordination

The real Join controller accepts strict v1/v2 envelopes. A v2 Manager recipient reaches proof
finalization only after ordinary role readiness, loaded target hierarchy and the actual
post-role native readiness probe. ProofFinalization is a one-submission correlation gate, not
authority: bounded queue refusal is definite, accepted commands are never replayed, and only
the matching pending receipt can confirm it. The receipt returns to RoleReady and requires a
new actual native inspection before navigation. A later native failure re-enters read-only
convergence checks without replaying proof activation, admission or a grant.

Attempt revision/cursor/DID/nonce/deadline guards and retained navigation suppression remain
authoritative through this phase. Cancel drops the correlation gate; late finalization
receipts cannot navigate. Metadata, an activated local Manager proof or imported capability
can remain stored after cancellation or an unknown result, which the dialog explains. The
existing masked Join journey additionally exercises full bounded finalization admission,
accepted-command no-replay and stale/duplicate receipt correlation without claiming native QA.
Owner Access now receives v2 production output; Manager issuer controls are the following
activation slice. Existing Owner-v1 joins remain compatible and do not invent Manager proofs.
