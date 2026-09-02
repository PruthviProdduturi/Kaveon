use criterion::{Criterion, criterion_group, criterion_main};

fn bench_hash_aggregate(_c: &mut Criterion) {
    // TODO: generate test Parquet, benchmark GROUP BY
}

criterion_group!(benches, bench_hash_aggregate);
criterion_main!(benches);
