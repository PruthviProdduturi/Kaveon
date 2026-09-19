//! The differential sweep as a gate: the statements of
//! `scripts/differential-cases.py`, run against the same rows held in two
//! Parquet encodings — an Arrow dictionary schema for every text column,
//! and plain UTF-8 — through the node-local planner, again against the
//! plain rows held as one file and as a directory of three files, again
//! against the same rows held as a Hive-partitioned directory
//! (`region=…/industry=…/`, the keys read from the paths) and as one file
//! of the same column order, and through the distributed planner's
//! fragments executed in this process as a two-worker cluster would run
//! them — for the dictionary file and for the partitioned directory. Any
//! divergence is an Engine defect, whatever the cluster later says. The
//! encodings exercise different operator paths (dictionary-aware
//! predicates, coded folds, the columnar aggregate's arena keys) against
//! one truth; the layouts exercise the directory reader (listing, file
//! assignment, per-file pruning, partition columns and partition pruning)
//! against the single-file reader; the fragment path exercises the stage
//! planner, the exchanges and the fragment compiler.
use std::collections::{BTreeMap, HashMap};
use std::fs::File;
use std::path::PathBuf;
use std::sync::Arc;

use arrow::array::{
    Array, ArrayRef, Date32Array, Int64Array, StringArray, StringDictionaryBuilder,
};
use arrow::compute::cast;
use arrow::datatypes::{DataType, Field, Int32Type, Schema};
use arrow::record_batch::RecordBatch;
use kaveon_core::{
    AccessPattern, BatchOperator, CatalogManager, CatalogProvider, DataFormat, ExchangeId,
    MemoryCatalog, Partitioning, QueryMemoryPool, Result, StageId, StorageType, TableMeta,
};
use kaveon_sql::logical_plan::LogicalPlan;
use kaveon_storage::ScanPartition;
use parquet::arrow::ArrowWriter;
use parquet::file::properties::WriterProperties;

use crate::fragment_exec::{
    ExchangeBatches, ExchangeInputProvider, ExchangeOutputBatches, execute_fragment_with_memory,
};

/// The exchange inputs of one task: the batches every producer task
/// wrote to the partition this task consumes.
struct TaskInputs {
    inputs: HashMap<ExchangeId, ExchangeBatches>,
}

impl ExchangeInputProvider for TaskInputs {
    fn read(&self, exchange_id: &ExchangeId) -> Result<ExchangeBatches> {
        self.inputs
            .get(exchange_id)
            .map(|input| ExchangeBatches {
                schema: input.schema.clone(),
                batches: input.batches.clone(),
            })
            .ok_or_else(|| {
                kaveon_core::KaveonError::Execution(format!(
                    "exchange {exchange_id} was not produced before its consumer ran"
                ))
            })
    }
}

/// Execute `plan` as a coordinator would across `workers` workers, in
/// this process: every stage's tasks run through the fragment executor
/// in dependency order, and each task's exchange outputs are routed to
/// the consuming tasks as the orchestrator routes them — output
/// partition `p` of every producer to consumer task `p`, a broadcast or
/// single output to every consumer. The root stage's result batches come
/// back. This is the fragment path without the network: the stage
/// planner, the fragment compiler and every operator a worker runs.
pub(crate) fn execute_distributed(
    query: &str,
    plan: &LogicalPlan,
    manager: &CatalogManager,
    workers: usize,
    pool: &QueryMemoryPool,
) -> Result<Vec<RecordBatch>> {
    let graph = crate::planner::build_stage_graph(query, plan, workers)?;
    let fragments = crate::planner::build_executable_fragments(query, plan, manager, workers)?;
    let mut produced: HashMap<ExchangeId, Vec<ExchangeOutputBatches>> = HashMap::new();
    let mut done: Vec<StageId> = Vec::new();
    let mut results = Vec::new();
    while done.len() < graph.stages.len() {
        let stage = graph
            .stages
            .iter()
            .find(|stage| {
                !done.contains(&stage.id)
                    && graph
                        .exchanges
                        .iter()
                        .filter(|exchange| exchange.target_stage == stage.id)
                        .all(|exchange| done.contains(&exchange.source_stage))
            })
            .ok_or_else(|| {
                kaveon_core::KaveonError::Execution("the stage graph has a cycle".into())
            })?;
        let fragment = fragments
            .get(&stage.id)
            .ok_or_else(|| kaveon_core::KaveonError::Execution("stage without fragment".into()))?;
        for task in 0..stage.task_count {
            let mut inputs = HashMap::new();
            for exchange in graph
                .exchanges
                .iter()
                .filter(|exchange| exchange.target_stage == stage.id)
            {
                let partition = match exchange.partitioning {
                    Partitioning::Single | Partitioning::Broadcast => 0,
                    Partitioning::Hash { .. } | Partitioning::RoundRobin { .. } => task,
                };
                let outputs = produced.get(&exchange.id).ok_or_else(|| {
                    kaveon_core::KaveonError::Execution(format!(
                        "exchange {} has no producer output",
                        exchange.id
                    ))
                })?;
                let schema = outputs
                    .first()
                    .map(|output| output.schema.clone())
                    .ok_or_else(|| {
                        kaveon_core::KaveonError::Execution(format!(
                            "exchange {} has no producer",
                            exchange.id
                        ))
                    })?;
                let batches = outputs
                    .iter()
                    .flat_map(|output| {
                        output
                            .partitions
                            .get(partition)
                            .cloned()
                            .unwrap_or_default()
                    })
                    .collect();
                inputs.insert(exchange.id.clone(), ExchangeBatches { schema, batches });
            }
            let execution = execute_fragment_with_memory(
                fragment,
                manager,
                &TaskInputs { inputs },
                ScanPartition::new(task, stage.task_count)?,
                Some(pool),
            )?;
            for (exchange, output) in execution.exchange_outputs {
                produced.entry(exchange).or_default().push(output);
            }
            if stage.id == graph.root_stage {
                results.extend(execution.result_batches);
            }
        }
        done.push(stage.id);
    }
    Ok(results)
}

