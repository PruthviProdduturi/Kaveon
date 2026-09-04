use std::collections::VecDeque;

use arrow::compute::{SortColumn, SortOptions, concat_batches, lexsort_to_indices, take};
use arrow::datatypes::SchemaRef;
use arrow::record_batch::RecordBatch;
use kaveon_core::{BatchOperator, Expr, KaveonError, OperatorMemoryAccount, Result};

use crate::expr_eval::evaluate;
use crate::spill::SpillManager;

const DEFAULT_OUTPUT_BATCH_SIZE: usize = 8_192;

#[derive(Debug, Clone)]
pub struct SortExpr {
    pub expr: Expr,
    pub ascending: bool,
    pub nulls_first: bool,
}

impl SortExpr {
    pub fn new(expr: Expr, ascending: bool) -> Self {
        Self {
            expr,
            ascending,
            nulls_first: !ascending,
        }
    }

    pub fn with_nulls_first(mut self, nulls_first: bool) -> Self {
        self.nulls_first = nulls_first;
        self
    }
}

pub struct SortOperator {
    source: Box<dyn BatchOperator>,
    sort_exprs: Vec<SortExpr>,
    schema: SchemaRef,
    output_batch_size: usize,
    output: VecDeque<RecordBatch>,
    initialized: bool,
    spill: Option<(OperatorMemoryAccount, SpillManager)>,
}

impl SortOperator {
    pub fn new(source: Box<dyn BatchOperator>, sort_exprs: Vec<SortExpr>) -> Result<Self> {
        if sort_exprs.is_empty() {
            return Err(KaveonError::Execution(
                "sort requires at least one ordering expression".into(),
            ));
        }
        let schema = source.schema().clone();
        Ok(Self {
            source,
            sort_exprs,
            schema,
            output_batch_size: DEFAULT_OUTPUT_BATCH_SIZE,
            output: VecDeque::new(),
            initialized: false,
            spill: None,
        })
    }

    pub fn new_with_spill(
        source: Box<dyn BatchOperator>,
        sort_exprs: Vec<SortExpr>,
        memory: OperatorMemoryAccount,
        spill: SpillManager,
    ) -> Result<Self> {
        let mut operator = Self::new(source, sort_exprs)?;
        operator.spill = Some((memory, spill));
        Ok(operator)
    }

    pub fn with_output_batch_size(mut self, output_batch_size: usize) -> Result<Self> {
        if output_batch_size == 0 {
            return Err(KaveonError::Execution(
                "sort output batch size must be greater than zero".into(),
            ));
        }
        self.output_batch_size = output_batch_size;
        Ok(self)
    }

    fn initialize(&mut self) -> Result<()> {
        let mut batches = Vec::new();
        let mut reservations = Vec::new();
        let mut runs = Vec::new();
        while let Some(batch) = self.source.next_batch()? {
            if batch.num_rows() > 0 {
                if let Some((memory, spill)) = &self.spill {
                    let bytes = u64::try_from(batch.get_array_memory_size()).map_err(|_| {
                        KaveonError::Execution("sort batch memory size exceeds u64".into())
                    })?;
                    match memory.reserve(bytes) {
                        Ok(reservation) => reservations.push(reservation),
                        Err(_) if !batches.is_empty() => {
                            let sorted = sort_batches(&self.schema, &batches, &self.sort_exprs)?;
                            runs.push(spill.write_run(&self.schema, &[sorted])?);
                            batches.clear();
                            reservations.clear();
                            match memory.reserve(bytes) {
                                Ok(reservation) => reservations.push(reservation),
                                Err(_) => {
                                    let sorted = sort_batches(
                                        &self.schema,
                                        std::slice::from_ref(&batch),
                                        &self.sort_exprs,
                                    )?;
                                    runs.push(spill.write_run(&self.schema, &[sorted])?);
                                    continue;
                                }
                            }
                        }
                        Err(_) => {
                            let sorted = sort_batches(
                                &self.schema,
                                std::slice::from_ref(&batch),
                                &self.sort_exprs,
                            )?;
                            runs.push(spill.write_run(&self.schema, &[sorted])?);
                            continue;
                        }
                    }
                }
                batches.push(batch);
            }
        }
        if batches.is_empty() && runs.is_empty() {
            return Ok(());
        }

        if !runs.is_empty() {
            if !batches.is_empty() {
                let sorted = sort_batches(&self.schema, &batches, &self.sort_exprs)?;
                let spill = &self.spill.as_ref().expect("spill mode is active").1;
                runs.push(spill.write_run(&self.schema, &[sorted])?);
            }
            batches.clear();
            reservations.clear();
            for run in &runs {
                batches.extend(run.read()?);
            }
        }

        let sorted = sort_batches(&self.schema, &batches, &self.sort_exprs)?;

        for offset in (0..sorted.num_rows()).step_by(self.output_batch_size) {
            let length = self.output_batch_size.min(sorted.num_rows() - offset);
            self.output.push_back(sorted.slice(offset, length));
        }
        Ok(())
    }
}

