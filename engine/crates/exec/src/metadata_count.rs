//! Exact COUNT(*) over one validated local snapshot, without decoding data pages.
use std::sync::Arc;

use arrow::{
    array::UInt64Array,
    datatypes::{DataType, Field, Schema, SchemaRef},
    record_batch::RecordBatch,
};
use kaveon_core::{
    BatchOperator, CatalogManager, DataFormat, MemoryReservation, OperatorMemoryAccount, Result,
    StorageType, TableReference,
};
use kaveon_storage::{DeltaTableReader, ParquetReader};

pub struct MetadataCount {
    schema: SchemaRef,
    batch: Option<RecordBatch>,
    memory: Option<OperatorMemoryAccount>,
    _reservation: Option<MemoryReservation>,
}

impl MetadataCount {
    /// None means the storage format/backend has no supported exact metadata path.
    /// Storage errors are never converted to approximate counts or ignored.
    pub fn try_new(
        catalog: &CatalogManager,
        table: &str,
        columns: Option<&[String]>,
        count_expressions: usize,
        memory: Option<OperatorMemoryAccount>,
    ) -> Result<Option<Self>> {
        let resolved = catalog.resolve_table(&TableReference::parse(table))?;
        if !matches!(resolved.storage, StorageType::Local { .. })
            || matches!(resolved.table.format, DataFormat::Iceberg)
        {
            return Ok(None);
        }
        if let Some(memory) = &memory {
            memory.check_cancelled()?;
        }
        // Delta metadata resolves the active file set exactly once and validates
        // its protocol and physical/logical schema before summing file footers.
        let metadata = match resolved.table.format {
            DataFormat::Parquet => ParquetReader::new(resolved.full_path()).metadata()?,
            DataFormat::Delta => DeltaTableReader::new(resolved.full_path()).metadata()?,
            DataFormat::Iceberg => unreachable!(),
        };
        if let Some(columns) = columns {
            for name in columns {
                metadata.schema.index_of(name)?;
            }
        }
        let bytes = (count_expressions as u64).checked_mul(128).ok_or_else(|| {
            kaveon_core::KaveonError::Execution("metadata COUNT output size overflow".into())
        })?;
        let reservation = memory
            .as_ref()
            .map(|memory| memory.reserve(bytes))
            .transpose()?;
        let name =
            crate::aggregate::AggExpr::new(crate::aggregate::AggFunc::Count, "*").output_name();
        let schema = Arc::new(Schema::new(
            (0..count_expressions)
                .map(|_| Field::new(&name, DataType::UInt64, true))
                .collect::<Vec<_>>(),
        ));
        let batch = RecordBatch::try_new(
            schema.clone(),
            (0..count_expressions)
                .map(|_| Arc::new(UInt64Array::from(vec![metadata.row_count])) as _)
                .collect(),
        )?;
        Ok(Some(Self {
            schema,
            batch: Some(batch),
            memory,
            _reservation: reservation,
        }))
    }
}

impl BatchOperator for MetadataCount {
    fn schema(&self) -> &SchemaRef {
        &self.schema
    }
    fn next_batch(&mut self) -> Result<Option<RecordBatch>> {
        if let Some(memory) = &self.memory {
            memory.check_cancelled()?;
        }
        if self.batch.is_none() {
            self._reservation = None;
        }
        Ok(self.batch.take())
    }
}
