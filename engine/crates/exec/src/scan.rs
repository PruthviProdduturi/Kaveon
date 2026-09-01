use arrow::datatypes::{Field, Schema, SchemaRef};
use arrow::record_batch::RecordBatch;
use kaveon_core::{BatchOperator, BatchSource, KaveonError, Result};
use std::sync::Arc;

pub struct ScanOperator {
    source: Box<dyn BatchSource>,
    output_schema: SchemaRef,
    projection: Option<Vec<usize>>,
}

impl ScanOperator {
    pub fn new(source: Box<dyn BatchSource>, columns: Option<&[String]>) -> Result<Self> {
        let source_schema = source.schema().clone();

        let (output_schema, projection) = match columns {
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
                let fields: Vec<Field> = indices
                    .iter()
                    .map(|&i| source_schema.field(i).clone())
                    .collect();
                (Arc::new(Schema::new(fields)), Some(indices))
            }
            _ => (source_schema, None),
        };

        Ok(Self {
            source,
            output_schema,
            projection,
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

        match &self.projection {
            None => Ok(Some(batch)),
            Some(indices) => {
                let projected = batch.project(indices)?;
                Ok(Some(projected))
            }
        }
    }
}
