# Distributed mixed-integer aggregate extrema qualification

## Scope

The coordinator's aggregate extrema merge now compares mixed signed and
unsigned integer JSON values exactly, including values above the exact
floating-point range. Negative signed values are ordered below every unsigned
value; non-negative signed values are compared after lossless widening.

## Evidence

The server regression `merges_mixed_signed_unsigned_extrema_without_float_rounding`
merges partials containing large signed and unsigned integer values and
verifies the exact minimum and maximum.

## Boundary

This covers mixed integer extrema in the coordinator merge. Floating-point
values and mixed integer/real coercion retain their existing comparison path.
