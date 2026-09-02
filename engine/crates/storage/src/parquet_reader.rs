use arrow::datatypes::{DataType, SchemaRef};
use arrow::record_batch::{RecordBatch, RecordBatchReader as ArrowRecordBatchReader};
use kaveon_core::{BatchSource, CompareOp, KaveonError, Result, ScalarValue, StoragePredicate};
use parquet::arrow::ProjectionMask;
use parquet::arrow::arrow_reader::{ParquetRecordBatchReader, ParquetRecordBatchReaderBuilder};
use parquet::file::metadata::{ParquetMetaData, RowGroupMetaData};
use parquet::file::statistics::Statistics;
use std::cmp::Ordering;
use std::collections::HashSet;
use std::fs::File;
use std::path::{Path, PathBuf};
use std::sync::Arc;

const DEFAULT_BATCH_SIZE: usize = 8_192;

#[derive(Clone, Debug)]
pub struct ParquetFileMetadata {
    pub schema: SchemaRef,
    pub row_count: u64,
    pub row_group_count: usize,
}

/// Streaming adapter over parquet-rs that implements the shared execution
/// contract while also remaining usable as a standard iterator.
pub struct ParquetBatchIterator {
    schema: SchemaRef,
    inner: ParquetRecordBatchReader,
}

impl Iterator for ParquetBatchIterator {
    type Item = Result<RecordBatch>;

    fn next(&mut self) -> Option<Self::Item> {
        self.inner
            .next()
            .map(|result| result.map_err(|error| storage_error(error.to_string())))
    }
}

impl BatchSource for ParquetBatchIterator {
    fn schema(&self) -> &SchemaRef {
        &self.schema
    }

    fn next_batch(&mut self) -> Result<Option<RecordBatch>> {
        self.next().transpose()
    }
}

/// Configuration for a synchronous local Parquet read.
pub struct ParquetReader {
    path: PathBuf,
    batch_size: usize,
    columns: Option<Vec<String>>,
    predicate: Option<StoragePredicate>,
}

impl ParquetReader {
    pub fn new(path: impl AsRef<Path>) -> Self {
        Self {
            path: path.as_ref().to_path_buf(),
            batch_size: DEFAULT_BATCH_SIZE,
            columns: None,
            predicate: None,
        }
    }

    pub fn with_batch_size(mut self, size: usize) -> Self {
        self.batch_size = size;
        self
    }

    pub fn with_columns(mut self, columns: Vec<String>) -> Self {
        self.columns = Some(columns);
        self
    }

    pub fn with_predicate(mut self, predicate: StoragePredicate) -> Self {
        self.predicate = Some(match self.predicate.take() {
            Some(existing) => StoragePredicate::And(vec![existing, predicate]),
            None => predicate,
        });
        self
    }

    pub fn read(&self) -> Result<ParquetBatchIterator> {
        let builder = self.configure_builder(self.open_builder()?)?;
        let inner = builder.build().map_err(parquet_error)?;
        let schema = inner.schema();
        Ok(ParquetBatchIterator { schema, inner })
    }

    /// Convenience method for callers that explicitly want materialization.
    pub fn read_batches(&self) -> Result<Vec<RecordBatch>> {
        self.read()?.collect()
    }

    pub fn metadata(&self) -> Result<ParquetFileMetadata> {
        let builder = self.open_builder()?;
        let row_count = u64::try_from(builder.metadata().file_metadata().num_rows())
            .map_err(|_| storage_error("Parquet metadata contains a negative row count"))?;
        Ok(ParquetFileMetadata {
            schema: Arc::clone(builder.schema()),
            row_count,
            row_group_count: builder.metadata().num_row_groups(),
        })
    }

    fn open_builder(&self) -> Result<ParquetRecordBatchReaderBuilder<File>> {
        if self.batch_size == 0 {
            return Err(storage_error("batch size must be greater than zero"));
        }
        ParquetRecordBatchReaderBuilder::try_new(File::open(&self.path)?).map_err(parquet_error)
    }

