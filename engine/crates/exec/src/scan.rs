use arrow::array::ArrayRef;
use arrow::compute;
use arrow::datatypes::{DataType, Field, Schema, SchemaRef};
use arrow::record_batch::RecordBatch;
use kaveon_core::{BatchOperator, BatchSource, KaveonError, Result};
use std::sync::Arc;

pub struct ScanOperator {
    source: Box<dyn BatchSource>,
    output_schema: SchemaRef,
    projection: Option<Vec<usize>>,
    /// Output positions whose source type is narrower than the type the
    /// scan presents (see `widened`).
    widen: Vec<usize>,
}

/// The type a scan presents for a stored column. Narrow integers (Parquet
/// `INT(8)`, `INT(16)`, unsigned widths) are read as the widths every
/// operator computes with, so a SUM over a `smallint` column is an ordinary
/// integer aggregate rather than a type the executor has to special-case.
pub fn widened(data_type: &DataType) -> DataType {
    match data_type {
        DataType::Int8 | DataType::Int16 | DataType::UInt8 | DataType::UInt16 => DataType::Int32,
        DataType::UInt32 => DataType::Int64,
        other => other.clone(),
    }
}

impl ScanOperator {
    pub fn new(source: Box<dyn BatchSource>, columns: Option<&[String]>) -> Result<Self> {
        let source_schema = source.schema().clone();

        let (fields, projection): (Vec<Field>, Option<Vec<usize>>) = match columns {
            Some(cols) if !cols.is_empty() => {
                let indices: Vec<usize> = cols
                    .iter()
                    .map(|col| {
                        source_schema.index_of(col).map_err(|_| {
                            KaveonError::Execution(format!(
                                "scan projection references unknown column '{col}'"
                            ))
                        })
                    })
                    .collect::<Result<_>>()?;
                let fields = indices
                    .iter()
                    .map(|&i| source_schema.field(i).clone())
                    .collect();
                (fields, Some(indices))
            }
            _ => (
                source_schema
                    .fields()
                    .iter()
                    .map(|field| field.as_ref().clone())
                    .collect(),
                None,
            ),
        };
        let mut widen = Vec::new();
        let fields: Vec<Field> = fields
            .into_iter()
            .enumerate()
            .map(|(position, field)| {
                let presented = widened(field.data_type());
                if &presented != field.data_type() {
                    widen.push(position);
                    field.with_data_type(presented)
                } else {
                    field
                }
            })
            .collect();

        Ok(Self {
            source,
            output_schema: Arc::new(Schema::new(fields)),
            projection,
            widen,
        })
    }
}

impl BatchOperator for ScanOperator {
    fn schema(&self) -> &SchemaRef {
        &self.output_schema
    }

    fn next_batch(&mut self) -> Result<Option<RecordBatch>> {
        let Some(batch) = self.source.next_batch()? else {
            return Ok(None);
        };

        let batch = match &self.projection {
            None => batch,
            Some(indices) => batch.project(indices)?,
        };
        // The batch goes out under the declared schema. A source's batches
        // may carry schema metadata the declaration does not — a Parquet
        // footer's key-value entries (a Spark or pandas schema, this
        // Engine's own layout entry) come back as Arrow schema metadata —
        // and the operators downstream compare schemas whole.
        if self.widen.is_empty() && batch.schema() == self.output_schema {
            return Ok(Some(batch));
        }
        let mut columns: Vec<ArrayRef> = batch.columns().to_vec();
        for &position in &self.widen {
            columns[position] = compute::cast(
                &columns[position],
                self.output_schema.field(position).data_type(),
            )?;
        }
        Ok(Some(RecordBatch::try_new_with_options(
            Arc::clone(&self.output_schema),
            columns,
            &arrow::record_batch::RecordBatchOptions::new().with_row_count(Some(batch.num_rows())),
        )?))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use arrow::array::{AsArray, Int16Array, Int64Array, UInt32Array};
    use arrow::datatypes::Int32Type;

    struct Source {
        schema: SchemaRef,
        batch: Option<RecordBatch>,
    }

    impl BatchSource for Source {
        fn schema(&self) -> &SchemaRef {
            &self.schema
        }
        fn next_batch(&mut self) -> Result<Option<RecordBatch>> {
            Ok(self.batch.take())
        }
    }

    #[test]
    fn narrow_integers_are_presented_at_computing_widths() {
        let schema = Arc::new(Schema::new(vec![
            Field::new("small", DataType::Int16, true),
            Field::new("wide", DataType::Int64, true),
            Field::new("unsigned", DataType::UInt32, true),
        ]));
        let batch = RecordBatch::try_new(
            Arc::clone(&schema),
            vec![
                Arc::new(Int16Array::from(vec![Some(-3), None, Some(7)])),
                Arc::new(Int64Array::from(vec![1, 2, 3])),
                Arc::new(UInt32Array::from(vec![u32::MAX, 0, 1])),
            ],
        )
        .unwrap();
        let mut scan = ScanOperator::new(
            Box::new(Source {
                schema,
                batch: Some(batch),
            }),
            Some(&["unsigned".to_owned(), "small".to_owned()]),
        )
        .unwrap();
        assert_eq!(
            scan.schema()
                .fields()
                .iter()
                .map(|field| field.data_type().clone())
                .collect::<Vec<_>>(),
            vec![DataType::Int64, DataType::Int32]
        );
        let output = scan.next_batch().unwrap().unwrap();
        assert_eq!(output.schema(), *scan.schema());
        let small = output.column(1).as_primitive::<Int32Type>();
        assert_eq!(
            small.iter().collect::<Vec<_>>(),
            vec![Some(-3), None, Some(7)]
        );
        let unsigned = output
            .column(0)
            .as_primitive::<arrow::datatypes::Int64Type>();
        assert_eq!(unsigned.value(0), u32::MAX as i64);
    }

    /// A source whose schema carries metadata (a Parquet footer's key-value
    /// entries) hands out batches under the operator's declared schema,
    /// which carries none.
    #[test]
    fn schema_metadata_of_the_source_is_not_presented() {
        let schema = Arc::new(Schema::new_with_metadata(
            vec![Field::new("wide", DataType::Int64, true)],
            std::collections::HashMap::from([(
                "kaveon.layout.clustered_by".to_owned(),
                "[\"wide\"]".to_owned(),
            )]),
        ));
        let batch = RecordBatch::try_new(
            Arc::clone(&schema),
            vec![Arc::new(Int64Array::from(vec![1, 2, 3]))],
        )
        .unwrap();
        let mut scan = ScanOperator::new(
            Box::new(Source {
                schema,
                batch: Some(batch),
            }),
            None,
        )
        .unwrap();
        assert!(scan.schema().metadata().is_empty());
        let output = scan.next_batch().unwrap().unwrap();
        assert_eq!(output.schema(), *scan.schema());
        assert_eq!(output.num_rows(), 3);
    }
}
