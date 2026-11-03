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
Provider discovery remains limited to successfully opened native Verse bootstrap peers;
no UI/provider URL bypass or Boolean permission cache belongs in these modules.

JobLease owns both cancellation and an Arc of the four-slot permit. Blocking reads
and classification writes clone this owner until their finite IO ends, so cancelling
an async waiter cannot free capacity while its uncancellable disk task is still
running. Shared-store writes/readback are awaited directly; the sync runtime drains
remaining finite blocking tasks before its OS-thread join can certify terminal reset.
