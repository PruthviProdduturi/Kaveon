use std::collections::VecDeque;

use arrow::array::{Array, Int64Array, UInt32Array};
use arrow::compute::{SortColumn, SortOptions, concat_batches, lexsort_to_indices, take};
use arrow::datatypes::SchemaRef;
use arrow::record_batch::RecordBatch;
use kaveon_core::{BatchOperator, KaveonError, MemoryReservation, OperatorMemoryAccount, Result};

use crate::expr_eval::evaluate;
use crate::sort::{ExternalMerge, SortExpr, compact_runs};
use crate::spill::SpillManager;

pub struct TopNOperator {
    source: Box<dyn BatchOperator>,
    sort_exprs: Vec<SortExpr>,
    limit: usize,
    schema: SchemaRef,
    output: VecDeque<RecordBatch>,
    initialized: bool,
    spill: Option<(OperatorMemoryAccount, SpillManager)>,
    external_merge: Option<ExternalMerge>,
    merge_fan_in: usize,
    memory: Option<OperatorMemoryAccount>,
    output_memory: Vec<MemoryReservation>,
}

const DEFAULT_MERGE_FAN_IN: usize = 16;
const MINIMUM_MERGE_FAN_IN: usize = 2;

impl TopNOperator {
    pub fn new(
        source: Box<dyn BatchOperator>,
        sort_exprs: Vec<SortExpr>,
        limit: usize,
    ) -> Result<Self> {
        if sort_exprs.is_empty() {
            return Err(KaveonError::Execution(
                "TopN requires at least one ordering expression".into(),
            ));
        }
        let schema = source.schema().clone();
        Ok(Self {
            source,
            sort_exprs,
            limit,
            schema,
            output: VecDeque::new(),
            initialized: false,
            spill: None,
            external_merge: None,
            merge_fan_in: DEFAULT_MERGE_FAN_IN,
            memory: None,
            output_memory: Vec::new(),
        })
    }

    pub fn new_with_spill(
        source: Box<dyn BatchOperator>,
        sort_exprs: Vec<SortExpr>,
        limit: usize,
        memory: OperatorMemoryAccount,
        spill: SpillManager,
    ) -> Result<Self> {
        let mut operator = Self::new(source, sort_exprs, limit)?;
        operator.memory = Some(memory.clone());
        operator.spill = Some((memory, spill));
        Ok(operator)
    }

    pub fn with_memory(mut self, memory: OperatorMemoryAccount) -> Self {
        self.memory = Some(memory);
        self
    }

    pub fn with_merge_fan_in(mut self, merge_fan_in: usize) -> Result<Self> {
        if merge_fan_in < MINIMUM_MERGE_FAN_IN {
            return Err(KaveonError::Execution(format!(
                "TopN spill merge fan-in must be at least {MINIMUM_MERGE_FAN_IN}"
            )));
        }
        self.merge_fan_in = merge_fan_in;
        Ok(self)
    }