    fn configure_builder(
        &self,
        mut builder: ParquetRecordBatchReaderBuilder<File>,
    ) -> Result<ParquetRecordBatchReaderBuilder<File>> {
        let schema = Arc::clone(builder.schema());
        builder = builder.with_batch_size(self.batch_size);

        if let Some(columns) = &self.columns {
            let projection = projection_indices(&schema, columns)?;
            let mask = ProjectionMask::roots(builder.parquet_schema(), projection);
            builder = builder.with_projection(mask);
        }

        if let Some(predicate) = &self.predicate {
            validate_predicate(predicate, &schema)?;
            let groups = matching_row_groups(builder.metadata().as_ref(), &schema, predicate);
            builder = builder.with_row_groups(groups);
        }
        Ok(builder)
    }
}

fn projection_indices(schema: &SchemaRef, columns: &[String]) -> Result<Vec<usize>> {
    if columns.is_empty() {
        return Err(storage_error("projection must contain at least one column"));
    }
    let mut seen = HashSet::with_capacity(columns.len());
    columns
        .iter()
        .map(|column| {
            if !seen.insert(column.as_str()) {
                return Err(storage_error(format!(
                    "projection contains duplicate column '{column}'"
                )));
            }
            schema.index_of(column).map_err(|_| {
                storage_error(format!("projection references unknown column '{column}'"))
            })
        })
        .collect()
}

fn validate_predicate(predicate: &StoragePredicate, schema: &SchemaRef) -> Result<()> {
    match predicate {
        StoragePredicate::Compare { column, value, .. } => {
            validate_column_value(column, value, schema)
        }
        StoragePredicate::IsNull { column } | StoragePredicate::IsNotNull { column } => {
            validate_column(column, schema).map(|_| ())
        }
        StoragePredicate::In { column, values } => {
            validate_column(column, schema)?;
            for value in values {
                validate_column_value(column, value, schema)?;
            }
            Ok(())
        }
        StoragePredicate::And(predicates) | StoragePredicate::Or(predicates) => {
            for child in predicates {
                validate_predicate(child, schema)?;
            }
            Ok(())
        }
        StoragePredicate::Not(predicate) => validate_predicate(predicate, schema),
    }
}

fn validate_column<'a>(column: &str, schema: &'a SchemaRef) -> Result<&'a DataType> {
    schema
        .field_with_name(column)
        .map(|field| field.data_type())
        .map_err(|_| storage_error(format!("predicate references unknown column '{column}'")))
}

fn validate_column_value(column: &str, value: &ScalarValue, schema: &SchemaRef) -> Result<()> {
    let data_type = validate_column(column, schema)?;
    if matches!(value, ScalarValue::Null) {
        return Err(storage_error(format!(
            "comparison predicate for column '{column}' cannot use NULL"
        )));
    }
    if !scalar_matches_data_type(value, data_type) {
        return Err(storage_error(format!(
            "predicate value type does not match column '{column}' ({data_type})"
        )));
    }
    Ok(())
}

fn matching_row_groups(
    metadata: &ParquetMetaData,
    schema: &SchemaRef,
    predicate: &StoragePredicate,
) -> Vec<usize> {
    metadata
        .row_groups()
        .iter()
        .enumerate()
        .filter_map(|(index, group)| predicate_can_match(group, schema, predicate).then_some(index))
        .collect()
}

fn predicate_can_match(
    group: &RowGroupMetaData,
    schema: &SchemaRef,
    predicate: &StoragePredicate,
) -> bool {
    match predicate {
        StoragePredicate::Compare { column, op, value } => schema
            .index_of(column)
            .ok()
            .is_none_or(|index| compare_can_match(group, index, *op, value)),
        StoragePredicate::IsNull { column } => column_statistics(group, schema, column)
            .is_none_or(|stats| stats.null_count_opt() != Some(0)),
        StoragePredicate::IsNotNull { column } => column_statistics(group, schema, column)
            .is_none_or(|stats| stats.null_count_opt() != Some(group.num_rows() as u64)),
        StoragePredicate::In { column, values } => {
            schema.index_of(column).ok().is_none_or(|index| {
                values
                    .iter()
                    .any(|value| compare_can_match(group, index, CompareOp::Eq, value))
            })
        }
        StoragePredicate::And(predicates) => predicates
            .iter()
            .all(|child| predicate_can_match(group, schema, child)),
        StoragePredicate::Or(predicates) => predicates
            .iter()
            .any(|child| predicate_can_match(group, schema, child)),
        // A "may match" result cannot be safely inverted. Retaining the row
        // group preserves correctness until exact domain reasoning is added.
        StoragePredicate::Not(_) => true,
    }
}

