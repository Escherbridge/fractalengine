---
type: QA Pass Verdict
pass: "10"
focused_smoke: true
full_coverage: false
status: schema_boot_failure_followup_pending
---

Combined source da07a22 compiled and staged with executable E8E679D618275DB8CA219DE860A3BBB544E971A07B5EC4B4E018417B5AE6627D. Same saved root20261101-191920-658, no reseed. Startup passed the corrected default_val declaration but refused field_def.schema_revisions TYPE option<array> FLEXIBLE. This is a second declaration under existingQA-0031, not a clean boot or UI acceptance.

Root58632 failed before health; launcher recorded failure and cleanup for captured identities. Independent process inspection confirmed PID58632 absent. Root had gone before a fresh descendant snapshot, so no exhaustive child-absence claim is made.

Followup source changes schema_revisions to option<array<object>> FLEXIBLE and explicitly overwrites outer and wildcard-element declarations, using the existing visibility_groups storage pattern. New build, startup and schema/default roundtrips remain required. No screenshot, control, chart, simulation, or full-coverage assertion was exercised.
