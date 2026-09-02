use arrow::array::{ArrayRef, Float64Array, Int64Array, StringArray};
use arrow::datatypes::{DataType, Field, Schema};
use arrow::record_batch::RecordBatch;
use criterion::{BenchmarkId, Criterion, Throughput, black_box, criterion_group, criterion_main};
use kaveon_core::{CompareOp, ScalarValue, StoragePredicate};
use kaveon_storage::ParquetReader;
use parquet::arrow::ArrowWriter;
use parquet::file::properties::WriterProperties;
use std::fs::File;
use std::path::{Path, PathBuf};
use std::sync::Arc;

const DEFAULT_ROW_COUNT: usize = 1_000_000;
const WRITE_BATCH_SIZE: usize = 65_536;
const ROW_GROUP_SIZE: usize = 131_072;
const READ_BATCH_SIZE: usize = 8_192;
const REGION_COUNT: usize = 32;

struct BenchmarkFile(PathBuf);

impl Drop for BenchmarkFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

fn benchmark_row_count() -> usize {
    std::env::var("KAVEON_BENCH_ROWS")
        .ok()
        .and_then(|value| value.parse().ok())
        .filter(|value| *value > 0)
        .unwrap_or(DEFAULT_ROW_COUNT)
}

fn create_benchmark_file(row_count: usize) -> BenchmarkFile {
    let path = std::env::temp_dir().join(format!(
        "kaveon-storage-benchmark-{}-{row_count}.parquet",
        std::process::id()
    ));
    write_parquet(&path, row_count);
    BenchmarkFile(path)
}

fn write_parquet(path: &Path, row_count: usize) {
    let schema = Arc::new(Schema::new(vec![
        Field::new("row_id", DataType::Int64, false),
        Field::new("region", DataType::Utf8, false),
        Field::new("revenue", DataType::Float64, false),
        Field::new("quantity", DataType::Int64, false),
    ]));
    let properties = WriterProperties::builder()
        .set_max_row_group_size(ROW_GROUP_SIZE)
        .build();
    let output = File::create(path).expect("benchmark Parquet file must be creatable");
    let mut writer = ArrowWriter::try_new(output, Arc::clone(&schema), Some(properties))
        .expect("benchmark writer must initialize");

    for start in (0..row_count).step_by(WRITE_BATCH_SIZE) {
        let end = (start + WRITE_BATCH_SIZE).min(row_count);
        let ids = (start..end).map(|value| value as i64).collect::<Vec<_>>();
        let regions = (start..end)
            .map(|value| format!("region_{:02}", value % REGION_COUNT))
            .collect::<Vec<_>>();
        let revenue = (start..end)
            .map(|value| (value % 10_000) as f64 / 100.0)
            .collect::<Vec<_>>();
        let quantity = (start..end)
            .map(|value| (value % 100) as i64)
            .collect::<Vec<_>>();
        let batch = RecordBatch::try_new(
            Arc::clone(&schema),
            vec![
                Arc::new(Int64Array::from(ids)) as ArrayRef,
                Arc::new(StringArray::from(regions)) as ArrayRef,
                Arc::new(Float64Array::from(revenue)) as ArrayRef,
                Arc::new(Int64Array::from(quantity)) as ArrayRef,
            ],
        )
        .expect("benchmark batch must be valid");
        writer.write(&batch).expect("benchmark batch must write");
    }
    writer.close().expect("benchmark writer must close");
}

fn consume(reader: &ParquetReader) -> usize {
    reader
        .read()
        .expect("benchmark reader must open")
        .map(|batch| batch.expect("benchmark batch must decode").num_rows())
        .sum()
}

fn benchmark_storage(c: &mut Criterion) {
    let row_count = benchmark_row_count();
    let file = create_benchmark_file(row_count);
    let mut group = c.benchmark_group("parquet_storage");
    group.throughput(Throughput::Elements(row_count as u64));

    group.bench_with_input(
        BenchmarkId::new("full_scan", row_count),
        &file,
        |b, file| {
            b.iter(|| {
                let reader = ParquetReader::new(&file.0).with_batch_size(READ_BATCH_SIZE);
                black_box(consume(&reader));
            });
        },
    );

    group.bench_with_input(
        BenchmarkId::new("two_column_projection", row_count),
        &file,
        |b, file| {
            b.iter(|| {
                let reader = ParquetReader::new(&file.0)
                    .with_batch_size(READ_BATCH_SIZE)
                    .with_columns(vec!["region".to_owned(), "revenue".to_owned()]);
                black_box(consume(&reader));
            });
        },
    );

    group.bench_with_input(
        BenchmarkId::new("single_row_group", row_count),
        &file,
        |b, file| {
            b.iter(|| {
                let reader = ParquetReader::new(&file.0)
                    .with_batch_size(READ_BATCH_SIZE)
                    .with_columns(vec!["row_id".to_owned(), "revenue".to_owned()])
                    .with_predicate(StoragePredicate::Compare {
                        column: "row_id".to_owned(),
                        op: CompareOp::Eq,
                        value: ScalarValue::Int64((row_count / 2) as i64),
                    });
                black_box(consume(&reader));
            });
        },
    );

    group.finish();
}

criterion_group!(benches, benchmark_storage);
criterion_main!(benches);
