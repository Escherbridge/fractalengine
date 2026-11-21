//! DataFusion `TableProvider` over opened observation hexons (FR-13, D-CL44).
//! The caller supplies already-decrypted hexon bytes; this module does no I/O and holds no keys.
//! Projections and range/equality filters are pushed down to Parquet row-group statistics.

use std::any::Any;
use std::sync::Arc;

use bytes::Bytes;
use datafusion::arrow::array::RecordBatch;
use datafusion::arrow::datatypes::{Schema, SchemaRef};
use datafusion::catalog::Session;
use datafusion::common::ScalarValue;
use datafusion::datasource::memory::MemTable;
use datafusion::datasource::{TableProvider, TableType};
use datafusion::error::DataFusionError;
use datafusion::logical_expr::{Expr, Operator, TableProviderFilterPushDown};
use datafusion::physical_plan::ExecutionPlan;
use fe_format::HexonArchive;
use parquet::arrow::arrow_reader::ParquetRecordBatchReaderBuilder;
use parquet::arrow::ProjectionMask;
use parquet::file::metadata::{ParquetMetaData, RowGroupMetaData};
use parquet::file::statistics::Statistics;

use super::{profile, read_observation_hexon, ObservationHexonError};

/// One validated hexon's `observations/readings.parquet`, held in memory.
#[derive(Debug, Clone)]
struct ReadingsMember {
    parquet: Bytes,
    metadata: Arc<ParquetMetaData>,
}

/// Table `readings` over every supplied observation hexon (one partition per hexon).
#[derive(Debug)]
pub struct ObservationScanTable {
    members: Vec<ReadingsMember>,
    schema: SchemaRef,
}

/// One pushed-down `column <op> literal` conjunct.
#[derive(Debug, Clone)]
struct Predicate {
    column: usize,
    op: Operator,
    literal: ScalarValue,
}

impl ObservationScanTable {
    /// Validates each hexon against the SPEC-10 profile before it can be scanned.
    pub fn try_new<'a>(
        hexons: impl IntoIterator<Item = &'a [u8]>,
    ) -> Result<Self, ObservationHexonError> {
        let mut members = Vec::new();
        for hexon_bytes in hexons {
            read_observation_hexon(hexon_bytes)?;
            let raw = HexonArchive::read_member(
                hexon_bytes,
                profile::READINGS_MEMBER,
                fe_format::limits::MAX_ENTRY_UNCOMPRESSED_BYTES,
            )?
            .ok_or_else(|| {
                ObservationHexonError::ProfileViolation(format!(
                    "missing required member {}",
                    profile::READINGS_MEMBER
                ))
            })?;
            let parquet = Bytes::from(raw);
            let metadata = ParquetRecordBatchReaderBuilder::try_new(parquet.clone())?
                .metadata()
                .clone();
            members.push(ReadingsMember { parquet, metadata });
        }
        Ok(Self {
            members,
            schema: Arc::new(profile::readings_arrow_schema()),
        })
    }

    /// `(hexon index, row group index)` pairs whose statistics may satisfy every filter.
    pub fn surviving_row_groups(&self, filters: &[Expr]) -> Vec<(usize, usize)> {
        let predicates = collect_predicates(filters, &self.schema);
        let mut kept = Vec::new();
        for (member_index, member) in self.members.iter().enumerate() {
            for group_index in 0..member.metadata.num_row_groups() {
                if row_group_may_match(member.metadata.row_group(group_index), &predicates) {
                    kept.push((member_index, group_index));
                }
            }
        }
        kept
    }
}

#[async_trait::async_trait]
impl TableProvider for ObservationScanTable {
    fn as_any(&self) -> &dyn Any {
        self
    }

    fn schema(&self) -> SchemaRef {
        Arc::clone(&self.schema)
    }

    fn table_type(&self) -> TableType {
        TableType::Base
    }

