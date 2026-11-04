---
type: Review
date: 2026-11-04
title: Native navigation security v3 review
---

# NAV-1B v3: independent security and authority source re-review

Reviewer: `/root/completion_scope_review`, 2026-11-04.

**APPROVED for source integration of complete NAV-1B v3.** The NAV-F1 bootstrap correction does not introduce a grant or bypass the typed navigation authority floors. The previously approved S1 provider-retirement correction remains intact. No new blocker found. This is source approval only; compilation, authored journeys, actual DB-role acceptance, Windows-provider delivery and runtime expiry/recovery checks remain unexecuted and unaccepted.

## Exact reviewed source and preservation

Isolated checkout `.omc/worktrees/qa-native-navigation-helper`, documented foundation `541aef483c2bffb01975744e882ed310d1bfc612`. Independently verified the supplied artifacts under that checkout's `.omc/research/`:

| Artifact | Raw bytes | SHA256 |
| --- | ---: | --- |
| qa-native-navigation-nav1b-v3-source.diff |122314 |`5ac3688beb10967c4b961cee7f524c9878d176bee42d8f40c714a8b37be44a33` |
| qa-native-navigation-nav1b-v3-nav-f1.delta.diff |12612 |`9ff6a2f416a795732d6a2a3038a6784c9246e5809b089bfbe8995e30b9a35f68` |
| qa-native-navigation-nav1b-v3-hashes.json |7298 |`e14503f105ed4ed6c0ce63f001a0426aeac5bfc923019aa695ddb2831a6adba2` |
| qa-native-navigation-nav1b-v3-source-report.md |8439 |`de5fa8410e3291dfe8f71d33d53778e76382f7c5214a47aabe2a2a466d5d3d6d` |

All25 current raw file sizes/hashes match the manifest and the separate v3 snapshot. All22 preserved v2 source hashes match the approved v2 manifest. The25-file pre-correction baseline, including the three added production-owner baselines, also matches its saved before manifest. All ten preserved research artifacts remain byte-identical to their canonical originals. Exactly the five reported paths change against v2 plus those baselines: replica_reads.rs, db_results/mod.rs, the navigation UI journey, and two directory documentation files. The other20 paths are byte-identical.

Read the original security review, its approved v2 correction, the functional NAV-F1 finding, actual v3 delta and surrounding production role/result/source/proof/provider paths. Applied the shared UI/UX guidance. Only this main review artifact was authored. No production source edit, Git operation, compiler/build/formatter/test, app/process/OS/native input/capture, API/DB/store or network action was performed. Main and other active owners were not operated or edited.

## Accepted role initializes source identity, without injecting privilege

Actual `VerseManager::accept_scene_role` still returns before any mutation unless the supplied scope exactly equals `NavigationManager::active_scope`. The new get_or_insert_with initializes access_generation only when it is absent. It does not replace an existing epoch, including an epoch previously minted by invalidate_scene_access. synchronize does not rewrite access_generation. The original node_index invalidation-preservation assertion is unchanged; independently verified that file's SHA256 `efce459c980bad4b10016234a00c6950698bf1bade32d21a75b0e2fab6f0724e`.

Initialization creates a source identity, not a RoleLevel, durable grant or target proof. The following existing logic still parses the actual supplied role and sets blocked according to can_view. Unknown and `none` parse to RoleLevel::None, remain blocked, clear pending replica/source read models and cannot produce positive navigation eligibility. Even though a matching denied result may initialize a source epoch, the epoch alone grants nothing. Repeating a matching accepted result retains the same epoch rather than refreshing prior proof eligibility.

The new crate-private `apply_local_role_result` seam accepts only DbResult::LocalRoleResolved. It applies the existing manager acceptance, existing exact-scope role-cache handler and existing Pen cancellation effects together. The real apply_db_results pre-routing branch calls this seam; the later result arm no longer applies the cache handler a second time. For stale scope both manager acceptance and role-cache handling refuse, and the exact-scope Pen condition is false. An unrelated DbResult makes the seam return immediately. No exposed API, new role lookup, durable mutation, caller-selected DID or external privilege surface is added.

The source remains the normal local-role result path: the actual DB worker handles ResolveLocalRole using its own local_did, and emits LocalRoleResolved only after the resolver succeeds. This patch neither replaces that resolver nor makes its UI receipt an independent authority credential. Existing result ordering and scope matching remain; v3 does not claim to add a new reply-generation protocol for same-scope requests.

## Product guard and independent typed target proof still gate every positive path

Actual product_guard continues to copy access directly from manager.scene_access_generation, role from the existing LocalUserRole cache, and capture_view from actual can_view plus !scene_access_blocked. Its modal/close/gesture/path/popup readiness checks remain. It refreshes the actual source and reads the resulting navigation proof epoch; the UI fixture no longer overwrites that Guard or fabricates a replacement source key.

