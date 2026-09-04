use std::collections::VecDeque;

use arrow::compute::{SortColumn, SortOptions, concat_batches, lexsort_to_indices, take};
use arrow::datatypes::SchemaRef;
use arrow::record_batch::RecordBatch;
use kaveon_core::{BatchOperator, KaveonError, OperatorMemoryAccount, Result};

use crate::expr_eval::evaluate;
use crate::sort::SortExpr;
use crate::spill::SpillManager;

pub struct TopNOperator {
    source: Box<dyn BatchOperator>,
    sort_exprs: Vec<SortExpr>,
    limit: usize,
    schema: SchemaRef,
    output: VecDeque<RecordBatch>,
    initialized: bool,
    spill: Option<(OperatorMemoryAccount, SpillManager)>,
}

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
        operator.spill = Some((memory, spill));
        Ok(operator)
    }

    fn initialize(&mut self) -> Result<()> {
        if self.limit == 0 {
            return Ok(());
        }
        let mut batches = Vec::new();
        let mut reservations = Vec::new();
        let mut runs = Vec::new();
        while let Some(batch) = self.source.next_batch()? {
            if batch.num_rows() > 0 {
                let candidate = merge_top_n(&self.schema, &[batch], &self.sort_exprs, self.limit)?
                    .expect("non-empty input produces a TopN candidate");
                if let Some((memory, spill)) = &self.spill {
                    let bytes = u64::try_from(candidate.get_array_memory_size()).map_err(|_| {
                        KaveonError::Execution("TopN batch memory size exceeds u64".into())
                    })?;
                    match memory.reserve(bytes) {
                        Ok(reservation) => reservations.push(reservation),
                        Err(_) => {
                            runs.push(spill.write_run(&self.schema, &[candidate])?);
                            continue;
                        }
                    }
                }
                batches.push(candidate);
            }
        }
        if batches.is_empty() && runs.is_empty() {
            return Ok(());
        }

        for run in &runs {
            batches.extend(run.read()?);
        }

        if let Some(batch) = merge_top_n(&self.schema, &batches, &self.sort_exprs, self.limit)? {
            self.output.push_back(batch);
        }
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
    let combined = concat_batches(schema, &non_empty)?;
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
    let indices = lexsort_to_indices(&columns, Some(limit))?;
    let output_columns = combined
        .columns()
        .iter()
        .map(|column| take(column.as_ref(), &indices, None))
        .collect::<std::result::Result<Vec<_>, _>>()?;
    Ok(Some(RecordBatch::try_new(schema.clone(), output_columns)?))
}

impl BatchOperator for TopNOperator {
    fn schema(&self) -> &SchemaRef {
        &self.schema
    }

    fn next_batch(&mut self) -> Result<Option<RecordBatch>> {
        if !self.initialized {
            self.initialized = true;
            self.initialize()?;
        }
        Ok(self.output.pop_front())
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use arrow::array::{Array, Int64Array, StringArray};
    use arrow::datatypes::{DataType, Field, Schema};

    use super::*;
    use kaveon_core::Expr;

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
        let input_values = vec![vec![Some(2), Some(5)], vec![Some(4), Some(1)]];
        let probe = MockOperator::new(vec![input_values[0].clone()]);
        let per_batch_bytes = u64::try_from(probe.schema().fields().len()).unwrap();
        let memory_pool = kaveon_core::QueryMemoryPool::new("topn-spill", per_batch_bytes).unwrap();
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
        .unwrap();

        assert_eq!(
            values(&operator.next_batch().unwrap().unwrap()),
            vec![Some(5), Some(4), Some(2)]
        );
        assert!(spill_metrics.snapshot().peak_bytes > 0);
        assert!(memory_pool.snapshot().peak_bytes <= per_batch_bytes);
    }
}
