//! Late materialisation measured: a wide Parquet object (100 columns,
//! 2 M rows, a URL column with a 2 % `LIKE '%google%'` hit rate, scattered
//! uniformly) read through the object reader with the row filter on and
//! off, over dictionary and plain text encodings, with and without an
//! offset index. Ignored: run it by hand in release —
//!
//! `cargo test -p kaveon-storage --release late_materialisation_benchmark -- --ignored --nocapture`
//!
//! It prints wall time, compressed bytes the decoder read, and rows the
//! row filter admitted. Single runs are never benchmark claims; the medians
//! here are for choosing defaults and for the qualification record.
use std::sync::Arc;
use std::time::{Duration, Instant};

use arrow::array::{ArrayRef, Float64Array, Int64Array, StringArray, StringDictionaryBuilder};
use arrow::datatypes::{DataType, Field, Int32Type, Schema};
use arrow::record_batch::RecordBatch;
use kaveon_core::StoragePredicate;
use object_store::{ObjectStore, PutPayload, memory::InMemory, path::Path};
use parquet::arrow::ArrowWriter;
use parquet::file::properties::WriterProperties;

use crate::adls_reader::{AdlsAuthMode, AdlsParquetReader, cache_object_store};
use crate::scan_predicate::LateMaterialisation;
use crate::{ScanMetrics, ScanMetricsSnapshot};

const ROWS: usize = 2_000_000;
const ROW_GROUP_ROWS: usize = 250_000;
const INT_COLUMNS: usize = 60;
const FLOAT_COLUMNS: usize = 20;
const CATEGORY_COLUMNS: usize = 18;
const HIT_EVERY: u64 = 50;

fn mix(seed: u64) -> u64 {
    let mut value = seed.wrapping_mul(0x9E37_79B9_7F4A_7C15);
    value ^= value >> 29;
    value = value.wrapping_mul(0xBF58_476D_1CE4_E5B9);
    value ^ (value >> 32)
}

fn url(row: usize) -> String {
    if mix(row as u64 * 7 + 3).is_multiple_of(HIT_EVERY) {
        format!(
            "http://www.google.com/search?q={}&start={}",
            mix(row as u64),
            row % 100
        )
    } else {
        format!(
            "http://site-{}.example/path/{}/{}",
            mix(row as u64) % 20_000,
            mix(row as u64 + 1) % 1_000,
            row
        )
    }
}

/// The wide table as one batch per row group.
fn row_group(start: usize, dictionary: bool) -> RecordBatch {
    let rows = ROW_GROUP_ROWS.min(ROWS - start);
    let mut fields = vec![Field::new("id", DataType::Int64, false)];
    let mut arrays: Vec<ArrayRef> = vec![Arc::new(Int64Array::from_iter_values(
        (start..start + rows).map(|row| row as i64),
    ))];
    for column in 0..INT_COLUMNS {
        fields.push(Field::new(format!("i{column}"), DataType::Int64, false));
        arrays.push(Arc::new(Int64Array::from_iter_values(
            (start..start + rows).map(|row| (mix(row as u64 * 131 + column as u64) % 1_000) as i64),
        )));
    }
    for column in 0..FLOAT_COLUMNS {
        fields.push(Field::new(format!("f{column}"), DataType::Float64, false));
        arrays.push(Arc::new(Float64Array::from_iter_values(
            (start..start + rows)
                .map(|row| (mix(row as u64 * 17 + column as u64) % 10_000) as f64 / 100.0),
        )));
    }
    let text_type = if dictionary {
        DataType::Dictionary(Box::new(DataType::Int32), Box::new(DataType::Utf8))
    } else {
        DataType::Utf8
    };
    for column in 0..CATEGORY_COLUMNS {
        fields.push(Field::new(format!("c{column}"), text_type.clone(), false));
        let values = (start..start + rows)
            .map(|row| format!("category-{}", mix(row as u64 * 3 + column as u64) % 50));
        arrays.push(text_array(values, dictionary));
    }
    fields.push(Field::new("url", text_type, false));
    arrays.push(text_array((start..start + rows).map(url), dictionary));
    RecordBatch::try_new(Arc::new(Schema::new(fields)), arrays).unwrap()
}

fn text_array(values: impl Iterator<Item = String>, dictionary: bool) -> ArrayRef {
    if dictionary {
        let mut builder = StringDictionaryBuilder::<Int32Type>::new();
        for value in values {
            builder.append_value(value);
        }
        Arc::new(builder.finish())
    } else {
        Arc::new(StringArray::from_iter_values(values))
    }
}