/// (name, statement with {T} for the events table and {U} for users,
/// whether the statement orders its output).
const CASES: &[(&str, &str, bool)] = &[
    (
        "distinct_values",
        "SELECT DISTINCT region FROM {T} ORDER BY region",
        true,
    ),
    (
        "order_by_dict_desc",
        "SELECT country, region FROM {T} WHERE surface = 'Export' AND event_date = '2026-07-20' ORDER BY country DESC, region LIMIT 5",
        true,
    ),
    (
        "in_list",
        "SELECT country, SUM(actions) AS a FROM {T} WHERE country IN ('Japan', 'Brazil', 'Kenya') GROUP BY country ORDER BY country",
        true,
    ),
    (
        "not_equal",
        "SELECT COUNT(*) AS n FROM {T} WHERE region <> 'Asia'",
        false,
    ),
    (
        "like_prefix",
        "SELECT COUNT(*) AS n FROM {T} WHERE country LIKE 'United%'",
        false,
    ),
    // Late materialisation's shape: every column of the few rows a
    // contains-LIKE admits, then a top-N — the reader decodes the wide
    // rest only for the survivors, and the executor's filter is still the
    // truth on what arrives (the decoder admits short skip runs whole).
    (
        "wide_like_topn",
        "SELECT * FROM {T} WHERE country LIKE '%ted King%' AND actions > 30 ORDER BY event_date DESC, user_id, latency_p75_ms, duration_sec LIMIT 10",
        true,
    ),
    (
        "not_like_ilike",
        "SELECT country, surface, COUNT(*) AS n FROM {T} WHERE country NOT LIKE '%a%' AND surface ILIKE 'sql%' GROUP BY country, surface ORDER BY country, surface",
        true,
    ),
    (
        "like_or_null",
        "SELECT COUNT(*) AS n FROM {T} WHERE industry LIKE 'Gov%' OR industry IS NULL",
        false,
    ),
    (
        "or_predicate",
        "SELECT COUNT(*) AS n FROM {T} WHERE region = 'Africa' OR platform = 'Mobile'",
        false,
    ),
    (
        "null_safe",
        "SELECT COUNT(*) AS n FROM {T} WHERE country IS NOT NULL AND industry IS NULL",
        false,
    ),
    (
        "case_expr",
        "SELECT CASE WHEN region = 'Europe' THEN 'EU' ELSE 'Other' END AS zone, SUM(sessions) AS s FROM {T} GROUP BY CASE WHEN region = 'Europe' THEN 'EU' ELSE 'Other' END ORDER BY zone",
        true,
    ),
    (
        "upper_fn",
        "SELECT UPPER(surface) AS s, COUNT(*) AS n FROM {T} WHERE event_date = '2026-07-04' GROUP BY UPPER(surface) ORDER BY s",
        true,
    ),
    (
        "avg_min_max",
        "SELECT platform, AVG(duration_sec) AS d, MIN(latency_p75_ms) AS lo, MAX(latency_p75_ms) AS hi FROM {T} GROUP BY platform ORDER BY platform",
        true,
    ),
    (
        "text_min_max_grouped",
        "SELECT surface, MIN(country) AS first_country, MAX(event_date) AS last_day FROM {T} GROUP BY surface ORDER BY surface",
        true,
    ),
    (
        "three_keys",
        "SELECT region, platform, license, SUM(actions) AS a FROM {T} GROUP BY region, platform, license ORDER BY a DESC, region, platform, license LIMIT 12",
        true,
    ),
    (
        "mixed_key_types",
        "SELECT country, user_id % 7 AS bucket, COUNT(*) AS n FROM {T} WHERE event_date = '2026-07-15' AND surface = 'Chat' GROUP BY country, user_id % 7 ORDER BY n DESC, country, bucket LIMIT 10",
        true,
    ),
    (
        "having",
        "SELECT industry, SUM(errors) AS e FROM {T} GROUP BY industry HAVING SUM(errors) > 0 ORDER BY e DESC, industry LIMIT 5",
        true,
    ),
    (
        "count_distinct_lowcard",
        "SELECT COUNT(DISTINCT country) AS c, COUNT(DISTINCT surface) AS s FROM {T}",
        false,
    ),
    (
        "count_distinct_grouped_lowcard",
        "SELECT region, COUNT(DISTINCT country) AS c FROM {T} GROUP BY region ORDER BY region",
        true,
    ),
    (
        "date_range_and_dim",
        "SELECT country, SUM(queries_run) AS q FROM {T} WHERE event_date BETWEEN '2026-07-10' AND '2026-07-12' AND platform = 'Web' GROUP BY country ORDER BY q DESC, country LIMIT 5",
        true,
    ),
    (
        "join_users",
        "SELECT u.locale, SUM(t.actions) AS a FROM {T} t JOIN {U} u ON u.user_id = t.user_id WHERE t.event_date = '2026-07-04' AND t.surface = 'API' GROUP BY u.locale ORDER BY a DESC, u.locale LIMIT 5",
        true,
    ),
    (
        "join_dict_keys",
        "SELECT t.country, u.country AS user_country, COUNT(*) AS n FROM {T} t JOIN {U} u ON u.user_id = t.user_id WHERE t.event_date = '2026-07-04' AND t.surface = 'API' AND t.country <> u.country GROUP BY t.country, u.country ORDER BY n DESC, t.country, u.country LIMIT 3",
        true,
    ),
    (
        "topn_by_key",
        "SELECT country, surface, SUM(actions) AS a FROM {T} WHERE event_date = '2026-07-31' GROUP BY country, surface ORDER BY country, surface LIMIT 8",
        true,
    ),
    (
        "offset",
        "SELECT country, SUM(actions) AS a FROM {T} GROUP BY country ORDER BY a DESC, country LIMIT 5 OFFSET 5",
        true,
    ),
    (
        "limit_no_order",
        "SELECT COUNT(*) AS n FROM (SELECT country FROM {T} WHERE event_date = '2026-07-04' LIMIT 100) x",
        false,
    ),
    (
        "union_all",
        "SELECT 'a' AS k, COUNT(*) AS n FROM {T} WHERE region = 'Asia' UNION ALL SELECT 'e', COUNT(*) FROM {T} WHERE region = 'Europe' ORDER BY k",
        true,
    ),
    (
        "subquery_in",
        "SELECT COUNT(*) AS n FROM {T} WHERE country IN (SELECT country FROM {U} WHERE locale = 'ja-JP' GROUP BY country)",
        false,
    ),
    // A correlated EXISTS with a residual (Q21's shape): an event whose
    // user also has an event on the same day from another country, and
    // (NOT EXISTS) with no such event of more actions.
    (
        "exists_residual",
        "SELECT t.country, COUNT(*) AS n FROM {T} t WHERE t.event_date = '2026-07-04' AND EXISTS (SELECT * FROM {T} o WHERE o.user_id = t.user_id AND o.event_date = '2026-07-04' AND o.country <> t.country) GROUP BY t.country ORDER BY n DESC, t.country",
        true,
    ),
    (
        "not_exists_residual",
        "SELECT t.country, COUNT(*) AS n FROM {T} t WHERE t.event_date = '2026-07-04' AND NOT EXISTS (SELECT * FROM {T} o WHERE o.user_id = t.user_id AND o.event_date = '2026-07-04' AND o.country <> t.country AND o.actions > t.actions) GROUP BY t.country ORDER BY n DESC, t.country",
        true,
    ),
    (
        "arith_projection",
        "SELECT country, SUM(actions * 2 + sessions) AS x FROM {T} WHERE event_date = '2026-07-04' GROUP BY country ORDER BY x DESC, country LIMIT 3",
        true,
    ),
    (
        "string_concat",
        "SELECT country || ' / ' || region AS place, COUNT(*) AS n FROM {T} WHERE event_date = '2026-07-04' AND surface = 'Chat' GROUP BY country || ' / ' || region ORDER BY n DESC, place LIMIT 3",
        true,
    ),
    (
        "window_rank",
        "SELECT country, a, RANK() OVER (ORDER BY a DESC) AS r FROM (SELECT country, SUM(actions) AS a FROM {T} WHERE event_date = '2026-07-04' GROUP BY country) x ORDER BY r, country LIMIT 3",
        true,
    ),
    (
        "count_star_filter_only",
        "SELECT COUNT(*) AS n FROM {T} WHERE surface = 'Chat' AND country = 'India' AND event_date >= '2026-07-20'",
        false,
    ),
    (
        "regexp_group_key",
        "SELECT REGEXP_REPLACE(country, '^([A-Z][a-z]+).*$', '$1') AS k, AVG(LENGTH(country)) AS l, COUNT(*) AS n, MIN(country) AS first FROM {T} WHERE country <> '' GROUP BY REGEXP_REPLACE(country, '^([A-Z][a-z]+).*$', '$1') HAVING COUNT(*) > 1000 ORDER BY l DESC, k LIMIT 6",
        true,
    ),
    (
        "regexp_projection",
        "SELECT REGEXP_REPLACE(surface, '[^A-Za-z]+', '_') AS s, region FROM {T} WHERE event_date = '2026-07-04' AND industry IS NULL ORDER BY s, region LIMIT 6",
        true,
    ),
    (
        "regexp_distinct",
        "SELECT DISTINCT REGEXP_REPLACE(platform, 'top|ile', '') AS p FROM {T} ORDER BY p",
        true,
    ),
    // The partitioned layout's keys: `region` and `industry` (nullable)
    // come from the paths there and from the file everywhere else. These
    // shapes fold to a decision on some files and a residual on others.
    (
        "partition_in_and_null",
        "SELECT COUNT(*) AS n FROM {T} WHERE region IN ('Asia', 'Europe') AND industry IS NULL",
        false,
    ),
    (
        "partition_not",
        "SELECT region, COUNT(*) AS n FROM {T} WHERE NOT (region = 'Asia' OR industry = 'Retail') GROUP BY region ORDER BY region",
        true,
    ),
    (
        "partition_range_text",
        "SELECT industry, SUM(actions) AS a FROM {T} WHERE region > 'M' AND region <= 'South America' GROUP BY industry ORDER BY industry",
        true,
    ),
    (
        "partition_keys_only",
        "SELECT region, industry, COUNT(*) AS n FROM {T} GROUP BY region, industry ORDER BY region, industry",
        true,
    ),
    (
        "partition_like_or_file_column",
        "SELECT COUNT(*) AS n FROM {T} WHERE region LIKE 'S%' OR actions > 38",
        false,
    ),
    (
        "partition_between_and_residual",
        "SELECT country, COUNT(*) AS n FROM {T} WHERE region BETWEEN 'Asia' AND 'Europe' AND industry IS NOT NULL AND sessions >= 3 GROUP BY country ORDER BY country",
        true,
    ),
    (
        "partition_ne_and_null_group",
        "SELECT industry, MIN(latency_p75_ms) AS lo FROM {T} WHERE region <> 'Africa' GROUP BY industry ORDER BY industry",
        true,
    ),
];