    fn supports_filters_pushdown(
        &self,
        filters: &[&Expr],
    ) -> Result<Vec<TableProviderFilterPushDown>, DataFusionError> {
        // Inexact: statistics only skip row groups; DataFusion re-applies the filter to rows.
        Ok(filters
            .iter()
            .map(|filter| {
                let mut found = Vec::new();
                collect_expr_predicates(filter, &self.schema, &mut found);
                if found.is_empty() {
                    TableProviderFilterPushDown::Unsupported
                } else {
                    TableProviderFilterPushDown::Inexact
                }
            })
            .collect())
    }

    async fn scan(
        &self,
        state: &dyn Session,
        projection: Option<&Vec<usize>>,
        filters: &[Expr],
        limit: Option<usize>,
    ) -> Result<Arc<dyn ExecutionPlan>, DataFusionError> {
        // Columns actually decoded from Parquet: ascending, deduplicated; column 0 stands in
        // for an empty projection (count(*)) so row counts survive.
        let read_columns: Vec<usize> = match projection {
            None => (0..self.schema.fields().len()).collect(),
            Some(wanted) if wanted.is_empty() => vec![0],
            Some(wanted) => {
                let mut sorted = wanted.clone();
                sorted.sort_unstable();
                sorted.dedup();
                sorted
            }
        };
        let read_schema: SchemaRef = Arc::new(
            Schema::new(
                read_columns
                    .iter()
                    .map(|&index| self.schema.field(index).clone())
                    .collect::<Vec<_>>(),
            ),
        );
        let table_projection: Option<Vec<usize>> = match projection {
            None => None,
            Some(wanted) => Some(
                wanted
                    .iter()
                    .map(|index| {
                        read_columns.binary_search(index).map_err(|_| {
                            DataFusionError::Internal(format!(
                                "projection column {index} missing from the decoded set"
                            ))
                        })
                    })
                    .collect::<Result<Vec<_>, _>>()?,
            ),
        };

        let surviving = self.surviving_row_groups(filters);
        let mut partitions: Vec<Vec<RecordBatch>> = Vec::new();
        for (member_index, member) in self.members.iter().enumerate() {
            let groups: Vec<usize> = surviving
                .iter()
                .filter(|(owner, _)| *owner == member_index)
                .map(|(_, group)| *group)
                .collect();
            if groups.is_empty() {
                continue;
            }
            let builder = ParquetRecordBatchReaderBuilder::try_new(member.parquet.clone())
                .map_err(|e| DataFusionError::External(Box::new(e)))?;
            let mask = ProjectionMask::roots(builder.parquet_schema(), read_columns.iter().copied());
            let reader = builder
                .with_row_groups(groups)
                .with_projection(mask)
                .build()
                .map_err(|e| DataFusionError::External(Box::new(e)))?;
            let mut batches = Vec::new();
            for batch in reader {
                batches.push(batch.map_err(|e| DataFusionError::ArrowError(e, None))?);
            }
            partitions.push(batches);
        }
        if partitions.is_empty() {
            partitions.push(Vec::new());
        }

        let memory = MemTable::try_new(read_schema, partitions)?;
        memory
            .scan(state, table_projection.as_ref(), &[], limit)
            .await
    }
}

fn collect_predicates(filters: &[Expr], schema: &Schema) -> Vec<Predicate> {
    let mut out = Vec::new();
    for filter in filters {
        collect_expr_predicates(filter, schema, &mut out);
    }
    out
}

