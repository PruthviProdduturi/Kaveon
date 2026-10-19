# Track 2 qualification: `ARRAY_POSITIONS`

Date: 2026-10-19

`ARRAY_POSITIONS(array, element)` returns a typed `ARRAY<BIGINT>` containing
every one-based index whose element equals the requested value, preserving
input order. A NULL input array produces a NULL row; a non-NULL array with no
matches produces an empty array. A NULL needle matches NULL elements using the
same null-aware equality contract as `ARRAY_POSITION`.

## Evidence

Run from `engine/`:

```powershell
cargo test -p kaveon-exec --lib array_positions
```

The distributed differential corpus compares repeated matches and NULL-row
behavior across local and three-worker execution. Nested and temporal element
comparisons use the existing recursive equality path.

External-client and cloud qualification remain part of the broader Track 2
boundary.