const SURFACES: [&str; 6] = [
    "API",
    "Chart Builder",
    "Chat",
    "Dashboard",
    "Export",
    "SQL Lab",
];
const PLATFORMS: [&str; 3] = ["Desktop", "Mobile", "Web"];
const LICENSES: [&str; 3] = ["Free", "Pro", "Enterprise"];
const SEGMENTS: [&str; 4] = ["SMB", "Mid-market", "Enterprise", "Public"];
const INDUSTRIES: [&str; 5] = ["Technology", "Government", "Logistics", "Retail", "Finance"];
const REGIONS: [&str; 6] = [
    "Africa",
    "Asia",
    "Europe",
    "North America",
    "Oceania",
    "South America",
];
const COUNTRIES: [(&str, &str); 12] = [
    ("Kenya", "Africa"),
    ("Nigeria", "Africa"),
    ("Japan", "Asia"),
    ("India", "Asia"),
    ("Germany", "Europe"),
    ("United Kingdom", "Europe"),
    ("United States", "North America"),
    ("Canada", "North America"),
    ("Australia", "Oceania"),
    ("Brazil", "South America"),
    ("Argentina", "South America"),
    ("France", "Europe"),
];
const LOCALES: [&str; 5] = ["en-US", "en-GB", "ja-JP", "hi-IN", "de-DE"];
const ROWS: usize = 24_000;
const USERS: i64 = 900;
/// Days since the epoch for 2026-07-01.
const JULY_FIRST_2026: i32 = 20_635;

