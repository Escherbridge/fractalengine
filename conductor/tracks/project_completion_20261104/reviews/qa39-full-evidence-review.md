---
type: Review
status: approved_focused
---

# QA39: independent focused terrain-grant evidence review

Reviewer: `/root/brush_ingress_trace`, 2026-11-04.

**APPROVED as focused evidence for the recorded normal scoped terrain update.**
No blocking packet-integrity or claimed normal-update discrepancy was found.
This is not acceptance of the pending negative authorization, private-reader,
queue/lifetime, concurrency, rollback, FULL or release requirements.

Only this review artifact was written. No production edits, Git operation,
compiler/formatter/build/test, application/process/OS/input/capture, API/DB/store
or network operation was performed. File reads, hashing and actual inspection
of the saved PNG are evidence review, not a new live execution.

## Frozen packet and independent integrity checks

Manifest `qa/passes/39/packet-manifest.json`, SHA256
`ec654fd786690db3bc28b41f23681db4356ee2fe9021a2de3d24e0fc315e1ec7`.
Independently hashed all26 payloads and checked their byte counts. The recursive
actual file set equals the manifest's26 entries plus the manifest itself:
27files, with no extra or missing payload. All17 original-payload anchors in
packet-summary also match their current raw bytes.

Read the actual finite scan implementation/patterns and independently recomputed
the published scan for all24 covered text payloads:22 UTF-8 and two UTF-8 BOM
files, with exact encoding/count agreement and zero matches for all four patterns.
Decoding was strict; no NUL or replacement-character text payload was found.
The PNG and the scan report itself are outside that published scan denominator;
the manifest is separately parsed/hashed. Pattern coverage is not a universal
proof that arbitrary secret formats cannot exist.

## Stage, fresh launch and recorded ownership

The packet pins source commit
`c0aa2bce5cf0b87414b9ddfc27e85105604a4bfd`, source fingerprint
`A723E4048C6CC56423F3AE55F137A977E0ABC40C4E0818E31699BC560B93E663`,
and stage `.qa/stage/c0aa2bce-20261104113342170`.
The context's complete stage manifest equals the published stage-manifest.json.
The recorded executable was independently read and hashed; it matches
`FD20A49C96AB2B2EBDF482E4D0C201BB20DA1969E7138759E023836DF61D1845`.
The manifest records source_dirty=true. This is evidence for that recorded
fingerprint/executable, not a clean-checkout claim or proof for later patches.

The actual stage log records successful staging in404s. Independently recomputed
compiler diagnostic counts are zero warnings and zero errors, with no panic
marker. This production build is not the final integrated checks sweep.

The launch log records a fresh terrain fixture run
`.qa/run/20261104-113656-546`, PID107032, successful API/bootstrap readiness,
three fixture values summing60, both actual terrain petal bindings, and16s total
launch. The public fixture IDs agree with the target/readbacks:

- Verse `01M6KXKCM5TA69VBG9JRQMAQP4`.
- Petal A `01M6KXKD6NBF7RKTB6FSFAF2NQ`.
- Control Petal B `01M6KXKD9MHR48YWM8NH6BSE3S`.

Successful fixture setup is setup evidence, not a replacement for the retained
explicit update or for negative authority cases. The packet does not export the
runtime DB-reader mode. No direct-reader/channel-only claim is inferred.

## Explicit normal update and readback

Independently compared the complete JSON objects in:

- owner-terrain-a-before.jsonl;
- terrain-layer-update-request.json;
- owner-set-terrain-layer-once.jsonl;
- owner-terrain-a-after.jsonl.

The only requested config difference is `layers[0].visible`, true to false on
the satellite layer. Changing that one field in the complete before-config
produces the entire request exactly. Layer1 remains visible; origin, scale,
source mode, tileset, enabled state and every other config field are unchanged.
The successful mutation acknowledgement and the separate successful GET each
contain the whole requested config exactly. Their raw response files also match.

The two complete control Petal B responses are byte-for-byte identical, SHA256
`011bced40c7fad91003d5bab81580646544793e5debeb74eb0a2b208bee7ffff`.
This supports the recorded target isolation for this normal update, not a
universal transactional or concurrent-write guarantee.

owner-initial-terrain-a.jsonl preserves the mistyped initial Verse read refusal:
status0, `no_token_for_verse`, refused=true. That is local credential selection
failure before an HTTP request, not an authorization bypass or an ambiguously
accepted mutation. Subsequent retained commands use the actual fixture Verse.
There is one retained explicit mutation acknowledgement; the failed initial
operation was a read, and no mutation replay is recorded or needed to explain
the accepted result. No claims about unrecorded external traffic are made.

The recorded normal update and public Owner UI observations support this bounded
Owner-scoped success. They do not establish private credential-read mode,
original-caller identity beyond the endpoint contract, or rejection behavior for
stale, moved, revoked or expired grants.

## Actual screenshot and UI limits

Opened and visually inspected the actual saved
`screenshots/after-update.png`,1280x720. Independently verified its PNG dimensions,
hash and exact bytes against the run-local source named in
owner-after-update-shot.jsonl.

The image shows the normal editor with QA Terrain/Petal A, an Owner status,
rendered terrain/grid/placard, the hierarchy and Inspector; it is not black or an
error-only frame. The active-petal breadcrumb has an opaque backing. The saved
state agrees on target Verse/Petal,1280x720/1x, Select tool and no node selection.
The generic settled flag explicitly leaves DB/sync/terrain completion unknown;
the API acknowledgement and separate readback provide the persistence evidence.

A single after-image does not independently prove the satellite layer's rendered
visual semantics, a complete layout pass, all ordinary asset rendering/picking,
native-provider interaction/capture, or multi-scale responsive acceptance. This
packet grants no acceptance of later physical-target, shared-pointer or native
navigation changes.

## Logs and exact recorded cleanup

Independently read the retained actual stage/launch/stop/stdout/stderr files,
verified their bytes/hashes against packet-summary, and recomputed strict decoding,
level counts, compiler diagnostics and panic markers. Retained stdout is
1000 INFO /22 WARN /0 ERROR, with no panic marker. Stderr is empty. Warnings are
retained; this is not a warning-free runtime claim. The bounded final API log
tail is not substituted for the complete retained stdout count.

The stop log records the exact run stopped at12:02:59.557 local.
cleanup-identities.json is timestamped2026-11-04T19:03:05.8915295Z. Its eight
full PID/start-ticks/executable tuples exactly equal the context's root plus
seven recorded descendants, in order. Every saved check reports pid_present=false
and recorded_identity_present=false, and the aggregate absence flag agrees.
This verifies the saved eight-identity cleanup evidence; no new OS census or
universal post-exit descendant claim was made during this review.

## Remaining acceptance

QA0070 is not closed by this packet. Required separate evidence still includes
actual channel-only/private read operation, queued/prepared expiry/revocation/
generation/ancestry changes, both auth-table loss cases, transaction rollback and
driver result slots, permit/deadline/late-result ownership and supported mutator
ordering. A normal success does not prove those refusal or isolation properties.

No FULL-suite, native-provider, peer-role, retained-store restart, final checks,
exact-SHA CI, tagged-release/download or overall project-completion credit is
granted. The frozen packet is approved within its stated normal runtime smoke
scope and should retain all refusal records and limitations when published.
