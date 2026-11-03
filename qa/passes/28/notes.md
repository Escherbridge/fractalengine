# Focused QA28: Viewer hierarchy joins; ordinary scene content is missing

The sealed `c060f50b` stage built without warnings in 413.3 seconds. The retained
Owner and recipient stores were restarted without reseeding, on distinct ports
8882 and 8883, with exact process identities recorded in `pair-context.json`.
The recipient is the formerly fresh QA27 store; its Genesis Verse and actual DID
remain unchanged before this attempt.

The Owner enters the exact Verse scope through the current-Verse breadcrumb,
opens Options and Access, and supplies the observed recipient DID. The recipient
first opens a clean, focused Join dialog. A fresh Owner-v2 Viewer invitation is
then generated and transferred within its short one-use lease. The reviewed
owned-app client reports `consumed_once`, three applied commands and
`input_submitted: true`. It explicitly leaves readiness pending real transport
and role confirmation. No consumed credential or input batch is replayed, and
no clipboard or raw invitation is used in the evidence.

The immediate post-input snapshot still shows Genesis, Join and pending UI
work. This is an asynchronous intermediate observation, not Joined evidence.
The subsequent generic QA wait refuses because no Owner token covers the
recipient's active view. That refusal preserves the Owner-only debug floor;
it is not evidence that native Join failed. The recipient runtime independently
reports the shared Verse's actual local Viewer role. Normal scoped REST reads,
using the legitimately published Viewer token privately through the pinned
owned-run transport, return the same shared Verse, Fractal and six Petal IDs as
the Owner.

Content sharing nevertheless fails. The Owner hierarchy contains 25 nodes; all
six recipient Petals have empty node arrays. A normal read of the Owner's Cube 1
through the recipient Viewer scope returns the API's not-found-or-not-permitted
result. A delayed hierarchy read about twenty minutes after native role
resolution still shows 25 Owner nodes versus zero recipient nodes. Both
hierarchy reads return HTTP 200 and `ok: true`. The missing-content finding is
distinct from invitation submission, Viewer admission and hierarchy arrival.

Read-only source investigation identifies absent ordinary outbound publication
and recovery: ordinary CreateNode, CreateObject and CommitNodeMutation commit
locally without emitting node row replication. Replica-open recovery requests
the local catalog, whose enumeration contains Verse, Fractal and Petal rows
only. The existing inbound path supports authorized, tombstone-aware node rows;
plugin-host publication already has a committed full-node example. A shared
authorized committed-node publisher and bounded recovery repair are being
implemented separately. Current-state row recovery cannot establish historical
journal convergence, asset dependency completion or atomic peer batch behavior.

Both exact app lifetimes and all captured descendants are stopped: eight
identities per app are verified absent. The wrapper and stop sessions completed.
There was no helper process. The verdict inventories 37 evidence inputs and 50
parsed records, with raw-log hashes and secret scans. No Rust panic markers or
structured ERROR lines were found; Owner/recipient WARN counts are 12 and 11.
Independent evidence review approves these focused claims and the missing-content failure; see `independent-review.md`. This is a focused partial result;
full catalog coverage, integrated tests, UX acceptance, CI and releases remain.