/// A deterministic mixer: the same rows on every run and machine.
fn mix(seed: u64) -> u64 {
    let mut value = seed.wrapping_mul(0x9E37_79B9_7F4A_7C15);
    value ^= value >> 29;
    value = value.wrapping_mul(0xBF58_476D_1CE4_E5B9);
    value ^ (value >> 32)
}

struct Columns {
    names: Vec<&'static str>,
    text: BTreeMap<&'static str, Vec<Option<String>>>,
    ints: BTreeMap<&'static str, Vec<i64>>,
    dates: Vec<i32>,
}

fn events() -> Columns {
    let mut text: BTreeMap<&'static str, Vec<Option<String>>> = BTreeMap::new();
    let mut ints: BTreeMap<&'static str, Vec<i64>> = BTreeMap::new();
    let mut dates = Vec::with_capacity(ROWS);
    for row in 0..ROWS as u64 {
        let pick = |salt: u64, modulo: usize| (mix(row * 31 + salt) % modulo as u64) as usize;
        let (country, region) = COUNTRIES[pick(1, COUNTRIES.len())];
        text.entry("surface")
            .or_default()
            .push(Some(SURFACES[pick(2, SURFACES.len())].into()));
        text.entry("platform")
            .or_default()
            .push(Some(PLATFORMS[pick(3, PLATFORMS.len())].into()));
        text.entry("license")
            .or_default()
            .push(Some(LICENSES[pick(4, LICENSES.len())].into()));
        text.entry("segment")
            .or_default()
            .push(Some(SEGMENTS[pick(5, SEGMENTS.len())].into()));
        text.entry("industry")
            .or_default()
            .push((pick(6, 9) != 0).then(|| INDUSTRIES[pick(7, INDUSTRIES.len())].into()));
        text.entry("region").or_default().push(Some(region.into()));
        text.entry("country")
            .or_default()
            .push(Some(country.into()));
        ints.entry("user_id")
            .or_default()
            .push(pick(8, USERS as usize) as i64);
        ints.entry("actions").or_default().push(pick(9, 40) as i64);
        ints.entry("sessions")
            .or_default()
            .push(pick(10, 5) as i64 + 1);
        ints.entry("duration_sec")
            .or_default()
            .push(pick(11, 3_000) as i64 + 50);
        ints.entry("latency_p75_ms")
            .or_default()
            .push(pick(12, 2_951) as i64 + 50);
        ints.entry("errors")
            .or_default()
            .push(if pick(13, 20) == 0 {
                pick(14, 4) as i64
            } else {
                0
            });
        ints.entry("queries_run")
            .or_default()
            .push(pick(15, 25) as i64);
        dates.push(JULY_FIRST_2026 + pick(16, 31) as i32);
    }
    assert!(
        REGIONS
            .iter()
            .all(|region| COUNTRIES.iter().any(|(_, r)| r == region))
    );
    Columns {
        names: vec![
            "user_id",
            "surface",
            "platform",
            "license",
            "segment",
            "industry",
            "region",
            "country",
            "event_date",
            "actions",
            "sessions",
            "duration_sec",
            "latency_p75_ms",
            "errors",
            "queries_run",
        ],
        text,
        ints,
        dates,
    }
}

