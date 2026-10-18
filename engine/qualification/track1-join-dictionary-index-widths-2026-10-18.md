# Dictionary index widths for join keys

## Scope

Dictionary join-key decoding now accepts all Arrow signed and unsigned index
widths: Int8, UInt8, Int16, UInt16, Int32, UInt32, Int64, and UInt64. Index
values are validated before conversion to `usize`; negative and overflowing
indices fail closed. Value NULLs retain the existing non-matching behavior.

## Evidence

- `kaveon-exec` test
  `dictionary_index_widths_unbox_values_without_reordering` exercises all
  eight dictionary index types with reordered keys and NULL dictionary values.
- `kaveon-server` test
  `distributed_integer_join_index_widths_match_local` compares the supported
  integer value widths through local execution and the in-process three-worker
  fragment path.

The current Arrow Parquet writer rejects primitive dictionary arrays, so the
distributed check uses equivalent plain integer Parquet layouts. Dictionary
index decoding is directly covered by the execution regression; native
dictionary-encoded Parquet remains a storage qualification boundary.
