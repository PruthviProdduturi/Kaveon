# Distributed UInt64 aggregate extrema qualification

## Scope

Distributed partial-aggregate `MIN` and `MAX` now compare same-type signed
and unsigned integer values exactly before falling back to floating-point
comparison for mixed numeric values. This preserves adjacent `UInt64` values
above the exact `f64` range.

## Evidence

The server regression `merges_uint64_extrema_without_float_rounding` merges
worker partials containing `UInt64` values near `u64::MAX` and verifies the
exact minimum and maximum without precision loss.

## Boundary

This covers same-type integer extrema in the coordinator partial merge. Mixed
signed/unsigned numeric coercion and floating-point extrema retain their
existing behavior.