fn sort_batches(
    schema: &SchemaRef,
    batches: &[RecordBatch],
    sort_exprs: &[SortExpr],
) -> Result<RecordBatch> {
    let combined = concat_batches(schema, batches)?;
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
    let indices = lexsort_to_indices(&columns, None)?;
    let sorted_columns = combined
        .columns()
        .iter()
        .map(|column| take(column.as_ref(), &indices, None))
        .collect::<std::result::Result<Vec<_>, _>>()?;
    Ok(RecordBatch::try_new(schema.clone(), sorted_columns)?)
}

impl BatchOperator for SortOperator {
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
    use kaveon_core::predicate::ScalarValue;
    use kaveon_core::{BinaryOp, collect_batches};

    use super::*;

    struct MockOperator {
        schema: SchemaRef,
        batches: VecDeque<RecordBatch>,
    }

    impl MockOperator {
        fn new(batches: Vec<RecordBatch>) -> Self {
            let schema = batches
                .first()
                .map_or_else(|| Arc::new(Schema::empty()), RecordBatch::schema);
            Self {
                schema,
                batches: batches.into(),
            }
        }

        fn with_schema(schema: SchemaRef, batches: Vec<RecordBatch>) -> Self {
            Self {
                schema,
                batches: batches.into(),
            }
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

    fn batch(ids: Vec<Option<i64>>, names: Vec<&str>) -> RecordBatch {
        let schema = Arc::new(Schema::new(vec![
            Field::new("id", DataType::Int64, true),
            Field::new("name", DataType::Utf8, false),
        ]));
        RecordBatch::try_new(
            schema,
            vec![
                Arc::new(Int64Array::from(ids)),
                Arc::new(StringArray::from(names)),
            ],
        )
        .expect("valid batch")
    }

    fn ids(batches: &[RecordBatch]) -> Vec<Option<i64>> {
        batches
            .iter()
            .flat_map(|batch| {
                let array = batch
                    .column(0)
                    .as_any()
                    .downcast_ref::<Int64Array>()
                    .unwrap();
                (0..array.len()).map(|index| {
                    if array.is_null(index) {
                        None
                    } else {
                        Some(array.value(index))
                    }
                })
            })
            .collect()
    }

    #[test]
    fn sorts_multiple_batches_ascending_and_chunks_output() {
        let source = MockOperator::new(vec![
            batch(vec![Some(4), Some(1)], vec!["d", "a"]),
            batch(vec![Some(3), Some(2)], vec!["c", "b"]),
        ]);
        let mut operator = SortOperator::new(
            Box::new(source),
            vec![SortExpr::new(Expr::Column("id".into()), true)],
        )
        .unwrap()
        .with_output_batch_size(2)
        .unwrap();

        let result = collect_batches(&mut operator).unwrap();
        assert_eq!(result.len(), 2);
        assert_eq!(ids(&result), vec![Some(1), Some(2), Some(3), Some(4)]);
    }

    #[test]
    fn sorts_descending_with_explicit_null_ordering() {
        let source = MockOperator::new(vec![batch(
            vec![Some(2), None, Some(1)],
            vec!["b", "null", "a"],
        )]);
        let mut operator = SortOperator::new(
            Box::new(source),
            vec![SortExpr::new(Expr::Column("id".into()), false).with_nulls_first(false)],
        )
        .unwrap();

        assert_eq!(
            ids(&collect_batches(&mut operator).unwrap()),
            vec![Some(2), Some(1), None]
        );
    }

    #[test]
    fn applies_lexicographic_multi_column_ordering() {
        let source = MockOperator::new(vec![batch(
            vec![Some(1), Some(2), Some(1)],
            vec!["a", "z", "z"],
        )]);
        let mut operator = SortOperator::new(
            Box::new(source),
            vec![
                SortExpr::new(Expr::Column("id".into()), true),
                SortExpr::new(Expr::Column("name".into()), false),
            ],
        )
        .unwrap();
        let result = collect_batches(&mut operator).unwrap();
        let names = result[0]
            .column(1)
            .as_any()
            .downcast_ref::<StringArray>()
            .unwrap();
        assert_eq!(ids(&result), vec![Some(1), Some(1), Some(2)]);
        assert_eq!(
            names.iter().collect::<Vec<_>>(),
            vec![Some("z"), Some("a"), Some("z")]
        );
    }

    #[test]
    fn empty_input_produces_no_batches() {
        let schema = Arc::new(Schema::new(vec![Field::new("id", DataType::Int64, true)]));
        let source = MockOperator::with_schema(schema, vec![]);
        let mut operator = SortOperator::new(
            Box::new(source),
            vec![SortExpr::new(Expr::Column("id".into()), true)],
        )
        .unwrap();
        assert!(operator.next_batch().unwrap().is_none());
    }

    #[test]
    fn rejects_empty_ordering_and_zero_batch_size() {
        let source = MockOperator::new(vec![batch(vec![Some(1)], vec!["a"])]);
        assert!(SortOperator::new(Box::new(source), vec![]).is_err());

        let source = MockOperator::new(vec![batch(vec![Some(1)], vec!["a"])]);
        assert!(
            SortOperator::new(
                Box::new(source),
                vec![SortExpr::new(Expr::Column("id".into()), true)]
            )
            .unwrap()
            .with_output_batch_size(0)
            .is_err()
        );
    }

    #[test]
    fn reports_unknown_and_invalid_sort_expressions() {
        let source = MockOperator::new(vec![batch(vec![Some(1)], vec!["a"])]);
        let mut unknown = SortOperator::new(
            Box::new(source),
            vec![SortExpr::new(Expr::Column("missing".into()), true)],
        )
        .unwrap();
        assert!(
            unknown
                .next_batch()
                .unwrap_err()
                .to_string()
                .contains("missing")
        );

        let source = MockOperator::new(vec![batch(vec![Some(1)], vec!["a"])]);
        let mut invalid =
            SortOperator::new(Box::new(source), vec![SortExpr::new(Expr::Star, true)]).unwrap();
        assert!(
            invalid
                .next_batch()
                .unwrap_err()
                .to_string()
                .contains("cannot be evaluated")
        );
    }

    #[test]
    fn reports_invalid_sort_expression_types() {
        let source = MockOperator::new(vec![batch(vec![Some(1)], vec!["a"])]);
        let mut operator = SortOperator::new(
            Box::new(source),
            vec![SortExpr::new(
                Expr::BinaryOp {
                    left: Box::new(Expr::Column("name".into())),
                    op: BinaryOp::Plus,
                    right: Box::new(Expr::Literal(ScalarValue::Utf8("suffix".into()))),
                },
                true,
            )],
        )
        .unwrap();
        assert!(
            operator
                .next_batch()
                .unwrap_err()
                .to_string()
                .contains("arithmetic not supported")
        );
    }

    #[test]
    fn spill_aware_sort_preserves_order_when_memory_forces_runs() {
        let first = batch(vec![Some(4), Some(1)], vec!["d", "a"]);
        let per_batch_bytes = u64::try_from(first.get_array_memory_size()).unwrap();
        let source = MockOperator::new(vec![first, batch(vec![Some(3), Some(2)], vec!["c", "b"])]);
        let memory_pool = kaveon_core::QueryMemoryPool::new("sort-spill", per_batch_bytes).unwrap();
        let memory = memory_pool.operator("sort").unwrap();
        let spill = SpillManager::new(std::env::temp_dir(), 64 * 1_024).unwrap();
        let spill_metrics = spill.clone();
        let mut operator = SortOperator::new_with_spill(
            Box::new(source),
            vec![SortExpr::new(Expr::Column("id".into()), true)],
            memory,
            spill,
        )
        .unwrap();

        assert_eq!(
            ids(&collect_batches(&mut operator).unwrap()),
            vec![Some(1), Some(2), Some(3), Some(4)]
        );
        assert!(spill_metrics.snapshot().peak_bytes > 0);
        assert!(memory_pool.snapshot().peak_bytes <= per_batch_bytes);
    }
}
