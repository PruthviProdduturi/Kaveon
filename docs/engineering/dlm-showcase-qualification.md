# DLM showcase qualification

This is the acceptance contract for enabling DLM-backed chart serving on the
canonical eight-dashboard showcase. A dashboard must always prefer a live
Engine query when the compiled context cannot reproduce the generated SQL
exactly.

## Eligibility matrix

| Dashboard | Charts | Exact DLM candidates | Required SQL fallback |
| --- | ---: | ---: | ---: |
| COVID-19 Global Overview | 6 | 3 | 3 |
| Kaveon Platform & Growth | 10 | 10 | 0 |
| Kaveon Events | 12 | 12 | 0 |
| Kaveon Worldwide Usage | 11 | 11 | 0 |
| AI Model Arena | 10 | 8 | 2 |
| NYC Yellow Taxi | 6 | 6 | 0 |
| Global Energy | 8 | 8 | 0 |
| Global Climate | 7 | 7 | 0 |
| **Total** | **70** | **65** | **5** |

The five required or provisional fallbacks are:

- `Case Fatality Rate`: its visualization uses a custom ratio expression that
  is not represented by the chart's `MAX(total_deaths)` metric configuration.
- `MMLU vs HumanEval` and `Cost vs Arena ELO`: the scatter definitions expose
  no aggregate metric for `serve-chart` to resolve.
- `Total Confirmed Cases` and `Total Deaths`: these are time-grain trend cards.
  Keep them on live SQL until a context response proves the same time-series
  columns, row order, and period semantics as the generated SQL.

## Required production state

Each of the nine registered datasets must have semantic columns and metric
expressions. DLM generation with empty `dataset_columns` or `dataset_metrics`
creates no useful answers. The compiled inventory must contain exactly the nine
active dataset IDs and no artifacts, routers, value-index rows, answers, or
sketches for deleted dataset IDs.

The OpenSource catalog is served by Kaveon Engine. The current PostgreSQL
profiler cannot read statistics or change counters through that connection, and
the dashboard client currently skips DLM whenever it resolves an Engine catalog.
Do not remove that client gate until Engine-backed profiling/freshness is
implemented or a conservative immutable-source policy is explicit and tested.

## Correctness checks

For every eligible chart, compare the DLM response with the generated SQL result
after normalizing numeric representation only. Column order, group keys, nulls,
row ordering, row count, and aggregate values must match.

Exercise these filter shapes where the dashboard exposes them:

1. No filters.
2. Every configured dimension as one equality filter.
3. Every configured `IN` dimension with one and multiple selected values.
4. A representative two-filter total and two-filter grouped result.
5. Every curated three- and four-dimension combination used by the dashboard.
6. Five or more simultaneous filters on the Kaveon dashboards. Since curation is
   capped at four dimensions, this must run live SQL unless an exact context
   exists.

An approximate proportional answer is never accepted for a dashboard chart.
The dashboard UI does not label approximate results, and the independence
assumption is unsafe for correlated dimensions and non-additive metrics.

## Freshness and fallback checks

- A context with recommendation `use_context` may serve.
- A context with recommendation `rebuild` must start the cooldown-protected
  background rebuild and return `served: false`, allowing live SQL fallback.
- Missing context, unknown metrics/dimensions, custom expressions, unsupported
  operators, and incomplete multi-metric answers must return `served: false`.
- An API error or `served: false` must leave the dashboard on its normal SQL path.
- After a controlled source-data change, no pre-change result may be served. The
  changed dataset must rebuild once, then match live SQL again.

Record the DLM hit count, SQL fallback count and reasons, equality comparisons,
freshness scores, rebuild events, and artifact inventory in the showcase
validation evidence. A rendered dashboard alone does not prove DLM correctness.