fn column_statistics<'a>(
    group: &'a RowGroupMetaData,
    schema: &SchemaRef,
    column: &str,
) -> Option<&'a Statistics> {
    schema
        .index_of(column)
        .ok()
        .and_then(|index| group.column(index).statistics())
}

fn compare_can_match(
    group: &RowGroupMetaData,
    column: usize,
    op: CompareOp,
    value: &ScalarValue,
) -> bool {
    let Some(stats) = group.column(column).statistics() else {
        return true;
    };
    if !stats.min_is_exact() || !stats.max_is_exact() {
        return true;
    }
    match stats {
        Statistics::Boolean(stats) => bounds_can_match(
            stats.min_opt().copied().map(ScalarValue::Bool),
            stats.max_opt().copied().map(ScalarValue::Bool),
            op,
            value,
        ),
        Statistics::Int32(stats) => bounds_can_match(
            stats
                .min_opt()
                .map(|value| ScalarValue::Int64(i64::from(*value))),
            stats
                .max_opt()
                .map(|value| ScalarValue::Int64(i64::from(*value))),
            op,
            value,
        ),
        Statistics::Int64(stats) => bounds_can_match(
            stats.min_opt().copied().map(ScalarValue::Int64),
            stats.max_opt().copied().map(ScalarValue::Int64),
            op,
            value,
        ),
        Statistics::Float(stats) => bounds_can_match(
            stats
                .min_opt()
                .map(|value| ScalarValue::Float64(f64::from(*value))),
            stats
                .max_opt()
                .map(|value| ScalarValue::Float64(f64::from(*value))),
            op,
            value,
        ),
        Statistics::Double(stats) => bounds_can_match(
            stats.min_opt().copied().map(ScalarValue::Float64),
            stats.max_opt().copied().map(ScalarValue::Float64),
            op,
            value,
        ),
        Statistics::ByteArray(stats) => {
            let min = stats
                .min_opt()
                .and_then(|value| std::str::from_utf8(value.data()).ok())
                .map(|value| ScalarValue::Utf8(value.to_owned()));
            let max = stats
                .max_opt()
                .and_then(|value| std::str::from_utf8(value.data()).ok())
                .map(|value| ScalarValue::Utf8(value.to_owned()));
            bounds_can_match(min, max, op, value)
        }
        Statistics::Int96(_) | Statistics::FixedLenByteArray(_) => true,
    }
}

fn bounds_can_match(
    min: Option<ScalarValue>,
    max: Option<ScalarValue>,
    op: CompareOp,
    value: &ScalarValue,
) -> bool {
    let (Some(min), Some(max)) = (min, max) else {
        return true;
    };
    match op {
        CompareOp::Eq => within_bounds(&min, &max, value),
        CompareOp::Ne => {
            !(scalar_cmp(&min, value) == Some(Ordering::Equal)
                && scalar_cmp(&max, value) == Some(Ordering::Equal))
        }
        CompareOp::Lt => scalar_cmp(&min, value) == Some(Ordering::Less),
        CompareOp::Le => scalar_cmp(&min, value) != Some(Ordering::Greater),
        CompareOp::Gt => scalar_cmp(&max, value) == Some(Ordering::Greater),
        CompareOp::Ge => scalar_cmp(&max, value) != Some(Ordering::Less),
    }
}

fn within_bounds(min: &ScalarValue, max: &ScalarValue, value: &ScalarValue) -> bool {
    scalar_cmp(min, value) != Some(Ordering::Greater)
        && scalar_cmp(max, value) != Some(Ordering::Less)
}

fn scalar_matches_data_type(value: &ScalarValue, data_type: &DataType) -> bool {
    matches!(
        (value, data_type),
        (ScalarValue::Bool(_), DataType::Boolean)
            | (
                ScalarValue::Int64(_),
                DataType::Int8 | DataType::Int16 | DataType::Int32 | DataType::Int64
            )
            | (
                ScalarValue::Float64(_),
                DataType::Float32 | DataType::Float64
            )
            | (ScalarValue::Utf8(_), DataType::Utf8 | DataType::LargeUtf8)
    )
}