/// Gathers the `column <op> literal` conjuncts of `expr`; anything else is ignored (never pruned on).
fn collect_expr_predicates(expr: &Expr, schema: &Schema, out: &mut Vec<Predicate>) {
    let Expr::BinaryExpr(binary) = expr else {
        return;
    };
    if binary.op == Operator::And {
        collect_expr_predicates(&binary.left, schema, out);
        collect_expr_predicates(&binary.right, schema, out);
        return;
    }
    let (column_expr, literal_expr, op) = match (binary.left.as_ref(), binary.right.as_ref()) {
        (Expr::Column(_), Expr::Literal(_)) => (&*binary.left, &*binary.right, binary.op),
        (Expr::Literal(_), Expr::Column(_)) => match flip(binary.op) {
            Some(flipped) => (&*binary.right, &*binary.left, flipped),
            None => return,
        },
        _ => return,
    };
    if !matches!(
        op,
        Operator::Eq | Operator::NotEq | Operator::Lt | Operator::LtEq | Operator::Gt | Operator::GtEq
    ) {
        return;
    }
    if let (Expr::Column(column), Expr::Literal(literal)) =
        (column_expr, literal_expr)
    {
        if let Ok(index) = schema.index_of(&column.name) {
            out.push(Predicate {
                column: index,
                op,
                literal: literal.clone(),
            });
        }
    }
}

fn flip(op: Operator) -> Option<Operator> {
    Some(match op {
        Operator::Eq => Operator::Eq,
        Operator::NotEq => Operator::NotEq,
        Operator::Lt => Operator::Gt,
        Operator::LtEq => Operator::GtEq,
        Operator::Gt => Operator::Lt,
        Operator::GtEq => Operator::LtEq,
        _ => return None,
    })
}

fn row_group_may_match(group: &RowGroupMetaData, predicates: &[Predicate]) -> bool {
    predicates.iter().all(|predicate| {
        match group.column(predicate.column).statistics() {
            Some(stats) => statistics_may_match(stats, predicate.op, &predicate.literal),
            None => true,
        }
    })
}

/// Conservative: `false` only when the min/max range proves no row can satisfy `column op literal`.
fn statistics_may_match(stats: &Statistics, op: Operator, literal: &ScalarValue) -> bool {
    match stats {
        Statistics::Int64(s) => integer_literal(literal).map_or(true, |lit| {
            range_may_match(s.min_opt().copied(), s.max_opt().copied(), op, lit)
        }),
        Statistics::Int32(s) => integer_literal(literal).map_or(true, |lit| {
            range_may_match(
                s.min_opt().map(|v| i64::from(*v)),
                s.max_opt().map(|v| i64::from(*v)),
                op,
                lit,
            )
        }),
        Statistics::Double(s) => float_literal(literal).map_or(true, |lit| {
            range_may_match(s.min_opt().copied(), s.max_opt().copied(), op, lit)
        }),
        Statistics::ByteArray(s) => string_literal(literal).map_or(true, |lit| {
            let min = s.min_opt().and_then(|v| v.as_utf8().ok());
            let max = s.max_opt().and_then(|v| v.as_utf8().ok());
            range_may_match(min, max, op, lit)
        }),
        _ => true,
    }
}

fn range_may_match<T: PartialOrd + Copy>(
    min: Option<T>,
    max: Option<T>,
    op: Operator,
    lit: T,
) -> bool {
    match op {
        Operator::Eq => min.map_or(true, |m| m <= lit) && max.map_or(true, |m| m >= lit),
        Operator::NotEq => match (min, max) {
            (Some(lo), Some(hi)) => !(lo == lit && hi == lit),
            _ => true,
        },
        Operator::Lt => min.map_or(true, |m| m < lit),
        Operator::LtEq => min.map_or(true, |m| m <= lit),
        Operator::Gt => max.map_or(true, |m| m > lit),
        Operator::GtEq => max.map_or(true, |m| m >= lit),
        _ => true,
    }
}

fn integer_literal(literal: &ScalarValue) -> Option<i64> {
    match literal {
        ScalarValue::Int8(Some(v)) => Some(i64::from(*v)),
        ScalarValue::Int16(Some(v)) => Some(i64::from(*v)),
        ScalarValue::Int32(Some(v)) => Some(i64::from(*v)),
        ScalarValue::Int64(Some(v)) => Some(*v),
        ScalarValue::UInt8(Some(v)) => Some(i64::from(*v)),
        ScalarValue::UInt16(Some(v)) => Some(i64::from(*v)),
        ScalarValue::UInt32(Some(v)) => Some(i64::from(*v)),
        ScalarValue::UInt64(Some(v)) => i64::try_from(*v).ok(),
        _ => None,
    }
}