The unchanged Source::capture still requires a nonzero context lifetime, ready/capture_view/clear labels, Some(access), actual role.can_view, unblocked scene access, valid unique stored current ancestry, no active Petal and no selected objects. A current Verse with no active Fractal may target a bounded Petal under that Verse, but cannot use the absence of a Fractal selection to escape the current Verse. A no-active-Verse context has no valid current ancestry and is ineligible. Source::allows repeats the required bounded target and Verse/optional-Fractal restrictions.

Actual typed ReadNativePetalView still goes through the private DB worker with its actual local_did, canonical request-ID validation and bounded one-second outer await. Its handler independently prepares stored Petal ancestry at Viewer level, compares actual Verse/Fractal/Petal IDs and exact name, reads minimal bounded rows under the existing access guard, and returns the typed target with an opaque source fence. Initializing a UI epoch therefore does not bypass the DB-local Viewer check or turn a cached Viewer result into an accepted target view. No REST/MCP caller or Owner HTTP floor changes are part of this delta.

ProofOwner remains at most16 target demands and one accepted read. Its immutable one-second deadline starts at original demand admission before queue submission. Backpressure, result arrival, repeated role acceptance, completed output and later use do not retag or extend an old demand. Exact source/request/target/set membership and current time remain necessary. The target-view reply in the journey is explicitly a typed successful-result fixture, not evidence that the durable DB Viewer read was executed.

## Approved provider retirement and use-time refusal remain byte-exact

The actual S1 production files are unchanged from approved v2, including navigation.rs, navigation_ui.rs, surface.rs, provider_identity.rs, snapshot.rs, host native delivery/state and the actual provider-output handoff. Relevant production behavior remains visible in those files:

- Failure retirement advances the checked process-static observed generation before trying the session lock, then clears owner eligibility/mapping/callback when possible. observed_available/current-generation checks independently refuse old bindings on lock refusal, poison or exhausted/unproved state.
- ProofOwner::retire preserves the exact accepted in_flight demand; repeated retirement does not manufacture another DB slot. Its exact stale reply must drain as discarded before fresh submission.
- The real mutable provider output slot goes through the same full-tree safety/handoff seam. Missing/unsafe output remains suppressed, process-static HOLD and alternate-publisher suppression remain, and safe complete unmarked recovery cannot revive retired eligibility.
- The issued-ID ledger retains tombstones. Fresh positive remapping uses fresh binding/set/target identity, while original physical IDs and issued aliases without their exact typed binding cannot fall through to an ordinary A1 click.
- Snapshot, retained Pending matching, translation, begin_pass and deferred final callback continue to retain/check the original proof and use a fresh Instant. The actual final callback checks current rendered membership, hierarchy and names before existing navigation effects. The raw native-input presence still prevents invalid typed input from becoming a physical row click.

The v3 change initializes a formerly absent legitimate source epoch; it does not edit any of these provider, timing, identity or typed-delivery floors. The v2 stale-alias/recovery and accepted-read retirement assertions remain, apart from obtaining initial role/access through the real shared role-result path.

## Authored journey parity and execution limits

The existing native UI journey now starts from VerseManager::from_verses with default ReplicaReads and an absent LocalUserRole. Its Guard comes only from product_guard; independently checked there is no Guard.access assignment or LocalUserRole.role assignment in this fixture. Both default construction and matching role application use the actual production seam, not a new fixture-only epoch initializer.

The added cases explicitly exercise current-Verse/no-Fractal/no-Petal Viewer, current-Fractal/no-Petal Viewer, and current-Verse/no-Fractal/no-Petal Editor. Each first verifies unresolved state cannot queue a typed target read, rejects a stale-scope role without epoch/cache mutation, accepts the matching result, checks actual Guard access equals the manager epoch, and preserves that epoch on repeated acceptance. The normal rendered target/read reply/finish/remap path then reaches navigation_petal_v1 without silently selecting a Fractal or Petal. Separate matching None/unknown cases verify blocked capture, no queued read, no profile and no remap.

Registered test attributes remain68→68 across the expanded25-file scope; the original22-file scope remains21→21. These are meaningful authored source assertions, not executed passes. In particular the explicit success reply and complete-producer fixture do not establish actual durable Viewer/Editor permission or real Windows handler-to-Adapter.next ordering.

Root still owns exact integration review, compilation and the integrated existing journeys; actual fresh matching local role plus bounded DB target-view navigation; every render/annotation/Last/PreUpdate/begin/final-callback deadline boundary; provider hold/recovery/stale-ID behavior; lock/reset/ownership refusal; and read-retention and ordinary A1/physical-input regressions. Preserve accepted work and immutable unknown outcomes without replay.

Operator helper staging, original same-worker COM element/InvokePattern, watcher-before-discovery, exact process/HWND/context/handler identity, durable reservation/pre-Invoke intent, all-outcome one-attempt latch and uncertain-worker lifetime ownership remain later work. No all-Verse bootstrap, selected Inspector/node/group surface, WGC/capture, FULL campaign, project completion, CI or tagged-release gate closes on this source approval.
