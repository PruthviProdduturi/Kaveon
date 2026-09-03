use std::sync::Arc;
use std::time::Duration;

use arrow::array::{ArrayRef, Float64Array, Int64Array, StringArray};
use arrow::datatypes::{DataType, Field, Schema, SchemaRef};
use arrow::record_batch::RecordBatch;
use criterion::{BenchmarkId, Criterion, Throughput, black_box, criterion_group, criterion_main};
use kaveon_core::{BatchOperator, BinaryOp, Expr, Result, ScalarValue};
use kaveon_exec::aggregate::{AggExpr, AggFunc, HashAggregate};
use kaveon_exec::filter::FilterOperator;
use kaveon_exec::project::ProjectOperator;

const DEFAULT_ROW_COUNT: usize = 1_000_000;
const BATCH_SIZE: usize = 8_192;
const LOW_CARDINALITY: usize = 16;
const MEDIUM_CARDINALITY: usize = 1_024;
const HIGH_CARDINALITY: usize = 65_536;
const SAMPLE_SIZE: usize = 20;
const MEASUREMENT_SECONDS: u64 = 5;

#[derive(Clone)]
struct MemorySource {
    schema: SchemaRef,
    batches: Vec<RecordBatch>,
    next: usize,
}

impl MemorySource {
    fn new(batches: Vec<RecordBatch>) -> Self {
        let schema = batches
            .first()
            .expect("benchmark data must not be empty")
            .schema();
        Self {
            schema,
            batches,
            next: 0,
        }
    }
}

impl BatchOperator for MemorySource {
    fn schema(&self) -> &SchemaRef {
        &self.schema
    }

    fn next_batch(&mut self) -> Result<Option<RecordBatch>> {
        let batch = self.batches.get(self.next).cloned();
        self.next += usize::from(batch.is_some());
        Ok(batch)
    }
}

fn benchmark_row_count() -> usize {
    std::env::var("KAVEON_BENCH_ROWS")
        .ok()
        .and_then(|value| value.parse().ok())
        .filter(|value| *value > 0)
        .unwrap_or(DEFAULT_ROW_COUNT)
}

fn make_batches(row_count: usize, cardinality: usize) -> Vec<RecordBatch> {
    let schema = Arc::new(Schema::new(vec![
        Field::new("row_id", DataType::Int64, false),
        Field::new("group_id", DataType::Int64, false),
        Field::new("region", DataType::Utf8, false),
        Field::new("revenue", DataType::Float64, false),
        Field::new("quantity", DataType::Int64, false),
    ]));

    (0..row_count)
        .step_by(BATCH_SIZE)
        .map(|start| {
            let end = (start + BATCH_SIZE).min(row_count);
            let rows = start..end;
            let row_ids = rows.clone().map(|row| row as i64).collect::<Vec<_>>();
            let group_ids = rows
                .clone()
                .map(|row| (row % cardinality) as i64)
                .collect::<Vec<_>>();
            let regions = rows
                .clone()
                .map(|row| format!("region_{:02}", row % LOW_CARDINALITY))
                .collect::<Vec<_>>();
            let revenue = rows
                .clone()
                .map(|row| ((row * 37) % 100_000) as f64 / 100.0)
                .collect::<Vec<_>>();
            let quantity = rows
                .map(|row| ((row * 17) % 100) as i64 + 1)
                .collect::<Vec<_>>();
            RecordBatch::try_new(
                Arc::clone(&schema),
                vec![
                    Arc::new(Int64Array::from(row_ids)) as ArrayRef,
                    Arc::new(Int64Array::from(group_ids)) as ArrayRef,
                    Arc::new(StringArray::from(regions)) as ArrayRef,
                    Arc::new(Float64Array::from(revenue)) as ArrayRef,
                    Arc::new(Int64Array::from(quantity)) as ArrayRef,
                ],
            )
            .expect("benchmark batch must be valid")
        })
        .collect()
}

fn consume(operator: &mut dyn BatchOperator) -> usize {
    let mut rows = 0;
    while let Some(batch) = operator
        .next_batch()
        .expect("benchmark operator must execute")
    {
        rows += batch.num_rows();
        black_box(batch);
    }
    rows
}

fn aggregate_operator(source: MemorySource) -> HashAggregate {
    HashAggregate::new(
        Box::new(source),
        vec!["group_id".to_owned()],
        vec![
            AggExpr::new(AggFunc::Count, "*"),
            AggExpr::new(AggFunc::Sum, "revenue"),
            AggExpr::new(AggFunc::Avg, "quantity"),
            AggExpr::new(AggFunc::Min, "revenue"),
            AggExpr::new(AggFunc::Max, "revenue"),
        ],
    )
    .expect("benchmark aggregate must initialize")
}

fn benchmark_hash_aggregate(c: &mut Criterion) {
    let row_count = benchmark_row_count();
    let mut group = c.benchmark_group("hash_aggregate");
    group.throughput(Throughput::Elements(row_count as u64));
    group.sample_size(SAMPLE_SIZE);
    group.measurement_time(Duration::from_secs(MEASUREMENT_SECONDS));
    for cardinality in [LOW_CARDINALITY, MEDIUM_CARDINALITY, HIGH_CARDINALITY] {
        let source = MemorySource::new(make_batches(row_count, cardinality.min(row_count)));
        group.bench_with_input(
            BenchmarkId::new("group_by_count_sum_avg_min_max", cardinality),
            &source,
            |b, source| {
                b.iter(|| {
                    let mut operator = aggregate_operator(source.clone());
                    black_box(consume(&mut operator));
                });
            },
        );
    }
    group.finish();
}

fn benchmark_vector_pipeline(c: &mut Criterion) {
    let row_count = benchmark_row_count();
    let source = MemorySource::new(make_batches(row_count, MEDIUM_CARDINALITY));
    let mut group = c.benchmark_group("vector_pipeline");
    group.throughput(Throughput::Elements(row_count as u64));
    group.sample_size(SAMPLE_SIZE);
    group.measurement_time(Duration::from_secs(MEASUREMENT_SECONDS));

    group.bench_function("filter_selectivity_50_percent", |b| {
        b.iter(|| {
            let predicate = Expr::BinaryOp {
                left: Box::new(Expr::Column("quantity".to_owned())),
                op: BinaryOp::Gt,
                right: Box::new(Expr::Literal(ScalarValue::Int64(50))),
            };
            let mut operator = FilterOperator::new(Box::new(source.clone()), predicate);
            black_box(consume(&mut operator));
        })
    });

    group.bench_function("filter_project_arithmetic", |b| {
        b.iter(|| {
            let predicate = Expr::BinaryOp {
                left: Box::new(Expr::Column("quantity".to_owned())),
                op: BinaryOp::Gt,
                right: Box::new(Expr::Literal(ScalarValue::Int64(50))),
            };
            let filtered = FilterOperator::new(Box::new(source.clone()), predicate);
            let arithmetic = Expr::BinaryOp {
                left: Box::new(Expr::Column("revenue".to_owned())),
                op: BinaryOp::Multiply,
                right: Box::new(Expr::Literal(ScalarValue::Float64(1.07))),
            };
            let mut operator = ProjectOperator::new(
                Box::new(filtered),
                vec![Expr::Column("group_id".to_owned()), arithmetic],
            )
            .expect("benchmark projection must initialize");
            black_box(consume(&mut operator));
        })
    });
    group.finish();
}

criterion_group!(benches, benchmark_hash_aggregate, benchmark_vector_pipeline);
criterion_main!(benches);