fn scalar_cmp(left: &ScalarValue, right: &ScalarValue) -> Option<Ordering> {
    match (left, right) {
        (ScalarValue::Bool(left), ScalarValue::Bool(right)) => left.partial_cmp(right),
        (ScalarValue::Int64(left), ScalarValue::Int64(right)) => left.partial_cmp(right),
        (ScalarValue::Float64(left), ScalarValue::Float64(right)) => left.partial_cmp(right),
        (ScalarValue::Utf8(left), ScalarValue::Utf8(right)) => left.partial_cmp(right),
        _ => None,
    }
}

fn storage_error(message: impl Into<String>) -> KaveonError {
    KaveonError::Storage(message.into())
}

fn parquet_error(error: parquet::errors::ParquetError) -> KaveonError {
    storage_error(error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use arrow::array::{ArrayRef, Int32Array, StringArray};
    use arrow::datatypes::{Field, Schema};
    use parquet::arrow::ArrowWriter;
    use parquet::file::properties::WriterProperties;
    use std::sync::atomic::{AtomicU64, Ordering as AtomicOrdering};

    const ROW_GROUP_SIZE: usize = 3;
    static NEXT_FILE_ID: AtomicU64 = AtomicU64::new(0);

    struct TestFile(PathBuf);

    impl Drop for TestFile {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.0);
        }
    }

    fn fixture() -> TestFile {
        let id = NEXT_FILE_ID.fetch_add(1, AtomicOrdering::Relaxed);
        let file = TestFile(std::env::temp_dir().join(format!(
            "kaveon-storage-{}-{id}.parquet",
            std::process::id()
        )));
        let schema = Arc::new(Schema::new(vec![
            Field::new("id", DataType::Int32, false),
            Field::new("label", DataType::Utf8, true),
        ]));
        let batch = RecordBatch::try_new(
            Arc::clone(&schema),
            vec![
                Arc::new(Int32Array::from(vec![0, 1, 2, 3, 4, 5])) as ArrayRef,
                Arc::new(StringArray::from(vec![
                    Some("a"),
                    Some("b"),
                    None,
                    Some("d"),
                    Some("e"),
                    Some("f"),
                ])) as ArrayRef,
            ],
        )
        .expect("test batch must be valid");
        let properties = WriterProperties::builder()
            .set_max_row_group_size(ROW_GROUP_SIZE)
            .build();
        let output = File::create(&file.0).expect("test file must be creatable");
        let mut writer = ArrowWriter::try_new(output, schema, Some(properties))
            .expect("test writer must be valid");
        writer.write(&batch).expect("test batch must be writable");
        writer.close().expect("test writer must close");
        file
    }

    fn compare(column: &str, op: CompareOp, value: ScalarValue) -> StoragePredicate {
        StoragePredicate::Compare {
            column: column.to_owned(),
            op,
            value,
        }
    }

    fn row_count(reader: &ParquetReader) -> usize {
        reader
            .read()
            .expect("reader must open")
            .map(|batch| batch.expect("batch must decode").num_rows())
            .sum()
    }

    #[test]
    fn reports_file_metadata() {
        let file = fixture();
        let metadata = ParquetReader::new(&file.0)
            .metadata()
            .expect("metadata must load");
        assert_eq!(metadata.row_count, 6);
        assert_eq!(metadata.row_group_count, 2);
        assert_eq!(metadata.schema.fields().len(), 2);
    }

    #[test]
    fn streams_batches_through_shared_contract() {
        let file = fixture();
        let mut source = ParquetReader::new(&file.0)
            .with_batch_size(2)
            .read()
            .expect("reader must open");
        assert_eq!(source.schema().fields().len(), 2);
        let mut rows = 0;
        while let Some(batch) = source.next_batch().expect("batch must decode") {
            assert!(batch.num_rows() <= 2);
            rows += batch.num_rows();
        }
        assert_eq!(rows, 6);
    }

    #[test]
    fn projects_columns_and_rejects_invalid_projection() {
        let file = fixture();
        let mut source = ParquetReader::new(&file.0)
            .with_columns(vec!["label".to_owned()])
            .read()
            .expect("projection must read");
        assert_eq!(source.schema().fields().len(), 1);
        assert_eq!(source.schema().field(0).name(), "label");
        let batches = source
            .by_ref()
            .collect::<Result<Vec<_>>>()
            .expect("projection must decode");
        assert!(batches.iter().all(|batch| batch.num_columns() == 1));
        assert_eq!(batches[0].schema().field(0).name(), "label");

        for columns in [
            vec!["missing".to_owned()],
            vec!["id".to_owned(), "id".to_owned()],
            Vec::new(),
        ] {
            assert!(
                ParquetReader::new(&file.0)
                    .with_columns(columns)
                    .read()
                    .is_err()
            );
        }
    }

    #[test]
    fn validates_configuration_and_predicates() {
        let file = fixture();
        assert!(
            ParquetReader::new(&file.0)
                .with_batch_size(0)
                .read()
                .is_err()
        );
        assert!(
            ParquetReader::new(&file.0)
                .with_predicate(StoragePredicate::IsNull {
                    column: "missing".to_owned()
                })
                .read()
                .is_err()
        );
        assert!(
            ParquetReader::new(&file.0)
                .with_predicate(compare(
                    "id",
                    CompareOp::Eq,
                    ScalarValue::Utf8("1".to_owned())
                ))
                .read()
                .is_err()
        );
        assert!(
            ParquetReader::new(&file.0)
                .with_predicate(compare("id", CompareOp::Eq, ScalarValue::Null))
                .read()
                .is_err()
        );
    }

    #[test]
    fn reports_missing_and_corrupt_files() {
        let missing = std::env::temp_dir().join("kaveon-storage-file-does-not-exist.parquet");
        assert!(matches!(
            ParquetReader::new(missing).read(),
            Err(KaveonError::Io(_))
        ));

        let corrupt = TestFile(std::env::temp_dir().join(format!(
            "kaveon-storage-corrupt-{}.parquet",
            std::process::id()
        )));
        std::fs::write(&corrupt.0, b"not a parquet file").expect("corrupt fixture must write");
        assert!(matches!(
            ParquetReader::new(&corrupt.0).read(),
            Err(KaveonError::Storage(_))
        ));
    }

    #[test]
    fn prunes_row_groups_without_filtering_decoded_rows() {
        let file = fixture();
        let matching = ParquetReader::new(&file.0).with_predicate(compare(
            "id",
            CompareOp::Eq,
            ScalarValue::Int64(4),
        ));
        assert_eq!(row_count(&matching), ROW_GROUP_SIZE);

        let absent = ParquetReader::new(&file.0).with_predicate(compare(
            "id",
            CompareOp::Eq,
            ScalarValue::Int64(99),
        ));
        assert_eq!(row_count(&absent), 0);
    }

    #[test]
    fn supports_boolean_composition_and_null_counts() {
        let file = fixture();
        let range = StoragePredicate::And(vec![
            compare("id", CompareOp::Ge, ScalarValue::Int64(3)),
            compare("id", CompareOp::Lt, ScalarValue::Int64(6)),
        ]);
        assert_eq!(
            row_count(&ParquetReader::new(&file.0).with_predicate(range)),
            ROW_GROUP_SIZE
        );

        let nulls = StoragePredicate::IsNull {
            column: "label".to_owned(),
        };
        assert_eq!(
            row_count(&ParquetReader::new(&file.0).with_predicate(nulls)),
            ROW_GROUP_SIZE
        );

        let non_nulls = StoragePredicate::IsNotNull {
            column: "label".to_owned(),
        };
        assert_eq!(
            row_count(&ParquetReader::new(&file.0).with_predicate(non_nulls)),
            6
        );

        let disjunction = StoragePredicate::Or(vec![
            compare("id", CompareOp::Eq, ScalarValue::Int64(1)),
            compare("id", CompareOp::Eq, ScalarValue::Int64(4)),
        ]);
        assert_eq!(
            row_count(&ParquetReader::new(&file.0).with_predicate(disjunction)),
            6
        );
    }

    #[test]
    fn supports_in_and_not_equal_pruning() {
        let file = fixture();
        let values = StoragePredicate::In {
            column: "id".to_owned(),
            values: vec![ScalarValue::Int64(4), ScalarValue::Int64(99)],
        };
        assert_eq!(
            row_count(&ParquetReader::new(&file.0).with_predicate(values)),
            ROW_GROUP_SIZE
        );

        assert!(!bounds_can_match(
            Some(ScalarValue::Int64(4)),
            Some(ScalarValue::Int64(4)),
            CompareOp::Ne,
            &ScalarValue::Int64(4),
        ));
    }
}