fn users() -> Columns {
    let mut text: BTreeMap<&'static str, Vec<Option<String>>> = BTreeMap::new();
    let mut ints: BTreeMap<&'static str, Vec<i64>> = BTreeMap::new();
    for user in 0..USERS {
        let pick =
            |salt: u64, modulo: usize| (mix(user as u64 * 17 + salt) % modulo as u64) as usize;
        ints.entry("user_id").or_default().push(user);
        text.entry("locale")
            .or_default()
            .push(Some(LOCALES[pick(1, LOCALES.len())].into()));
        text.entry("country")
            .or_default()
            .push(Some(COUNTRIES[pick(2, COUNTRIES.len())].0.into()));
    }
    Columns {
        names: vec!["user_id", "locale", "country"],
        text,
        ints,
        dates: Vec::new(),
    }
}

/// The columns as one batch: text as dictionaries or plain strings.
fn batch(columns: &Columns, dictionary: bool) -> RecordBatch {
    let mut fields = Vec::new();
    let mut arrays: Vec<ArrayRef> = Vec::new();
    for name in &columns.names {
        if let Some(values) = columns.text.get(name) {
            if dictionary {
                let mut builder = StringDictionaryBuilder::<Int32Type>::new();
                for value in values {
                    builder.append_option(value.as_deref());
                }
                fields.push(Field::new(
                    *name,
                    DataType::Dictionary(Box::new(DataType::Int32), Box::new(DataType::Utf8)),
                    true,
                ));
                arrays.push(Arc::new(builder.finish()));
            } else {
                fields.push(Field::new(*name, DataType::Utf8, true));
                arrays.push(Arc::new(StringArray::from_iter(
                    values.iter().map(|v| v.as_deref()),
                )));
            }
        } else if let Some(values) = columns.ints.get(name) {
            fields.push(Field::new(*name, DataType::Int64, false));
            arrays.push(Arc::new(Int64Array::from(values.clone())));
        } else {
            assert_eq!(*name, "event_date");
            fields.push(Field::new(*name, DataType::Date32, false));
            arrays.push(Arc::new(Date32Array::from(columns.dates.clone())));
        }
    }
    RecordBatch::try_new(Arc::new(Schema::new(fields)), arrays).unwrap()
}

