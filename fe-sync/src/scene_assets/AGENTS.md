# Native scene asset jobs

See ../AGENTS.md section scene-assets for protocol, role, persistence and verification limits.

mod.rs owns bounded queues, per-open cancellation leases, exact native request owners and
completion ordering. admission.rs is the only job-to-DB query adapter; it owns deadlines
and bounded fresh retries while ordinary DB recovery may be busy. transfer.rs owns the
verified source/native/shared-store byte effects and RAII temporary-tag cleanup. The
separate scene_asset_gate module persists non-evictable protection independently from
DB reset and performs fresh Serve admission for authenticated native connections.

Keep the four-slot permit alive through local persistence and final admission. New
transfer modes must settle exact caller IDs and preserve the shutdown write barrier.
Providers combine bounded native bootstrap endpoints with authenticated successful
SyncFinished peers from that exact native subscription. The Owner can therefore open
with no bootstrap peers and learn a later joining Editor. Jobs sample the live roster
(after a bounded startup wait), not a list copied at job creation. NeighborDown,
stream error, subscription replacement and Close remove readiness; old roster objects
never become usable again. A cap failure refuses the full roster rather than choosing
a prefix. ID-only addresses resolve through the native endpoint's recently learned
routes; normal transport endpoint pinning and relay allowlisting still apply. No UI
provider URL bypass or Boolean permission cache belongs in these modules.

JobLease owns both cancellation and an Arc of the four-slot permit. Blocking reads
and classification writes clone this owner until their finite IO ends, so cancelling
an async waiter cannot free capacity while its uncancellable disk task is still
running. Shared-store writes/readback are awaited directly; the sync runtime drains
remaining finite blocking tasks before its OS-thread join can certify terminal reset.

Publish admission always carries the exact envelope asset ID and Petal as well as its
content digest. Sharing verified bytes never authorizes a different metadata identity;
every re-admission repeats that exact identity before metadata emission. Compatibility
FetchBlob also enters these admitted jobs; no has_blob/path.exists fast path may emit
BlobReady. The existing native journey authors corrupt-cache compatibility repair and
two exact metadata identities sharing bytes. These are source fixtures, not executed
acceptance or an actual two-application rendering result.
