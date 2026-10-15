# Apache Ossie interchange — scope

Apache Ossie (incubating), formerly Open Semantic Interchange, is a
vendor-neutral YAML format for semantic models: datasets, fields,
relationships and metrics. It began in November 2025 with 17 partners
(Snowflake, Salesforce, dbt Labs) and now has more than 50, including
Databricks, Oracle, Informatica, Collibra and Qlik.

The case for Kaveon speaking it is not that Kaveon lacks a semantic model — it
has one — but that the industry is agreeing on a format, and a self-hosted
open-source platform competing on openness should not be the one that cannot
import or export it. Spec: `version: 0.2.0.dev0` (unreleased and mutable;
0.1.1 is the last stable, 2025-12-11).

## What maps cleanly

Kaveon's dataset document already carries four of Ossie's five core classes.

| Ossie | Kaveon today |
|---|---|
| `name`, `description` | `name` / `dataset_name`, `description` |
| `datasets[].source` | `database_name` + `schema_name` + `table_name` |
| `fields[].name` | `columns[].column_name` |
| `fields[].datatype` | `columns[].data_type`, via a mapping (below) |
| `fields[].dimension.is_time` | `date_column`, and Date/DateTime datatypes |
| `metrics[].name` | `metrics[].name` |
| `metrics[].expression` | `metrics[].expression`, e.g. `SUM(primary_energy_consumption)` |
| `relationships` | `dimensions[]` — `dimension_table`, `fact_key`, `join_key` |
| `ai_context.synonyms` | the DLM context spec's aliases |

The datatype mapping is small and total: Ossie allows String, Integer,
Decimal, Float, Boolean, Date, Time, DateTime, DateTimeTz, Opaque; Kaveon
stores Arrow names (`Utf8`, `Int64`, `Date32`, `Float64`). Anything unmapped
becomes `Opaque` rather than being guessed at.

## The one piece that fits unusually well

Ossie expresses every expression as a list of dialect-keyed alternatives:

```yaml
expression:
  dialects:
    - dialect: ANSI_SQL
      expression: SUM(orders.amount)
```

Declared dialects include ANSI_SQL, SNOWFLAKE, DATABRICKS, BIGQUERY, TABLEAU,
DAX, MDX and OSSIE_SQL_2026, and only OSSIE_SQL_2026 has defined semantics —
the rest are opaque strings.

Kaveon already renders one logical expression per dialect: `dlm/engine_dialect.py`
holds a `Dialect` per backend with `expression()`, `ident()`, `literal()` and
the date predicates, because the DLM has always had to write the same question
for PostgreSQL, Fabric/Azure SQL and the Engine. Emitting more than one dialect
block is therefore close to free, and is a thing most single-backend tools
cannot do at all.

## What does not map, and must not be pretended

Ossie's core spec has **no filter concept** at dataset or metric level, and
**no aggregation-type field** — a metric is an opaque SQL string. So:

- `row_filters` (added 2026-10-14, the `iso_code IS NOT NULL` that separates
  countries from aggregates) has nowhere to live in core Ossie. It goes in
  `custom_extensions`, and an export must be explicit that another tool reading
  the model **will not apply it** and will therefore answer "top 10 countries"
  with World and OECD in the list. That is a correctness caveat, not a
  formatting one.
- `metric_type` (`sum`, `avg`, `distinct_count`) is derivable from the
  expression but is not a spec field; it goes in `custom_extensions`.
- `semantic_type`, the cube `shape` (dimensions with caps, measures with their
  permitted aggregates), and the DLM's additivity and breakdown declarations
  have no equivalent. All `custom_extensions`.
- Ossie's `primary_key` and `unique_keys` have no equivalent in Kaveon and are
  preserved on import without being acted on.

## Scope

**Export** — `GET /api/v1/datasets/{id}/ossie`, returning YAML.
- One dataset to one `semantic_model`. Kaveon has no multi-dataset model object
  and should not invent one.
- Fields from `columns`, metrics from `metrics`, relationships from
  `dimensions`, synonyms from the dataset's context spec aliases.
- Expressions emitted for ANSI_SQL plus the dialect of the dataset's own
  backend, through the existing `Dialect` objects.
- Everything Kaveon knows and Ossie does not goes under
  `custom_extensions: [{name: KAVEON, ...}]`, never silently dropped.

**Import** — `POST /api/v1/datasets/ossie`.
- Validate `version` and refuse an unknown major rather than guessing; the spec
  is pre-1.0 and 0.2.0 already broke the document root.
- Map the inverse. A metric whose expression has no ANSI_SQL or OSSIE_SQL_2026
  dialect is imported as a named metric that cannot be run, reported as such,
  not dropped.
- Round-trip test: export, import, export, and assert the second export equals
  the first.

**Deliberately out of scope**
- No multi-dataset models, no MDX/DAX/Tableau expression translation (they are
  opaque in the spec and Kaveon has no renderer for them), and no attempt to
  represent the cube shape in core fields.

## Cost and risk

Roughly a day: a serialiser, a deserialiser, the datatype table, two routes,
and the round-trip plus caveat tests. The only real risk is the spec moving —
it is `0.2.0.dev0` and explicitly mutable — so the version check must refuse
rather than cope, and the export should state the spec version it wrote.

## Why bother

Interoperability is the argument a self-hosted MIT platform can make that a
hosted competitor cannot, and it costs a day. The thing to avoid is the
opposite outcome: the semantic layer standardises around a format Kaveon
cannot read, and Kaveon's own model becomes proprietary by omission.