fn write(directory: &std::path::Path, file: &str, batch: &RecordBatch) -> TableMeta {
    let path = directory.join(file);
    let properties = WriterProperties::builder()
        .set_max_row_group_size(4_096)
        .build();
    let mut writer = ArrowWriter::try_new(
        File::create(&path).unwrap(),
        batch.schema(),
        Some(properties),
    )
    .unwrap();
    writer.write(batch).unwrap();
    writer.close().unwrap();
    TableMeta {
        name: file.trim_end_matches(".parquet").to_owned(),
        arrow_schema: batch.schema(),
        location: file.to_owned(),
        access: AccessPattern::Shortcut,
        format: DataFormat::Parquet,
    }
}

/// The batch as a directory table: `parts` files of consecutive rows, plus
/// the marker and hidden files a writer leaves behind.
fn write_directory(
    directory: &std::path::Path,
    name: &str,
    batch: &RecordBatch,
    parts: usize,
) -> TableMeta {
    let table = directory.join(name);
    std::fs::create_dir_all(&table).unwrap();
    let rows_per_part = batch.num_rows().div_ceil(parts);
    for part in 0..parts {
        let offset = part * rows_per_part;
        let length = rows_per_part.min(batch.num_rows() - offset);
        write(
            &table,
            &format!("part-{part:05}.parquet"),
            &batch.slice(offset, length),
        );
    }
    std::fs::write(table.join("_SUCCESS"), b"").unwrap();
    TableMeta {
        name: name.to_owned(),
        arrow_schema: batch.schema(),
        location: name.to_owned(),
        access: AccessPattern::Shortcut,
        format: DataFormat::Parquet,
    }
}

