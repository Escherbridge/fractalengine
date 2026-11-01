---
type: Review
date: 2026-11-01
reviewer: harness_security_review
source_branch: qa/s2-complete
source_range: 214045a..1974bac
scope: response names, actual-response observations, normal OS accessibility timing preparation
---

# S2 independent static review

The released branch is statically clean. This verdict covers the reviewed branch patches, not an unreviewed
merge or runtime behavior. Integration compilation and exhaustive live auditing remain pending.

ResponseExt preserves native response semantics. Named labels are lazy and describe control purpose/context;
labelled relationships remain native; observed records the actual response. Explicit Button metadata is limited
to custom click targets. Repeated actions identify their target without using generated tokens or current
sensitive input contents as accessible names. Results cells expose row/column context and retain their values
separately. The canonical production confirmation helper accepts a fifth accessible-name parameter; its existing
journey calls exercise the same helper, without a test-only compatibility API.

The source author corrected a browser-card splice before release: names attach to leaf button responses. The
complete patch sequence must be integrated together, including ad2e24c after 4fb9834. The final actual Rust sources
were parsed; this is not compilation or proof that every toolkit-created control has a name.

The host collector requires explicit timing opt-in plus a validated QA root. It leaves normal accessibility
activation/forwarding intact, rejects a synthetic-input comparison, bounds samples and phase timers, deduplicates
frame measurements and writes fixed owner-only outputs. It records context and incomplete/preactivated outcomes.
Its metric is application frame wall time, including scheduling/vsync/GPU work, not isolated accessibility CPU
cost. The optional continuous cadence applies before both measured phases and is reported.

The UIA helper uses a bounded hidden MTA child, verified process identity and HWND, process-lifetime-bound markers,
and an owned-descendant filter for fixed Button names. It does not enumerate the desktop root, read field values,
invoke actions or change focus. A primary-agent correction in 1974bac makes an exhausted probe exit 2; the separate
reviewer verified that the wrapper also returns failure, so absent controls cannot yield shell success.

Remaining gates: preserve property-correlation fields and strict divider endpoints during integration; compile;
wire explicit gate-off/timing launch options; prove actual normal OS activation with debug routes still denied;
collect a valid same-process timing pair; inspect every reachable panel, modal, popup, disabled state and repeated
action through the live tree. Toolkit scroll/window controls and the prior 96–163 observation gaps remain audit
work. Owner-chip wrapping and compact Inspector-tab clipping remain layout follow-ups.

No Cargo, apps or UIA probes were run by this reviewer. No source inventory or parser result establishes full
coverage, performance acceptance or final release readiness.

A separate design review also approved the response adapter, canonical confirmation API, Results context/value
semantics and bounded timing state machine. It checked pinned accesskit_windows: an explicit cell value exposes
the read-only UIA value pattern independently of its contextual Button label. No additional strategy hierarchy
is warranted. Main integration through da71a1b preserves the property-correlation additions and strict pointer
boundaries. Compilation and runtime evidence remain pending.

Launcher follow-up9c4b2e5 supplies explicit gate-off/timing options with invalid-combination rejection and run
provenance. Review found invoking-process environment leakage;11cb8d4 snapshots/restores all nine assigned values
in finally, including unset values and launch failure. A separate final static recheck closed that finding.

VERDICT: APPROVE STATIC BRANCH PATCHES; INTEGRATION AND LIVE VERIFICATION PENDING
