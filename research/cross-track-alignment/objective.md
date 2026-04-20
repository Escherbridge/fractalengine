# Research Objective: Cross-Track Alignment Analysis

## Subject
Dialectic/Socratic analysis of alignment between two FractalEngine Conductor tracks:
- **inspector_settings_20260419**: Inspector panel expansion (URL persistence, tab system, hierarchy inspection, RBAC UI)
- **profile_manager_20260419**: User identity & profile management (display, editing, identity management, P2P sync)

## Core Research Questions

1. **Dependency ordering**: Do the tracks handle the case where one is implemented before the other? Are there circular dependencies between phases?
2. **Data model alignment**: Are `PeerAccessEntry` (Inspector) and `UserProfile`/`PeerProfileCache` (Profile) redundant, conflicting, or properly complementary?
3. **Ownership gaps**: Which track owns online/offline state, display name resolution, role data, and the assembly of peer entries in the Access tab?
4. **Security model coherence**: Is admin verification, identity switching impact on roles, and stale profile eviction vs. persistent roles handled consistently?
5. **Shared infrastructure**: What common systems (SurrealDB schema, P2P message types, Bevy resources) do both tracks need that neither fully specifies?

## Success Criteria

- [ ] All 8 personas deployed with distinct analytical perspectives
- [ ] Contradictions between specs identified and documented
- [ ] Ownership gaps mapped (who builds what)
- [ ] Temporal ordering constraints identified
- [ ] Shared infrastructure requirements extracted
- [ ] Security model gaps surfaced
- [ ] Actionable recommendations for spec amendments produced

## Evidence Standards

- Primary evidence: the spec and plan documents themselves
- Secondary evidence: existing codebase patterns and structures
- Tertiary evidence: analogous patterns in similar P2P/UI frameworks
- All claims must reference specific spec sections or code locations

## Perspectives to Consider

- Implementer who builds Inspector Settings first, then Profile Manager
- Implementer who builds Profile Manager first, then Inspector Settings
- Implementer who builds both in parallel
- Security auditor reviewing the combined system
- End user experiencing the features
- Future maintainer extending either track

## Potential Biases to Guard Against

- Assuming sequential implementation (tracks may be built in parallel)
- Treating specs as complete (they explicitly have open questions)
- Over-indexing on the happy path (edge cases matter most for alignment)