/// The batch as a Hive-partitioned directory table: one file per
/// combination of the `keys` values, under `key=value` directories (a
/// NULL under Hive's default partition), the key columns absent from the
/// files. The catalog schema is what the directory reader serves: the file
/// columns, then the keys as read from the paths.
fn write_partitioned(
    directory: &std::path::Path,
    name: &str,
    batch: &RecordBatch,
    keys: &[&str],
) -> TableMeta {
    let table = directory.join(name);
    std::fs::create_dir_all(&table).unwrap();
    let key_columns = keys
        .iter()
        .map(|key| {
            cast(batch.column_by_name(key).unwrap(), &DataType::Utf8)
                .unwrap()
                .as_any()
                .downcast_ref::<StringArray>()
                .unwrap()
                .clone()
        })
        .collect::<Vec<_>>();
    let file_indices = (0..batch.num_columns())
        .filter(|index| !keys.contains(&batch.schema().field(*index).name().as_str()))
        .collect::<Vec<_>>();
    let mut groups: BTreeMap<Vec<Option<String>>, Vec<u32>> = BTreeMap::new();
    for row in 0..batch.num_rows() {
        let values = key_columns
            .iter()
            .map(|column| column.is_valid(row).then(|| column.value(row).to_owned()))
            .collect::<Vec<_>>();
        groups.entry(values).or_default().push(row as u32);
    }
    for (values, rows) in groups {
        let mut partition = table.clone();
        for (key, value) in keys.iter().zip(&values) {
            partition.push(format!(
                "{key}={}",
                value
                    .as_deref()
                    .unwrap_or(kaveon_storage::HIVE_DEFAULT_PARTITION)
            ));
        }
        std::fs::create_dir_all(&partition).unwrap();
        let rows = arrow::compute::take(
            &arrow::array::StructArray::from(batch.project(&file_indices).unwrap()),
            &arrow::array::UInt32Array::from(rows),
            None,
        )
        .unwrap();
        let rows = RecordBatch::from(
            rows.as_any()
                .downcast_ref::<arrow::array::StructArray>()
                .unwrap(),
        );
        write(&partition, "part-00000.parquet", &rows);
    }
    std::fs::write(table.join("_SUCCESS"), b"").unwrap();
    let schema = kaveon_storage::ParquetReader::new(&table)
        .metadata()
        .unwrap()
        .schema;
    TableMeta {
        name: name.to_owned(),
        arrow_schema: schema,
        location: name.to_owned(),
        access: AccessPattern::Shortcut,
        format: DataFormat::Parquet,
    }
}

/// The batch with `keys` moved to the end, in the order given: the column
/// order a partitioned directory table presents.
fn keys_last(batch: &RecordBatch, keys: &[&str]) -> RecordBatch {
    let mut indices = (0..batch.num_columns())
        .filter(|index| !keys.contains(&batch.schema().field(*index).name().as_str()))
        .collect::<Vec<_>>();
    indices.extend(keys.iter().map(|key| batch.schema().index_of(key).unwrap()));
    batch.project(&indices).unwrap()
}

/// Rows as text, one string per row, so both encodings, both layouts and
/// both paths compare alike.
pub(crate) fn canonical_rows(batches: &[RecordBatch], ordered: bool) -> Vec<String> {
    let mut rows = Vec::new();
    for batch in batches {
        let columns = batch
            .columns()
            .iter()
            .map(|column| cast(column, &DataType::Utf8).unwrap())
            .collect::<Vec<_>>();
        for row in 0..batch.num_rows() {
            let cells = columns
                .iter()
                .map(|column| {
                    let column = column.as_any().downcast_ref::<StringArray>().unwrap();
                    if column.is_null(row) {
                        "NULL".to_owned()
                    } else {
                        column.value(row).to_owned()
                    }
                })
                .collect::<Vec<_>>();
            rows.push(cells.join("\u{1f}"));
        }
    }
    if !ordered {
        rows.sort();
    }
    rows
}

fn drain(operator: &mut dyn BatchOperator) -> Vec<RecordBatch> {
    let mut batches = Vec::new();
    while let Some(batch) = operator.next_batch().unwrap() {
        batches.push(batch);
    }
    batches
}

