# Native scene asset workflows

This module owns bounded transfer intent and explicit local-copy adoption. It
does not own object history, peer publication provenance or the live scene tree.
Current asset descriptions come only from VerseManager's admitted complete DTO.
Need is a hint bound to that source; backend transfer admission remains authority.

Adoption review and confirmation bind selection, navigation intent, manager
incarnation/source epoch and access generation. Confirm re-reads the entire
candidate under the backend's actual Owner/reference fences, compares it with
the exact displayed candidate, then submits one write. A submitted adoption
retains its full candidate and request identity across navigation/access loss.
Timeouts report unknown without replay or ownership eviction. Terminal freeze
retires unsent/read/render work; accepted adoption owners clear only after the
database worker has joined.

Sync results arrive through SceneAssetEventBuffer after drain_sync_events, never
through a competing receiver. Only exact owned request IDs settle a transfer.
Success, failure and overflow request fresh bounded current reads. BlobReady is
only an availability hint; an old DTO is never replayed as current or Saved.

The UI view stored on UiManager is derived display state. The resource here
owns the workflows and accepted writes, so clearing UI read models cannot lose
accepted mutation outcomes.

The module keeps at most 32 accepted adoption owners. Read/unsent submission
deadlines are 5 seconds; a displayed review expires after 60 seconds. Accepted
writes never expire or retry: an absent exact outcome reports unknown and
blocks another adoption of that node. A changed fresh reference/model witness
requires another explicit confirmation, never a silent candidate replacement.

Transfer demand and owned jobs are each capped at 256. One sent job per node
keeps its original source/hash/request identity while a newer availability
admission can supersede its demand. BlobReady or LocalBlob does not settle
that earlier job. Exact settlement, a 30-second unconfirmed deadline, or event
overflow leads to fresh current reads; backoff is bounded at 32 seconds. These
are cache/read requests, not replayable mutations. Navigation/access/manager
retirement discards their old lifetime. The refresh backlog is bounded by the
scene's node hard limit and drains at most 64 entries per frame into the
existing bounded private current-read owner.

MissingMetadata never invents a digest or Need. A later admitted metadata
update can issue Need, or directly project LocalBlob when bytes arrived first.
The Inspector distinguishes verified file availability from publication and
GPU/render success. Oversize missing files show the transfer limit explicitly.

`journey.rs` extends the existing DB-result/residency App journey; it declares
no separate test. It covers metadata/byte/finality order, foreign and old IDs,
overflow/access loss, whole-candidate recheck, expired replies and Full channels,
accepted unknown/late outcomes and the terminal ownership boundary. Actual
failed-loader retry and paired-peer rendering still require the parent's live
QA stage; authored assertions are not execution evidence.
