---
type: ScenarioSupplement
id: S06
legacy_gold: G6
covered_by: [J03]
title: Run scoped Inspector queries with a nonzero data oracle
fixture: basic or terrain
verified_in_pass: "pending seeded-value UI validation"
tags: [gold, inspector, query, surrealql, analytic-oracle]
---

# Query three known values and their sum

Both seeded modes persist Cube 1/2/3 in Petal A with `qa_fixture=terrain`,
`qa_role=value` and numeric `qa_value=10/20/30`. Two origin marker rows have
`qa_role=marker`; they must not inflate the numeric oracle. Launch checks
property readback and scoped query count 3/sum 60 before handing off the run.
That setup check is independent evidence and does not complete this UI task.

1. Enter Petal A and open Inspector, then its named Query tab. Inspect
   the SurrealQL editor, current petal scope, template controls and disabled
   Run Query while empty. Use current semantic names and visible geometry;
   record clipped or unreachable controls instead of clicking off-window.
2. Choose **All nodes in this petal** and click **Run Query**. Inspect the
   actual editor and returned rows. A fresh fixture contains three Cube
   value nodes plus its QA oracle origin marker. Additional nodes created
   by earlier scenarios must be explained, not treated as missing values.
3. Replace the editor with `query_rows_sql` from `F info`'s fixture block,
   using normal text input. This query constrains the exact Petal A id,
   `properties.qa_fixture='terrain'`, `properties.qa_role='value'` and
   `tombstone=NONE`. Click Run Query. Inspect exactly three returned rows,
   matching fixture node IDs/names and values 10,20,30, sorted by value.
4. Replace it with `query_sum_sql` from the same fixture metadata and
   submit with Ctrl+Enter. The query selects count() as fixture_count and
   math::sum(properties.qa_value) as fixture_sum, with the same WHERE and
   GROUP ALL. Inspect one actual result row: fixture_count 3 and
   fixture_sum 60. Capture both row and aggregate results.
5. Enter Petal B and run the equivalent value query with Petal B's id.
   It has an origin marker and may have an earthwork region, but no tagged
   value rows. Inspect the honest empty result. Return to Petal A and
   verify the previous numeric query can still return 3/60. An error, stale
   result, scoped denial or endless loading state is a finding.

Read-only API queries may corroborate persistence after each UI action,
but cannot replace entering SQL, clicking Run Query or keyboard submission.
Judge result readability and units separately from correctness. Never accept
an empty table or initial zero aggregate as proof of the nonzero oracle,
and do not use clipboard commands as input.
