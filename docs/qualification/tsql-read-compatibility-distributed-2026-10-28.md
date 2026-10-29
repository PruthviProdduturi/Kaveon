# T-SQL read compatibility: distributed qualification

## Scope

This record qualifies the bounded SQL Server/Fabric read corpus in
[`tsql-read-compatibility-corpus-2026-10-28.json`](tsql-read-compatibility-corpus-2026-10-28.json).
It is Engine execution evidence, not a claim of general Fabric Warehouse
parity or external-client parity.

## Evidence

- Corpus: 31 cases, including explicit fail-closed cases for `PIVOT`,
  `UNPIVOT`, correlated `APPLY`, `TOP WITH TIES` without an order, and
  runtime-column styled date conversion.
- Planner gate: `versioned_tsql_corpus_matches_declared_boundaries` accepts
  every `planner_qualified` case and rejects every `explicit_rejection` case.
- Distributed gate: the in-process three-worker differential sweep completed
  with exact local-vs-distributed results after the `DATENAME` implementation;
  the run completed without worker failures or retained exchange/spill files.
- The corpus-level rerun after adding the `ISNUMERIC` distributed shape
  completed 1/1 with exact results in 729.75 seconds; the full differential
  workload completed without worker or spill failures.
- Focused distributed regression `distributed_tsql_left_and_right_match_local_unicode_semantics`
  passed with three workers and zero retained memory reservations.
- Qualified additions in this corpus revision: `COUNT_BIG`, `LEN`, `IIF`,
  `CHARINDEX`, `EOMONTH`, `DATEFROMPARTS`, `DATENAME`, `ISDATE`, `ISNUMERIC`,
  `LEFT`, and `RIGHT`.

`ISDATE` returns a non-null boolean 0/1 result. Text is parsed through a safe
typed date cast; numeric values are rejected as date inputs instead of being
interpreted as epoch offsets.

`ISNUMERIC` returns a non-null boolean 0/1 result. Native numeric values and
text accepted by the safe numeric cast are qualified; locale-specific currency
and SQL Server type-coercion extensions remain outside the contract.

`LEFT` and `RIGHT` return bounded Unicode scalar prefixes/suffixes, preserve
NULLs, and use the Engine's width-aware text path for narrow and large UTF-8
values.

## Contract

The qualified surface is deliberately bounded. `DATENAME` returns deterministic
English calendar names (and textual numeric parts); locale/session-language
formatting is not part of the contract. `EOMONTH` and `DATEFROMPARTS` preserve
typed date semantics and fail closed on invalid input. `TOP PERCENT` and
`TOP WITH TIES` remain qualified only where the ordering contract is
deterministic.

External Fabric/Azure SQL sources continue to use the platform passthrough
path. They are not executed by the Engine and are not covered by this record.