fn write_object(dictionary: bool, offset_index: bool) -> Vec<u8> {
    let mut bytes = Vec::new();
    let first = row_group(0, dictionary);
    let properties = WriterProperties::builder()
        .set_max_row_group_size(ROW_GROUP_ROWS)
        .set_dictionary_enabled(dictionary)
        .set_offset_index_disabled(!offset_index)
        .build();
    let mut writer = ArrowWriter::try_new(&mut bytes, first.schema(), Some(properties)).unwrap();
    writer.write(&first).unwrap();
    let mut start = ROW_GROUP_ROWS;
    while start < ROWS {
        writer.write(&row_group(start, dictionary)).unwrap();
        start += ROW_GROUP_ROWS;
    }
    writer.close().unwrap();
    bytes
}

struct Measurement {
    elapsed: Duration,
    rows: usize,
    snapshot: ScanMetricsSnapshot,
}

async fn scan(
    reader: AdlsParquetReader,
    columns: Option<Vec<String>>,
    mode: LateMaterialisation,
) -> Measurement {
    let metrics = ScanMetrics::default();
    let mut reader = reader
        .with_metrics(metrics.clone())
        .with_late_materialisation(mode)
        .with_predicate(StoragePredicate::Like {
            column: "url".into(),
            pattern: "%google%".into(),
            negated: false,
            case_insensitive: false,
        });
    if let Some(columns) = columns {
        reader = reader.with_columns(columns);
    }
    let started = Instant::now();
    let mut stream = reader.read().await.unwrap();
    let mut rows = 0;
    while let Some(batch) = stream.next_batch().await.unwrap() {
        rows += batch.num_rows();
    }
    Measurement {
        elapsed: started.elapsed(),
        rows,
        snapshot: metrics.snapshot(),
    }
}

/// (minimum, median) of the samples.
fn spread(mut samples: Vec<Duration>) -> (Duration, Duration) {
    samples.sort();
    (samples[0], samples[samples.len() / 2])
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "benchmark: release build, several minutes"]
async fn late_materialisation_benchmark() {
    let samples = std::env::var("KAVEON_BENCH_SAMPLES")
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(5);
    println!(
        "rows {ROWS}, row groups of {ROW_GROUP_ROWS}, {} columns, LIKE hit rate 1/{HIT_EVERY}, lanes {}, {samples} samples after one warm-up (min and median wall)",
        1 + INT_COLUMNS + FLOAT_COLUMNS + CATEGORY_COLUMNS + 1,
        crate::adls_reader::scan_parallelism()
    );
    println!(
        "{:<11} {:<7} {:<10} {:<7} {:>7} {:>7} {:>10} {:>12} {:>10} {:>10}",
        "text",
        "index",
        "projection",
        "filter",
        "min_ms",
        "med_ms",
        "rows",
        "bytes_read",
        "examined",
        "admitted"
    );
    for (dictionary, offset_index) in [(true, true), (true, false), (false, true), (false, false)] {
        let bytes = write_object(dictionary, offset_index);
        let account = format!(
            "bench-{}-{}-{}",
            std::process::id(),
            dictionary,
            offset_index
        );
        let store: Arc<dyn ObjectStore> = Arc::new(InMemory::new());
        store
            .put(&Path::from("wide.parquet"), PutPayload::from(bytes.clone()))
            .await
            .unwrap();
        cache_object_store(
            format!("{account}/wide/{:?}", AdlsAuthMode::Environment),
            store,
        );
        println!(
            "object: {} bytes, text {}, offset index {}",
            bytes.len(),
            if dictionary { "dictionary" } else { "plain" },
            offset_index
        );
        for (projection, columns) in [
            ("all", None),
            ("id,url", Some(vec!["id".to_owned(), "url".to_owned()])),
        ] {
            for mode in [
                LateMaterialisation::Never,
                LateMaterialisation::Always,
                LateMaterialisation::Auto,
            ] {
                // One untimed pass so page faults and allocator growth
                // are not charged to the first mode measured.
                scan(
                    AdlsParquetReader::new(&account, "wide", "wide.parquet"),
                    columns.clone(),
                    mode,
                )
                .await;
                let mut times = Vec::new();
                let mut last = None;
                for _ in 0..samples {
                    let reader = AdlsParquetReader::new(&account, "wide", "wide.parquet");
                    let measured = scan(reader, columns.clone(), mode).await;
                    times.push(measured.elapsed);
                    last = Some(measured);
                }
                let last = last.unwrap();
                let (minimum, median) = spread(times);
                println!(
                    "{:<11} {:<7} {:<10} {:<7} {:>7} {:>7} {:>10} {:>12} {:>10} {:>10}",
                    if dictionary { "dictionary" } else { "plain" },
                    offset_index,
                    projection,
                    format!("{mode:?}"),
                    minimum.as_millis(),
                    median.as_millis(),
                    last.rows,
                    last.snapshot.compressed_bytes_read,
                    last.snapshot.row_filter_rows_examined,
                    last.snapshot.row_filter_rows_admitted,
                );
            }
        }
    }
}
