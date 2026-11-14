---
type: Spec
date: 2026-11-01
status: in_progress
title: Rich object schemas, sequential simulation and historical data
---

# Rich object data and history

The user prioritized complex object schemas, appended data, object creation, updates and transforms, with a
historical timeseries model. They explicitly selected both replayable object changes and charted measurements.
This work is a priority within the active QA campaign; full QA convergence, final CI and tagged downloads remain
required. The source investigation is `.omc/research/qa-rich-data-simulation-plan.md`.

## Required behavior

1. Nested object/array/scalar input retains its JSON type. Invalid input remains editable, displays an error and
   does not silently become a string or overwrite stored values. Edits are confirmed by authoritative persistence.
2. Optional versioned property validation descriptors enforce a documented bounded vocabulary at the DB boundary.
   Legacy definitions remain metadata until explicitly upgraded. Definition changes validate existing values;
   incompatible changes fail without implicit backfill or mutation. Ancestor constraints combine conjunctively.
3. Ordinary creation, property set/delete and full transforms commit state, operation intention and existing
   `node_log` history atomically. Per-object commit versions support exact replay independently of transform
   `edit_seq`, record time, HLC and measurement event time. Legacy or unsupported intervals are explicit.
4. History reads authorize each retained fact and baseline, not only current ancestry. Queued writes recheck the
   validated grant at DB use. Local desktop operations resolve the local identity's role. Reply family, request
   identity, node identity and embedded-state identity must all agree.
5. Scoped REST/MCP history and state-at-version interfaces provide bounded pagination and availability metadata.
   Raw journal tables remain excluded from generic SQL. Current materialization and historical previews use
   distinct shared types; reading history must not apply a historical transform to the live scene.
6. Desktop wiring shows complete initial object transforms/data, confirmed updates, a bounded history/version
   viewer, and charted appended measurements with a readable data alternative. Measurement time and object commit
   order are distinct. The first contract does not imply a global multi-object wall-time snapshot.
7. An editable declarative driver proves actual API/DB behavior and produces scrubbed current-pass receipts and
   reports. Its explicit transformations are not advertised as an automatic application rule engine. Plugin
   parity requires the separately queued real host wiring; logged dummy receipts are insufficient.
8. The Inspector presents stored custom values in a width-filling Key/Type/Value/Actions grid with Edit, Copy and
   confirmed Delete. Nested details and editors remain bounded; narrow layouts retain reachable actions through
   horizontal scrolling. Copy uses the stored typed value, independently of compact previews and dirty drafts.
9. Every Verse, Fractal, Petal and Node tree entry exposes clear selection and contextual Rename, Options, Copy ID
   and legitimate confirmed Delete. One appearance window places an icon palette beside a circular color wheel
   and editable hex field, with a preview and Save/Cancel/Reset. Per-entry colors and icons persist as local device
   display preferences. Selection, path and visibility remain recognizable independently of those choices.
   Hierarchy mutations require stored authorization and an atomic cascade; temporarily disabled controls do not
   constitute final acceptance of the requested deletion workflow.

## Selection and Petal groups extension

The tree, right Inspector and viewport tools share NodeManager's selected set and active primary object.
Plain click replaces, Ctrl/Cmd toggles and Shift selects a visible range; active-Petal navigation clears
selection. Row glyphs follow text size with thin strokes, retaining accessible hit targets. Primary,
additional, partial-group and navigation-path states remain distinguishable from chosen icon colors.

Reuse existing Petal visibility groups as nonowning many-to-many collections. New groups use fresh stable
IDs, preserve legacy IDs and retain their objects on deletion. Creating a group from selection commits
the registry and all memberships atomically after current authorization and target validation. Group
selection reports hidden or unavailable members that were excluded.

The Inspector shows the selected count, primary, common values and mixed values. Primary-only data such
as a single object's history is explicitly labeled. Tools expose the common capability intersection;
unsupported mixed operations refuse as a whole. Supported group transformations use an explicit shared
selection-center pivot, capture all original poses and versions once, restore all on cancellation, and
commit atomically with one Undo action. Paths and derived stamps require their own truthful adapters.
Group registry changes publish with the petal row (QA-0123). Group membership transactions are not yet convergent: the scene publication seam defers them (fe-database/src/handlers/scene_publication/AGENTS.md:120-122). Peer visibility still needs an in-app paired check before it is claimed.

History playback exposes completed saves. Individual field audit facts remain available, but an interior
atomic-save cut returns no reconstructed state and points to the completed boundary. Explicit validated
batch identities distinguish adjacent saves even when timestamps match. Malformed tagged baselines
and incomplete groups fail closed. See decision ledger D-QA-017 through D-QA-019.

## Independent gold scenario

Alpha/Beta/Gamma carry nested calibration and metadata. Raw readings arrive in timestamp order T0,T2,T1. Alpha's
calibrated values must be21/23/25 (mean23,sum69); Beta's9/10/11 (mean10,sum30). A total-only99 is insufficient
because it coincides with the raw total. Stored movement totals are10m/8m/4m, followed by full rotation and
nonuniform scale checks. A version map records explicit per-object cuts.

After Alpha changes from gain2 to gain3 and moves to[6,0,8], an earlier committed version must still recover
gain2 at[3,0,4]. Page boundaries, legacy coverage refusal, foreign/unknown scope redaction, queued reparent denial,
journal-failure rollback and same-root restart are required. Use real renderable sample assets, not the tiny test
GLB placeholder. A scoped actor's old measurements currently joining current geometry must not be described as
historical position without the explicit version association.

## Acceptance boundaries

The initial descriptor is a named bounded property vocabulary, not full JSON Schema. Outer property presence is
optional; nested required fields are enforced. Defaults are validated without automatic backfill. Storage rejects
unsigned values above signed64 before loss; exact signed integers above2^53 remain supported. Mixed float/integer
constraints that would lose precision refuse explicitly. Unsupported keywords and null constraints fail clearly.

Source parsing and static review are not compilation, live persistence, complete feature coverage or release
readiness. New features must enter the canonical catalog and both final current-source coverage passes. The user
requires all fixes before the final integrated test/lint sweep; no new small test functions are introduced.