#[test]
fn the_differential_sweep_matches_across_parquet_encodings_and_execution_paths() {
    let directory =
        std::env::temp_dir().join(format!("kaveon-differential-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&directory).unwrap();
    let events = events();
    let users = users();
    let mut catalog = MemoryCatalog::new(
        "lake",
        StorageType::Local {
            base_path: PathBuf::from(&directory),
        },
    )
    .with_schema("events");
    for (file, columns, dictionary) in [
        ("events_dictionary.parquet", &events, true),
        ("events_plain.parquet", &events, false),
        ("users.parquet", &users, true),
    ] {
        let meta = write(&directory, file, &batch(columns, dictionary));
        catalog.register_table("events", meta).unwrap();
    }
    let parts = write_directory(&directory, "events_parts", &batch(&events, false), 3);
    catalog.register_table("events", parts).unwrap();
    let partition_keys = ["region", "industry"];
    let partitioned = write_partitioned(
        &directory,
        "events_partitioned",
        &batch(&events, false),
        &partition_keys,
    );
    assert_eq!(
        partitioned
            .arrow_schema
            .fields()
            .iter()
            .rev()
            .take(2)
            .map(|field| field.name().as_str())
            .collect::<Vec<_>>(),
        ["industry", "region"]
    );
    catalog.register_table("events", partitioned).unwrap();
    let hive_order = write(
        &directory,
        "events_hive_order.parquet",
        &keys_last(&batch(&events, false), &partition_keys),
    );
    catalog.register_table("events", hive_order).unwrap();
    let mut manager = CatalogManager::new("lake", "events");
    manager.register_catalog(Box::new(catalog));

    let optimized = |statement: &str| -> LogicalPlan {
        let mut plan = kaveon_sql::logical_plan::sql_to_logical_plan_for_binder(statement).unwrap();
        crate::planner::qualify_tables(&mut plan, "lake", "events");
        let plan = kaveon_optim::binder::bind(plan, &manager)
            .unwrap_or_else(|error| panic!("{statement}: {error}"));
        let plan = kaveon_optim::rules::push_filter_down(plan);
        kaveon_optim::rules::push_projection_down(plan)
    };
    let local = |statement: &str, ordered: bool| -> Vec<String> {
        let plan = optimized(statement);
        let pool = QueryMemoryPool::new("differential", 256 * 1024 * 1024).unwrap();
        let mut planned = crate::planner::plan_query_with_memory(&plan, &manager, &pool)
            .unwrap_or_else(|error| panic!("{statement}: {error}"));
        let rows = canonical_rows(&drain(planned.operator.as_mut()), ordered);
        drop(planned);
        assert_eq!(
            pool.snapshot().current_bytes,
            0,
            "{statement} leaked reservations"
        );
        rows
    };
    let distributed = |statement: &str, ordered: bool| -> Vec<String> {
        let plan = optimized(statement);
        let pool = QueryMemoryPool::new("differential-distributed", 256 * 1024 * 1024).unwrap();
        let batches = execute_distributed("differential", &plan, &manager, 2, &pool)
            .unwrap_or_else(|error| panic!("{statement} (distributed): {error}"));
        assert_eq!(
            pool.snapshot().current_bytes,
            0,
            "{statement} (distributed) leaked reservations"
        );
        canonical_rows(&batches, ordered)
    };
    let mut mismatches = Vec::new();
    for (name, template, ordered) in CASES {
        let statement = |table: &str| template.replace("{T}", table).replace("{U}", "users");
        let dictionary = local(&statement("events_dictionary"), *ordered);
        let plain = local(&statement("events_plain"), *ordered);
        let parts = local(&statement("events_parts"), *ordered);
        let fragments = distributed(&statement("events_dictionary"), *ordered);
        let partitioned = local(&statement("events_partitioned"), *ordered);
        let hive_order = local(&statement("events_hive_order"), *ordered);
        let partitioned_fragments = distributed(&statement("events_partitioned"), *ordered);
        assert!(!dictionary.is_empty(), "{name} returned no rows");
        if dictionary != plain {
            mismatches.push(format!(
                "{name}: dictionary {:?} versus plain {:?}",
                dictionary.iter().take(3).collect::<Vec<_>>(),
                plain.iter().take(3).collect::<Vec<_>>()
            ));
        }
        if parts != plain {
            mismatches.push(format!(
                "{name}: directory of three files {:?} versus one file {:?}",
                parts.iter().take(3).collect::<Vec<_>>(),
                plain.iter().take(3).collect::<Vec<_>>()
            ));
        }
        if dictionary != fragments {
            mismatches.push(format!(
                "{name}: local {:?} versus distributed {:?}",
                dictionary.iter().take(3).collect::<Vec<_>>(),
                fragments.iter().take(3).collect::<Vec<_>>()
            ));
        }
        if partitioned != hive_order {
            mismatches.push(format!(
                "{name}: partitioned directory {:?} versus one file in its column order {:?}",
                partitioned.iter().take(3).collect::<Vec<_>>(),
                hive_order.iter().take(3).collect::<Vec<_>>()
            ));
        }
        if partitioned != partitioned_fragments {
            mismatches.push(format!(
                "{name}: partitioned directory local {:?} versus distributed {:?}",
                partitioned.iter().take(3).collect::<Vec<_>>(),
                partitioned_fragments.iter().take(3).collect::<Vec<_>>()
            ));
        }
    }
    let _ = std::fs::remove_dir_all(&directory);
    assert!(mismatches.is_empty(), "{}", mismatches.join("\n"));
}