    fn initialize(&mut self) -> Result<()> {
        if self.limit == 0 {
            return Ok(());
        }
        let mut batches = Vec::new();
        let mut reservations = Vec::new();
        let mut retained_bytes = 0_u64;
        let mut runs = Vec::new();
        while let Some(batch) = self.source.next_batch()? {
            if batch.num_rows() == 0 {
                continue;
            }
            let bytes = crate::sort::sort_workspace_bytes(&batch);
            if let Some((memory, spill)) = &self.spill {
                let target = memory.query().snapshot().limit_bytes / (self.merge_fan_in as u64 + 2);
                if !batches.is_empty() && retained_bytes.saturating_add(bytes) > target {
                    let candidate =
                        merge_top_n(&self.schema, &batches, &self.sort_exprs, self.limit)?
                            .expect("nonempty candidates");
                    runs.push(spill.write_run(&self.schema, &[candidate])?);
                    batches.clear();
                    reservations.clear();
                    retained_bytes = 0;
                    if runs.len() >= self.merge_fan_in.saturating_mul(2) {
                        runs = compact_runs(
                            runs,
                            spill,
                            self.schema.clone(),
                            &self.sort_exprs,
                            self.limit,
                            self.merge_fan_in,
                            Some(self.limit),
                            self.memory.as_ref(),
                        )?;
                    }
                }
            }
            let workspace = self
                .memory
                .as_ref()
                .map(|memory| memory.reserve(bytes))
                .transpose()?;
            let candidate = merge_top_n(&self.schema, &[batch], &self.sort_exprs, self.limit)?
                .expect("nonempty input");
            drop(workspace);
            let candidate_bytes = crate::sort::sort_workspace_bytes(&candidate);
            if let Some(memory) = &self.memory {
                reservations.push(memory.reserve(candidate_bytes)?);
            }
            retained_bytes = retained_bytes.saturating_add(candidate_bytes);
            batches.push(candidate);
            if self.spill.is_none() && batches.len() >= self.merge_fan_in {
                // Keep at most fan-in candidate batches. Without compaction,
                // even LIMIT 1 accumulates state proportional to input batches.
                let candidate = merge_top_n(&self.schema, &batches, &self.sort_exprs, self.limit)?
                    .expect("nonempty candidates");
                let bytes = crate::sort::sort_workspace_bytes(&candidate);
                let retained = self
                    .memory
                    .as_ref()
                    .map(|memory| memory.reserve(bytes))
                    .transpose()?;
                batches.clear();
                reservations.clear();
                retained_bytes = bytes;
                batches.push(candidate);
                if let Some(retained) = retained {
                    reservations.push(retained);
                }
            }
        }
        if batches.is_empty() && runs.is_empty() {
            return Ok(());
        }
        if !runs.is_empty() {
            let spill = &self.spill.as_ref().expect("spill mode active").1;
            if !batches.is_empty() {
                let candidate = merge_top_n(&self.schema, &batches, &self.sort_exprs, self.limit)?
                    .expect("nonempty candidates");
                runs.push(spill.write_run(&self.schema, &[candidate])?);
            }
            batches.clear();
            reservations.clear();
            let runs = compact_runs(
                runs,
                spill,
                self.schema.clone(),
                &self.sort_exprs,
                self.limit,
                self.merge_fan_in,
                Some(self.limit),
                self.memory.as_ref(),
            )?;
            self.external_merge = Some(ExternalMerge::new(
                runs,
                self.schema.clone(),
                &self.sort_exprs,
                self.limit,
                Some(self.limit),
                self.memory.clone(),
            )?);
            return Ok(());
        }
        if let Some(batch) = merge_top_n(&self.schema, &batches, &self.sort_exprs, self.limit)? {
            self.output.push_back(batch);
        }
        self.output_memory = reservations;
        Ok(())
    }
}

/// Merges partition-local TopN batches into the globally ordered TopN result.
///
/// Each input partition only needs to contribute its first `limit` rows. A row
/// ranked below that boundary cannot appear in the global TopN because at least
/// `limit` rows in its own partition already rank ahead of it.
pub fn merge_top_n(
    schema: &SchemaRef,
    batches: &[RecordBatch],
    sort_exprs: &[SortExpr],
    limit: usize,
) -> Result<Option<RecordBatch>> {
    if sort_exprs.is_empty() {
        return Err(KaveonError::Execution(
            "TopN merge requires at least one ordering expression".into(),
        ));
    }
    if limit == 0 || batches.iter().all(|batch| batch.num_rows() == 0) {
        return Ok(None);
    }
    if batches.iter().any(|batch| batch.schema() != *schema) {
        return Err(KaveonError::Execution(
            "TopN merge received incompatible batch schemas".into(),
        ));
    }

    let non_empty = batches
        .iter()
        .filter(|batch| batch.num_rows() > 0)
        .cloned()
        .collect::<Vec<_>>();
    let combined = if non_empty.len() == 1 {
        non_empty[0].clone()
    } else {
        concat_batches(schema, &non_empty)?
    };
    let columns = sort_exprs
        .iter()
        .map(|sort_expr| {
            Ok(SortColumn {
                values: evaluate(&sort_expr.expr, &combined)?,
                options: Some(SortOptions {
                    descending: !sort_expr.ascending,
                    nulls_first: sort_expr.nulls_first,
                }),
            })
        })
        .collect::<Result<Vec<_>>>()?;
    let indices = integer_top_n_indices(&columns, limit)
        .map(Ok)
        .unwrap_or_else(|| lexsort_to_indices(&columns, Some(limit)))?;
    let output_columns = combined
        .columns()
        .iter()
        .map(|column| take(column.as_ref(), &indices, None))
        .collect::<std::result::Result<Vec<_>, _>>()?;
    Ok(Some(RecordBatch::try_new(schema.clone(), output_columns)?))
}

