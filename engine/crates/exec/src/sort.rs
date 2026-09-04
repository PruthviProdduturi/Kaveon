use std::collections::VecDeque;

use arrow::compute::{SortColumn, SortOptions, concat_batches, lexsort_to_indices, take};
use arrow::datatypes::SchemaRef;
use arrow::record_batch::RecordBatch;
use arrow::row::{RowConverter, Rows, SortField};
use kaveon_core::{BatchOperator, Expr, KaveonError, OperatorMemoryAccount, Result};

use crate::expr_eval::evaluate;
use crate::spill::{SpillManager, SpillRun, SpillRunReader};

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
    external_merge: Option<ExternalMerge>,
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
            external_merge: None,
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
            self.external_merge = Some(ExternalMerge::new(
                runs,
                self.schema.clone(),
                &self.sort_exprs,
                self.output_batch_size,
                None,
            )?);
            return Ok(());
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
        if let Some(merge) = &mut self.external_merge {
            return merge.next_batch();
        }
        Ok(self.output.pop_front())
    }
}

struct MergeCursor {
    reader: SpillRunReader,
    batch: Option<RecordBatch>,
    rows: Option<Rows>,
    row: usize,
}

pub(crate) struct ExternalMerge {
    cursors: Vec<MergeCursor>,
    converter: RowConverter,
    schema: SchemaRef,
    sort_exprs: Vec<SortExpr>,
    output_batch_size: usize,
    remaining: Option<usize>,
    // Declared last so readers are closed before run-file cleanup on Windows.
    _runs: Vec<SpillRun>,
}

impl ExternalMerge {
    pub(crate) fn new(
        runs: Vec<SpillRun>,
        schema: SchemaRef,
        sort_exprs: &[SortExpr],
        output_batch_size: usize,
        remaining: Option<usize>,
    ) -> Result<Self> {
        let mut readers = runs
            .iter()
            .map(SpillRun::reader)
            .collect::<Result<Vec<_>>>()?;
        let mut first_batch = None;
        for reader in &mut readers {
            if let Some(batch) = reader.next().transpose()?
                && batch.num_rows() > 0
            {
                first_batch = Some(batch);
                break;
            }
        }
        let Some(probe) = first_batch else {
            return Err(KaveonError::Execution(
                "external merge requires at least one non-empty spill run".into(),
            ));
        };
        let key_columns = evaluate_sort_columns(sort_exprs, &probe)?;
        let fields = key_columns
            .iter()
            .zip(sort_exprs)
            .map(|(column, sort_expr)| {
                SortField::new_with_options(
                    column.data_type().clone(),
                    SortOptions {
                        descending: !sort_expr.ascending,
                        nulls_first: sort_expr.nulls_first,
                    },
                )
            })
            .collect();
        let converter = RowConverter::new(fields)?;
        let mut cursors = Vec::with_capacity(runs.len());
        for run in &runs {
            let mut cursor = MergeCursor {
                reader: run.reader()?,
                batch: None,
                rows: None,
                row: 0,
            };
            load_next_cursor_batch(&converter, sort_exprs, &mut cursor)?;
            cursors.push(cursor);
        }
        Ok(Self {
            cursors,
            converter,
            schema,
            sort_exprs: sort_exprs.to_vec(),
            output_batch_size,
            remaining,
            _runs: runs,
        })
    }

    pub(crate) fn next_batch(&mut self) -> Result<Option<RecordBatch>> {
        let target = self.remaining.map_or(self.output_batch_size, |remaining| {
            remaining.min(self.output_batch_size)
        });
        if target == 0 {
            return Ok(None);
        }
        let mut rows = Vec::with_capacity(target);
        while rows.len() < target {
            let Some(index) = self.best_cursor() else {
                break;
            };
            let cursor = &mut self.cursors[index];
            let batch = cursor.batch.as_ref().expect("active cursor has a batch");
            rows.push(batch.slice(cursor.row, 1));
            cursor.row += 1;
            if cursor.row == batch.num_rows() {
                load_next_cursor_batch(&self.converter, &self.sort_exprs, cursor)?;
            }
        }
        if rows.is_empty() {
            return Ok(None);
        }
        if let Some(remaining) = &mut self.remaining {
            *remaining -= rows.len();
        }
        Ok(Some(concat_batches(&self.schema, &rows)?))
    }

    fn best_cursor(&self) -> Option<usize> {
        self.cursors
            .iter()
            .enumerate()
            .filter(|(_, cursor)| cursor.batch.is_some())
            .min_by(|(_, left), (_, right)| {
                left.rows
                    .as_ref()
                    .expect("active cursor has keys")
                    .row(left.row)
                    .cmp(
                        &right
                            .rows
                            .as_ref()
                            .expect("active cursor has keys")
                            .row(right.row),
                    )
            })
            .map(|(index, _)| index)
    }
}

fn evaluate_sort_columns(
    sort_exprs: &[SortExpr],
    batch: &RecordBatch,
) -> Result<Vec<arrow::array::ArrayRef>> {
    sort_exprs
        .iter()
        .map(|sort_expr| evaluate(&sort_expr.expr, batch))
        .collect()
}

fn load_next_cursor_batch(
    converter: &RowConverter,
    sort_exprs: &[SortExpr],
    cursor: &mut MergeCursor,
) -> Result<()> {
    loop {
        let Some(batch) = cursor.reader.next().transpose()? else {
            cursor.batch = None;
            cursor.rows = None;
            return Ok(());
        };
        if batch.num_rows() == 0 {
            continue;
        }
        let columns = if sort_exprs.is_empty() {
            return Err(KaveonError::Execution(
                "external merge cannot advance without ordering expressions".into(),
            ));
        } else {
            evaluate_sort_columns(sort_exprs, &batch)?
        };
        cursor.rows = Some(converter.convert_columns(&columns)?);
        cursor.batch = Some(batch);
        cursor.row = 0;
        return Ok(());
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
