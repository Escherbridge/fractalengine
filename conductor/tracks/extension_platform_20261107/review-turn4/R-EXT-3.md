---
type: Review
---
# R-EXT-3 - adversarial review gate (EXT-10, EXT-11, EXT-12, EXT-13), security review included

Diffs: 292b19c4 (EXT-10), 446e52e2 (EXT-11), working tree (EXT-12, uncommitted), b258bf66 (EXT-13). Security inbox: no entry names an owned file.

R1 FIXED - fe-database/tests/entity_property_test.rs:625 - EXT-11 test sent `since_ms: Some(0)` but `DbCommand::ReadPetalReadings.since_ms` is `i64` (fe-runtime/src/messages.rs:1181); compile error, now `since_ms: 0`.
R2 FIXED - fe-database/src/handlers/petal_readings.rs:45 - summary folded by (node, metric) only, so a node's "ft" reading counted toward (and could overwrite the value of) its "m" series with the wrong units label; key is now (node, metric, units), test expects 3 rows: m (12.0, 3), ft (33.0, 1), other (999.0, 1).
R3 FIXED - fe-database/src/handlers/petal_readings.rs:31 - source query ordered by recorded_at_ms only, so same-millisecond readings picked a nondeterministic "latest"; now `ORDER BY recorded_at_ms DESC, reading_id DESC`.
R4 FIXED - fe-database/src/handlers/petal_readings.rs:57 - replicated node_id/metric/units reached the UI with no length bound (node_measurements caps 256/128); oversized rows are now skipped (NAME_LIMIT 256, UNITS_LIMIT 128).
R5 FIXED - fe-api/src/export.rs:150 - sort comparator allocated a String per comparison (O(n log n) allocations on up to 500k rows); key now borrows `(i64, &str)`.
R6 FIXED - fe-ui/src/panels/egress_card.rs:205 - snapshot label showed raw epoch ms ("Snapshot as of 1763... ms"); now a UTC timestamp via chrono; duplicate now_epoch_ms removed in favour of gis::readings::now_unix_ms.
R7 FIXED - docs/bi-egress.md:180 - docs claimed identical bytes for a pinned snapshot without the caveats: anchor positions are live, share redemption drops as_of_ms (the Readings SQL carries the cutoff), and an unordered LIMIT query is not stable.
R8 KEPT (correct as written) - fe-database/src/handlers/node_measurements.rs:59 - EXT-10 DESC + truncate + reverse keeps the newest `limit` points ascending by (recorded_at_ms, reading_id), matching the UI order check (fe-ui/src/object_data/measurements.rs:62-66).
R9 KEPT (security holds) - fe-database/src/handlers/petal_readings.rs:23 - Viewer check at the petal scope before and after the read, node ancestry filtered to non-tombstoned nodes of that petal; Scoped foreign grant refused (test leg).
R10 KEPT (security holds) - fe-api/src/export.rs:245 - as_of_ms is a typed i64 query param, only post-filters rows already past the petal egress re-check; node shape refused with 400 before the query runs; the row cap errors rather than truncates (query_guard.rs:164), so no silent cut makes a snapshot unstable.
R11 KEPT (correct) - fe-ui/src/gis/egress_strings.rs:134 - readings_sql escapes the petal id and interpolates only an i64; ORDER is an accepted FROM boundary keyword (fe-query/src/sql_guard.rs:78-90).
R12 KEPT (correct) - fe-ui/src/gis/readings.rs:93 - route claims only its own in-flight `gis-readings-*` id; sync_petal resets the state so a late reply from the old petal is dropped; poll sends at most one request, re-polls after 10 s, warns on a closed channel.
R13 DEFERRED (trigger: petals with thousands of nodes show frame cost with the Readings tab open) - fe-ui/src/gis/readings.rs:165 - each visible frame does a linear node lookup per row (<=256 rows x petal nodes); cache resolved names per summary if it shows up.
R14 HANDED-OFF (I-DOC-CORE) - fe-database/src/AGENTS.md:702 - says latest-per-(node, metric); must say (node, metric, units) and that oversized names are skipped.
R15 HANDED-OFF (I-DOC-API) - fe-api/AGENTS.md:962 - add: a readings snapshot does not pin anchor positions; share links rely on the cutoff in the SQL.

Verdict: PASS-WITH-FIXES