// Common integer ORDER BY keys can compare typed values directly. Other types
// and nullable arrays retain Arrow's complete ordering semantics.
fn integer_top_n_indices(columns: &[SortColumn], limit: usize) -> Option<UInt32Array> {
    if columns.len() < 2 {
        return None;
    }
    let keys = columns
        .iter()
        .map(|column| {
            let values = column.values.as_any().downcast_ref::<Int64Array>()?;
            (values.null_count() == 0)
                .then_some((values, column.options.unwrap_or_default().descending))
        })
        .collect::<Option<Vec<_>>>()?;
    let rows = keys.first()?.0.len();
    if rows > u32::MAX as usize || keys.iter().any(|(key, _)| key.len() != rows) {
        return None;
    }
    let mut indices: Vec<u32> = (0..rows as u32).collect();
    let compare = |a: &u32, b: &u32| {
        for (key, descending) in &keys {
            let ordering = key.value(*a as usize).cmp(&key.value(*b as usize));
            if !ordering.is_eq() {
                return if *descending {
                    ordering.reverse()
                } else {
                    ordering
                };
            }
        }
        std::cmp::Ordering::Equal
    };
    let retained = limit.min(rows);
    if retained < rows {
        indices.select_nth_unstable_by(retained, compare);
        indices.truncate(retained);
    }
    indices.sort_unstable_by(compare);
    Some(UInt32Array::from(indices))
}

impl BatchOperator for TopNOperator {
    fn schema(&self) -> &SchemaRef {
        &self.schema
    }

