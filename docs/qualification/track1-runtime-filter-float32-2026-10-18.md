# Track 1 Float32 runtime-filter qualification

Kaveon now compiles an exact `Float64` build-side runtime-filter value into an Arrow `Float32` literal when the value is exactly representable by `f32`. Values that would round during narrowing are left out of storage pushdown, preserving correctness rather than changing the join key. The local Parquet reader applies the filter before decoding the first batch.

## Evidence

- Test: `cargo test -p kaveon-storage applies_float32_runtime_filter_without_rounding --quiet`
- The fixture contains four `Float32` rows in two row groups and the runtime filter selects exactly `2.25`.
- The result contains only the matching row, proving the filter is evaluated with the column's physical type.

This qualifies the local Parquet Float32 equality path. It does not qualify Float32 runtime filtering across every object-store reader or deployed cloud image, and it does not add Decimal256 or scale conversion.