/// `None` for NaN so a NaN literal never prunes (DataFusion orders NaN above every number).
fn float_literal(literal: &ScalarValue) -> Option<f64> {
    let value = match literal {
        ScalarValue::Float64(Some(v)) => *v,
        ScalarValue::Float32(Some(v)) => f64::from(*v),
        other => integer_literal(other)? as f64,
    };
    (!value.is_nan()).then_some(value)
}

fn string_literal(literal: &ScalarValue) -> Option<&str> {
    match literal {
        ScalarValue::Utf8(Some(v))
        | ScalarValue::LargeUtf8(Some(v))
        | ScalarValue::Utf8View(Some(v)) => Some(v.as_str()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::super::{write_observation_hexon, ReadingRow};
    use super::*;
    use datafusion::arrow::array::{Array, Float64Array, Int64Array};
    use datafusion::prelude::{col, lit, SessionContext};

    fn reading(id: &str, node: &str, metric: &str, value: f64, wall_ms: u64) -> ReadingRow {
        ReadingRow {
            reading_id: id.into(),
            node_id: node.into(),
            metric: metric.into(),
            value,
            units: "celsius".into(),
            recorded_at: "2026-10-14T00:00:00.000+00:00".into(),
            recorded_at_ms: 1_760_400_000_000 + wall_ms as i64,
            hlc_timestamp: wall_ms << 16,
            payload_id: None,
            source_index: 0,
        }
    }

    /// AC-13.1: a windowed aggregate over two pulled hexons equals the same aggregate over the
    /// source rows, and the window's statistics skip the hexon wholly outside it.
    #[tokio::test]
    async fn windowed_aggregate_over_pulled_hexons_matches_source_rows() {
        let early: Vec<ReadingRow> = (0..5)
            .map(|i| reading(&format!("a{i}"), "n1", "temp", 1.0 + i as f64, 10 + i))
            .collect();
        let late: Vec<ReadingRow> = (0..5)
            .map(|i| reading(&format!("b{i}"), "n2", "temp", 100.0 + i as f64, 1_000 + i))
            .collect();
        let hexon_early = write_observation_hexon("did:key:z6Mktest", &early, &[], 1).unwrap();
        let hexon_late = write_observation_hexon("did:key:z6Mktest", &late, &[], 1).unwrap();

        let table = Arc::new(
            ObservationScanTable::try_new([
                hexon_early.hexon_bytes.as_slice(),
                hexon_late.hexon_bytes.as_slice(),
            ])
            .unwrap(),
        );

        let window_filter = vec![col("hlc_wall_ms").gt_eq(lit(900_i64))];
        assert_eq!(
            table.surviving_row_groups(&window_filter),
            vec![(1, 0)],
            "the early hexon's row-group statistics must exclude it from the window"
        );

        let ctx = SessionContext::new();
        ctx.register_table("readings", table).unwrap();
        let batches = ctx
            .sql(
                "SELECT count(*) AS n, sum(value) AS total FROM readings \
                 WHERE hlc_wall_ms >= 900 AND metric = 'temp'",
            )
            .await
            .unwrap()
            .collect()
            .await
            .unwrap();

        let expected_total: f64 = late.iter().map(|r| r.value).sum();
        let count = batches[0]
            .column(0)
            .as_any()
            .downcast_ref::<Int64Array>()
            .unwrap()
            .value(0);
        let total = batches[0]
            .column(1)
            .as_any()
            .downcast_ref::<Float64Array>()
            .unwrap()
            .value(0);
        assert_eq!(count, late.len() as i64);
        assert!((total - expected_total).abs() < 1e-9);
    }
}