    fn next_batch(&mut self) -> Result<Option<RecordBatch>> {
        let expression_memory = self.memory.clone();
        crate::expr_eval::with_expression_memory(expression_memory.as_ref(), || {
            if !self.initialized {
                self.initialized = true;
                self.initialize()?;
            }
            if let Some(merge) = &mut self.external_merge {
                return merge.next_batch();
            }
            let batch = self.output.pop_front();
            if batch.is_none() {
                self.output_memory.clear();
            }
            Ok(batch)
        })
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use arrow::array::{Array, Int64Array, StringArray};
    use arrow::datatypes::{DataType, Field, Schema};

    use super::*;
    use kaveon_core::Expr;

    #[test]
    fn typed_integer_topn_matches_arrow_for_mixed_orders_ties_and_extremes() {
        let first = Arc::new(Int64Array::from(
            (0..1024)
                .map(|row| match row % 17 {
                    0 => i64::MIN,
                    1 => i64::MAX,
                    _ => (row % 13) - 6,
                })
                .collect::<Vec<_>>(),
        ));
        let second = Arc::new(Int64Array::from(
            (0..1024).map(|row| (row * 31) % 43).collect::<Vec<_>>(),
        ));
        for descending in [false, true] {
            let columns = vec![
                SortColumn {
                    values: first.clone(),
                    options: Some(SortOptions {
                        descending,
                        nulls_first: true,
                    }),
                },
                SortColumn {
                    values: second.clone(),
                    options: Some(SortOptions {
                        descending: !descending,
                        nulls_first: false,
                    }),
                },
            ];
            for limit in [0, 1, 20, 1024, 2048] {
                let actual = integer_top_n_indices(&columns, limit).unwrap();
                let expected = lexsort_to_indices(&columns, Some(limit)).unwrap();
                // Equal keys may select different tied row indices; their values must agree.
                for column in &columns {
                    assert_eq!(
                        take(column.values.as_ref(), &actual, None)
                            .unwrap()
                            .to_data(),
                        take(column.values.as_ref(), &expected, None)
                            .unwrap()
                            .to_data()
                    );
                }
            }
        }
        let nullable = vec![
            SortColumn {
                values: Arc::new(Int64Array::from(vec![Some(1), None])),
                options: None,
            },
            SortColumn {
                values: Arc::new(Int64Array::from(vec![2, 3])),
                options: None,
            },
        ];
        assert!(integer_top_n_indices(&nullable, 1).is_none());
    }

    struct MockOperator {
        schema: SchemaRef,
        batches: VecDeque<RecordBatch>,
    }

    impl MockOperator {
        fn new(values: Vec<Vec<Option<i64>>>) -> Self {
            let schema = Arc::new(Schema::new(vec![Field::new("id", DataType::Int64, true)]));
            let batches = values
                .into_iter()
                .map(|values| {
                    RecordBatch::try_new(schema.clone(), vec![Arc::new(Int64Array::from(values))])
                        .unwrap()
                })
                .collect();
            Self { schema, batches }
        }
    }

    impl BatchOperator for MockOperator {
        fn schema(&self) -> &SchemaRef {
            &self.schema
        }

        fn next_batch(&mut self) -> Result<Option<RecordBatch>> {
            Ok(self.batches.pop_front())
        }
    }

    fn values(batch: &RecordBatch) -> Vec<Option<i64>> {
        let array = batch
            .column(0)
            .as_any()
            .downcast_ref::<Int64Array>()
            .unwrap();
        (0..array.len())
            .map(|index| {
                if array.is_null(index) {
                    None
                } else {
                    Some(array.value(index))
                }
            })
            .collect()
    }

    #[test]
    fn selects_top_n_across_batches() {
        let source = MockOperator::new(vec![vec![Some(2), Some(5)], vec![Some(4), Some(1)]]);
        let mut operator = TopNOperator::new(
            Box::new(source),
            vec![SortExpr::new(Expr::Column("id".into()), false)],
            3,
        )
        .unwrap();
        let output = operator.next_batch().unwrap().unwrap();
        assert_eq!(values(&output), vec![Some(5), Some(4), Some(2)]);
        assert!(operator.next_batch().unwrap().is_none());
    }

    #[test]
    fn candidate_memory_is_bounded_across_many_input_batches() {
        let input = MockOperator::new((0..1000).map(|value| vec![Some(value)]).collect());
        let pool = kaveon_core::QueryMemoryPool::new("topn-many-batches", 64 * 1024).unwrap();
        let mut operator = TopNOperator::new(
            Box::new(input),
            vec![SortExpr::new(Expr::Column("id".into()), false)],
            3,
        )
        .unwrap()
        .with_memory(pool.operator("topn").unwrap());
        let batch = operator.next_batch().unwrap().unwrap();
        assert_eq!(
            batch
                .column(0)
                .as_any()
                .downcast_ref::<Int64Array>()
                .unwrap()
                .values()
                .as_ref(),
            &[999, 998, 997]
        );
        assert!(operator.next_batch().unwrap().is_none());
        assert!(pool.snapshot().peak_bytes <= 64 * 1024);
        assert_eq!(pool.snapshot().current_bytes, 0);
    }

    #[test]
    fn limit_larger_than_input_returns_all_rows() {
        let source = MockOperator::new(vec![vec![Some(2), Some(1)]]);
        let mut operator = TopNOperator::new(
            Box::new(source),
            vec![SortExpr::new(Expr::Column("id".into()), true)],
            10,
        )
        .unwrap();
        assert_eq!(
            values(&operator.next_batch().unwrap().unwrap()),
            vec![Some(1), Some(2)]
        );
    }

    #[test]
    fn zero_limit_does_not_read_input() {
        let source = MockOperator::new(vec![vec![Some(1)]]);
        let mut operator = TopNOperator::new(
            Box::new(source),
            vec![SortExpr::new(Expr::Column("id".into()), true)],
            0,
        )
        .unwrap();
        assert!(operator.next_batch().unwrap().is_none());
    }

    #[test]
    fn empty_input_produces_no_batch() {
        let source = MockOperator::new(vec![]);
        let mut operator = TopNOperator::new(
            Box::new(source),
            vec![SortExpr::new(Expr::Column("id".into()), true)],
            2,
        )
        .unwrap();
        assert!(operator.next_batch().unwrap().is_none());
    }

    #[test]
    fn respects_explicit_null_ordering() {
        let source = MockOperator::new(vec![vec![Some(2), None, Some(1)]]);
        let mut operator = TopNOperator::new(
            Box::new(source),
            vec![SortExpr::new(Expr::Column("id".into()), true).with_nulls_first(true)],
            2,
        )
        .unwrap();
        assert_eq!(
            values(&operator.next_batch().unwrap().unwrap()),
            vec![None, Some(1)]
        );
    }

    #[test]
    fn reports_invalid_configuration_and_expression() {
        let source = MockOperator::new(vec![vec![Some(1)]]);
        assert!(TopNOperator::new(Box::new(source), vec![], 1).is_err());

        let source = MockOperator::new(vec![vec![Some(1)]]);
        let mut operator = TopNOperator::new(
            Box::new(source),
            vec![SortExpr::new(Expr::Column("missing".into()), true)],
            1,
        )
        .unwrap();
        assert!(
            operator
                .next_batch()
                .unwrap_err()
                .to_string()
                .contains("missing")
        );
    }

    #[test]
    fn merges_partition_top_n_with_multi_column_and_null_ordering() {
        let schema = Arc::new(Schema::new(vec![
            Field::new("score", DataType::Int64, true),
            Field::new("name", DataType::Utf8, false),
        ]));
        let partition_a = RecordBatch::try_new(
            schema.clone(),
            vec![
                Arc::new(Int64Array::from(vec![None, Some(9), Some(8)])),
                Arc::new(StringArray::from(vec!["null-a", "amy", "zed"])),
            ],
        )
        .unwrap();
        let partition_b = RecordBatch::try_new(
            schema.clone(),
            vec![
                Arc::new(Int64Array::from(vec![None, Some(9), Some(7)])),
                Arc::new(StringArray::from(vec!["null-b", "zoe", "bob"])),
            ],
        )
        .unwrap();
        let ordering = vec![
            SortExpr::new(Expr::Column("score".into()), false).with_nulls_first(false),
            SortExpr::new(Expr::Column("name".into()), true),
        ];

        let result = merge_top_n(&schema, &[partition_a, partition_b], &ordering, 4)
            .unwrap()
            .unwrap();
        let scores = result
            .column(0)
            .as_any()
            .downcast_ref::<Int64Array>()
            .unwrap();
        let names = result
            .column(1)
            .as_any()
            .downcast_ref::<StringArray>()
            .unwrap();
        assert_eq!(
            (0..scores.len())
                .map(|index| (!scores.is_null(index)).then(|| scores.value(index)))
                .collect::<Vec<_>>(),
            vec![Some(9), Some(9), Some(8), Some(7)]
        );
        assert_eq!(
            names.iter().collect::<Vec<_>>(),
            vec![Some("amy"), Some("zoe"), Some("zed"), Some("bob")]
        );
    }

    #[test]
    fn merge_rejects_incompatible_partition_schema() {
        let schema = Arc::new(Schema::new(vec![Field::new("id", DataType::Int64, false)]));
        let incompatible_schema =
            Arc::new(Schema::new(vec![Field::new("id", DataType::Utf8, false)]));
        let batch = RecordBatch::try_new(
            incompatible_schema,
            vec![Arc::new(StringArray::from(vec!["1"]))],
        )
        .unwrap();
        let result = merge_top_n(
            &schema,
            &[batch],
            &[SortExpr::new(Expr::Column("id".into()), true)],
            1,
        );
        assert!(result.unwrap_err().to_string().contains("incompatible"));
    }

    #[test]
    fn spill_aware_top_n_matches_in_memory_result_under_a_tiny_limit() {
        let input_values = vec![
            vec![Some(2), Some(5)],
            vec![Some(4), Some(1)],
            vec![Some(8), Some(3)],
            vec![Some(7), Some(6)],
            vec![Some(10), Some(9)],
        ];
        let memory_limit = 16 * 1024;
        let memory_pool = kaveon_core::QueryMemoryPool::new("topn-spill", memory_limit).unwrap();
        let memory = memory_pool.operator("topn").unwrap();
        let spill = SpillManager::new(std::env::temp_dir(), 64 * 1_024).unwrap();
        let spill_metrics = spill.clone();
        let mut operator = TopNOperator::new_with_spill(
            Box::new(MockOperator::new(input_values)),
            vec![SortExpr::new(Expr::Column("id".into()), false)],
            3,
            memory,
            spill,
        )
        .unwrap()
        .with_merge_fan_in(2)
        .unwrap();

        assert_eq!(
            values(&operator.next_batch().unwrap().unwrap()),
            vec![Some(10), Some(9), Some(8)]
        );
        assert!(spill_metrics.snapshot().peak_bytes > 0);
        assert!(memory_pool.snapshot().peak_bytes <= memory_limit);
    }
}
