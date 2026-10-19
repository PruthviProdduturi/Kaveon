# Decimal128 runtime-filter row-group pruning

## Scope

This qualification covers the Decimal128 Parquet path when the column is
encoded as `FIXED_LEN_BYTE_ARRAY`. Runtime-filter bounds are decoded as signed
big-endian two's-complement integers and compared at the declared decimal
scale. Unsupported fixed-width types, including Decimal256, remain
conservative and are retained for a later wider-statistics implementation.

## Evidence

- The local Parquet fixture uses `DECIMAL(38,2)` and two row groups.
- Equality on `400.00` considers both row groups, prunes the first, and returns
  the exact row `[400]`.
- The partitioned join regression uses three worker-style partitions with
  duplicate build keys, a non-matching value, and a NULL probe key. It returns
  exactly two `(400,400)` rows and releases its query reservation.

Commands:

```powershell
cargo test -p kaveon-storage decimal128_runtime_filter_prunes_fixed_length_statistics -- --nocapture
cargo test -p kaveon-exec partitioned_join_applies_decimal128_runtime_filter_without_precision_loss -- --nocapture
```

## Boundary

This is local and in-process partitioned evidence. It does not qualify ADLS,
Delta, or a deployed multi-worker image; those remain cloud qualification work.
