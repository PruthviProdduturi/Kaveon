# Track 2 qualification: `ARRAY_JOIN` over large UTF-8 lists

Date: 2026-10-18

## Contract

`ARRAY_JOIN` accepts `ARRAY<VARCHAR>` values represented by Arrow
`LargeList<LargeUtf8>` without converting the element buffer through a
32-bit-offset UTF-8 array. It preserves Trino's delimiter, NULL replacement,
and NULL outer-array behavior while returning a regular VARCHAR result.

## Focused evidence

Run from `engine/`:

```powershell
cargo test -p kaveon-exec evaluates_array_join_for_large_utf8_lists --quiet
```

The regression joins `["alpha", NULL, "omega"]` with `|` and `<null>` and
returns `alpha|<null>|omega`. Broader external-client and cloud qualification
remains open in the parity matrix.
