//! The differential sweep as a gate: the statements of
//! `scripts/differential-cases.py`, run against the same rows held in two
//! Parquet encodings — an Arrow dictionary schema for every text column,
//! and plain UTF-8 — through the node-local planner, again against the
//! plain rows held as one file and as a directory of three files, again
//! against the same rows held as a Hive-partitioned directory
//! (`region=…/industry=…/`, the keys read from the paths) and as one file
//! of the same column order, again against the same rows as a clustered
//! directory (`ClusteredParquetWriter`: sorted by country and event date,
//! small row groups, page index, Bloom filters), and through the
//! distributed planner's fragments executed in this process as a
//! two-worker cluster would run them — for the dictionary file, the
//! partitioned directory and the clustered directory. Any divergence is an Engine defect, whatever the
//! cluster later says. The encodings exercise different operator paths
//! (dictionary-aware predicates, coded folds, the columnar aggregate's
//! arena keys) against one truth; the layouts exercise the directory
//! reader (listing, file assignment, per-file pruning, partition columns
//! and partition pruning) and the clustered layout's pruning (row groups a
//! filter skips, pages the offset index skips) against the single-file
//! reader; the fragment path exercises the stage planner, the exchanges
//! and the fragment compiler.
//!
//! A second sweep holds the cube to the row path: every statement the
//! cube answers over the events directory (declared shape, cube built by
//! the storage builder) equals the scanned answer — exactly for the row
//! count and the additive measures, bit-identically for the distinct
//! sketches against `APPROX_COUNT_DISTINCT`, and within the sketch's
//! error against the exact `COUNT(DISTINCT)`.
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
use kaveon_storage::{
    ClusteredParquetWriter, ClusteringLayout, FileNaming, LocalDirectorySink, ScanPartition,
};
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
    execute_fragments(&graph, &fragments, manager, pool).map(|run| run.batches)
}

/// What an in-process distributed run hands back: the root stage's rows
/// and every task's scan metrics, in execution order.
pub(crate) struct DistributedRun {
    pub batches: Vec<RecordBatch>,
    pub scan_metrics: Vec<kaveon_storage::ScanMetrics>,
}

/// [`execute_distributed`] over fragments already built — for a test that
/// changes the table between planning and execution, or builds them under
/// other options.
pub(crate) fn execute_fragments(
    graph: &kaveon_core::StageGraph,
    fragments: &BTreeMap<StageId, kaveon_core::ExecutableFragment>,
    manager: &CatalogManager,
    pool: &QueryMemoryPool,
) -> Result<DistributedRun> {
    let mut produced: HashMap<ExchangeId, Vec<ExchangeOutputBatches>> = HashMap::new();
    let mut done: Vec<StageId> = Vec::new();
    let mut results = Vec::new();
    let mut scan_metrics = Vec::new();
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
            scan_metrics.extend(execution.scan_metrics);
        }
        done.push(stage.id);
    }
    Ok(DistributedRun {
        batches: results,
        scan_metrics,
    })
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

/// Approximate cases: (name, statement, ordered, tolerance). Cells that
/// parse as numbers compare within `tolerance` of each other (relative to
/// the larger magnitude, absolute below one); lists element by element;
/// everything else exactly. HyperLogLog registers merge exactly, so a
/// distinct count is the same on every path and its tolerance is zero;
/// a KLL sketch merged from partials compacts differently from one built
/// in sequence, so a percentile's paths agree only within its rank error
/// — a tenth of the value over these uniform columns.
const APPROXIMATE_CASES: &[(&str, &str, bool, f64)] = &[
    (
        "approx_distinct_global",
        "SELECT APPROX_COUNT_DISTINCT(user_id) AS u, APPROX_DISTINCT(country) AS c, COUNT(DISTINCT user_id) AS exact FROM {T}",
        false,
        0.0,
    ),
    (
        "approx_distinct_grouped_and_filtered",
        "SELECT region, APPROX_COUNT_DISTINCT(user_id) AS u, COUNT(*) AS n FROM {T} WHERE actions > 5 GROUP BY region ORDER BY region",
        true,
        0.0,
    ),
    (
        "approx_percentile_grouped",
        "SELECT platform, APPROX_PERCENTILE(latency_p75_ms, 0.5) AS p50, APPROX_PERCENTILE(latency_p75_ms, ARRAY[0.9, 0.99]) AS tail FROM {T} GROUP BY platform ORDER BY platform",
        true,
        0.1,
    ),
    (
        "approx_percentile_global_expression",
        "SELECT APPROX_PERCENTILE(duration_sec, 0.25) + 1 AS q1 FROM {T}",
        false,
        0.1,
    ),
];

/// Whether two canonical rows differ beyond `tolerance` (see
/// [`APPROXIMATE_CASES`]); zero tolerance is the exact comparison.
fn rows_differ(left: &[String], right: &[String], tolerance: f64) -> bool {
    if tolerance == 0.0 {
        return left != right;
    }
    if left.len() != right.len() {
        return true;
    }
    left.iter().zip(right).any(|(left, right)| {
        let (left, right) = (left.split('\u{1f}'), right.split('\u{1f}'));
        left.zip(right).any(|(a, b)| {
            if a == b {
                return false;
            }
            let numbers = |cell: &str| -> Option<Vec<f64>> {
                cell.trim_matches(|c| c == '[' || c == ']')
                    .split(',')
                    .map(|item| item.trim().parse::<f64>().ok())
                    .collect()
            };
            match (numbers(a), numbers(b)) {
                (Some(a), Some(b)) if a.len() == b.len() => a
                    .iter()
                    .zip(&b)
                    .any(|(a, b)| (a - b).abs() > tolerance * a.abs().max(b.abs()).max(1.0)),
                _ => true,
            }
        })
    })
}

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

/// The batch as a clustered directory table: the rows sorted by
/// `clustered_by`, written by the clustered writer in row groups of 4 096
/// rows and files of at most 64 KiB, with Bloom filters on `bloom`.
fn write_clustered(
    directory: &std::path::Path,
    name: &str,
    batch: &RecordBatch,
    clustered_by: &[&str],
    bloom: &[&str],
) -> TableMeta {
    let table = directory.join(name);
    let columns = clustered_by
        .iter()
        .map(|column| arrow::compute::SortColumn {
            values: Arc::clone(batch.column(batch.schema().index_of(column).unwrap())),
            options: Some(arrow::compute::SortOptions {
                descending: false,
                nulls_first: false,
            }),
        })
        .collect::<Vec<_>>();
    let indices = arrow::compute::lexsort_to_indices(&columns, None).unwrap();
    let sorted = RecordBatch::try_new(
        batch.schema(),
        batch
            .columns()
            .iter()
            .map(|column| arrow::compute::take(column, &indices, None).unwrap())
            .collect(),
    )
    .unwrap();
    let layout = ClusteringLayout::new(
        clustered_by
            .iter()
            .map(|column| (*column).to_owned())
            .collect(),
        bloom.iter().map(|column| (*column).to_owned()).collect(),
    )
    .with_max_row_group_rows(4_096)
    .with_target_file_bytes(Some(64 * 1024));
    let mut writer = ClusteredParquetWriter::new(
        layout,
        batch.schema(),
        FileNaming::Parts {
            prefix: "part-clustered".into(),
        },
        Box::new(LocalDirectorySink::new(&table).unwrap()),
    )
    .unwrap();
    for offset in (0..sorted.num_rows()).step_by(1_000) {
        writer
            .write(&sorted.slice(offset, 1_000.min(sorted.num_rows() - offset)))
            .unwrap();
    }
    let files = writer.finish().unwrap();
    assert!(
        files.len() > 1,
        "the clustered layout spans files: {files:?}"
    );
    TableMeta {
        name: name.to_owned(),
        arrow_schema: batch.schema(),
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

/// Rows as text, one string per row, so both encodings, every layout and
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
    let clustered = write_clustered(
        &directory,
        "events_clustered",
        &batch(&events, false),
        &["country", "event_date"],
        &["user_id"],
    );
    catalog.register_table("events", clustered).unwrap();
    let mut manager = CatalogManager::new("lake", "events");
    manager.register_catalog(Box::new(catalog));

    let optimized_as = |statement: &str, approximate: bool| -> LogicalPlan {
        let mut plan = if approximate {
            kaveon_sql::logical_plan::sql_to_logical_plan_for_binder_approximate(statement)
        } else {
            kaveon_sql::logical_plan::sql_to_logical_plan_for_binder(statement)
        }
        .unwrap();
        crate::planner::qualify_tables(&mut plan, "lake", "events");
        let plan = kaveon_optim::binder::bind(plan, &manager)
            .unwrap_or_else(|error| panic!("{statement}: {error}"));
        let plan = kaveon_optim::rules::push_filter_down(plan);
        kaveon_optim::rules::push_projection_down(plan)
    };
    let optimized = |statement: &str| optimized_as(statement, false);
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
    let cases = CASES
        .iter()
        .map(|(name, template, ordered)| (*name, *template, *ordered, 0.0))
        .chain(APPROXIMATE_CASES.iter().copied())
        .collect::<Vec<_>>();
    for (name, template, ordered, tolerance) in &cases {
        let statement = |table: &str| template.replace("{T}", table).replace("{U}", "users");
        let dictionary = local(&statement("events_dictionary"), *ordered);
        let plain = local(&statement("events_plain"), *ordered);
        let parts = local(&statement("events_parts"), *ordered);
        let clustered = local(&statement("events_clustered"), *ordered);
        let fragments = distributed(&statement("events_dictionary"), *ordered);
        let partitioned = local(&statement("events_partitioned"), *ordered);
        let hive_order = local(&statement("events_hive_order"), *ordered);
        let partitioned_fragments = distributed(&statement("events_partitioned"), *ordered);
        let clustered_fragments = distributed(&statement("events_clustered"), *ordered);
        assert!(!dictionary.is_empty(), "{name} returned no rows");
        if rows_differ(&dictionary, &plain, *tolerance) {
            mismatches.push(format!(
                "{name}: dictionary {:?} versus plain {:?}",
                dictionary.iter().take(3).collect::<Vec<_>>(),
                plain.iter().take(3).collect::<Vec<_>>()
            ));
        }
        if rows_differ(&parts, &plain, *tolerance) {
            mismatches.push(format!(
                "{name}: directory of three files {:?} versus one file {:?}",
                parts.iter().take(3).collect::<Vec<_>>(),
                plain.iter().take(3).collect::<Vec<_>>()
            ));
        }
        if rows_differ(&clustered, &plain, *tolerance) {
            mismatches.push(format!(
                "{name}: clustered layout {:?} versus one file {:?}",
                clustered.iter().take(3).collect::<Vec<_>>(),
                plain.iter().take(3).collect::<Vec<_>>()
            ));
        }
        if rows_differ(&dictionary, &fragments, *tolerance) {
            mismatches.push(format!(
                "{name}: local {:?} versus distributed {:?}",
                dictionary.iter().take(3).collect::<Vec<_>>(),
                fragments.iter().take(3).collect::<Vec<_>>()
            ));
        }
        if rows_differ(&partitioned, &hive_order, *tolerance) {
            mismatches.push(format!(
                "{name}: partitioned directory {:?} versus one file in its column order {:?}",
                partitioned.iter().take(3).collect::<Vec<_>>(),
                hive_order.iter().take(3).collect::<Vec<_>>()
            ));
        }
        if rows_differ(&partitioned, &partitioned_fragments, *tolerance) {
            mismatches.push(format!(
                "{name}: partitioned directory local {:?} versus distributed {:?}",
                partitioned.iter().take(3).collect::<Vec<_>>(),
                partitioned_fragments.iter().take(3).collect::<Vec<_>>()
            ));
        }
        if rows_differ(&clustered, &clustered_fragments, *tolerance) {
            mismatches.push(format!(
                "{name}: clustered layout local {:?} versus distributed {:?}",
                clustered.iter().take(3).collect::<Vec<_>>(),
                clustered_fragments.iter().take(3).collect::<Vec<_>>()
            ));
        }
    }
    // The `approximate` setting: a plain COUNT(DISTINCT) rewritten to a
    // sketch answers what APPROX_COUNT_DISTINCT answers, under COUNT's
    // output name, on both paths.
    {
        let sql = "SELECT region, COUNT(DISTINCT user_id) AS u, COUNT(DISTINCT country) FROM events_dictionary GROUP BY region ORDER BY region";
        let approx_sql = "SELECT region, APPROX_COUNT_DISTINCT(user_id) AS u, APPROX_COUNT_DISTINCT(country) FROM events_dictionary GROUP BY region ORDER BY region";
        let plan = optimized_as(sql, true);
        assert_eq!(plan.aggregates().len(), 2);
        assert!(
            plan.aggregates()
                .iter()
                .all(|aggregate| aggregate.is_approximate())
        );
        let pool = QueryMemoryPool::new("differential-approximate", 256 * 1024 * 1024).unwrap();
        let mut planned = crate::planner::plan_query_with_memory(&plan, &manager, &pool).unwrap();
        assert_eq!(
            planned
                .operator
                .schema()
                .fields()
                .iter()
                .map(|field| field.name().as_str())
                .collect::<Vec<_>>(),
            ["region", "u", "count_country"]
        );
        let rewritten = canonical_rows(&drain(planned.operator.as_mut()), true);
        drop(planned);
        assert_eq!(rewritten, local(approx_sql, true));
        let batches =
            execute_distributed("differential-approximate", &plan, &manager, 2, &pool).unwrap();
        assert_eq!(canonical_rows(&batches, true), rewritten);
        assert_eq!(pool.snapshot().current_bytes, 0);
        // Exact and approximate counts of 900 users agree within the
        // sketch's standard error, and the sketch is not the exact count.
        let exact = local(sql, true);
        let error = kaveon_core::HllSketch::default_precision().standard_error();
        assert!(rows_differ(&exact, &rewritten, 0.0));
        assert!(!rows_differ(&exact, &rewritten, 3.0 * error));
    }
    let _ = std::fs::remove_dir_all(&directory);
    assert!(mismatches.is_empty(), "{}", mismatches.join("\n"));
}

/// A directory table of `id` files — one file per entry of `files`, each
/// holding that many consecutive ids from where the previous ended, in
/// row groups of 4 096 — registered as `lake.events.ids`.
fn ids_directory_catalog(
    directory: &std::path::Path,
    files: &[usize],
) -> (CatalogManager, Vec<i64>) {
    let table = directory.join("ids");
    std::fs::create_dir_all(&table).unwrap();
    let mut next = 0_i64;
    let mut all = Vec::new();
    for (index, rows) in files.iter().enumerate() {
        let ids = (next..next + *rows as i64).collect::<Vec<_>>();
        next += *rows as i64;
        all.extend(ids.iter().copied());
        write(
            &table,
            &format!("part-{index:05}.parquet"),
            &ids_batch(&ids),
        );
    }
    let schema = kaveon_storage::ParquetReader::new(&table)
        .metadata()
        .unwrap()
        .schema;
    let mut catalog = MemoryCatalog::new(
        "lake",
        StorageType::Local {
            base_path: PathBuf::from(directory),
        },
    )
    .with_schema("events");
    catalog
        .register_table(
            "events",
            TableMeta {
                name: "ids".to_owned(),
                arrow_schema: schema,
                location: "ids".to_owned(),
                access: AccessPattern::Shortcut,
                format: DataFormat::Parquet,
            },
        )
        .unwrap();
    let mut manager = CatalogManager::new("lake", "events");
    manager.register_catalog(Box::new(catalog));
    (manager, all)
}

fn ids_batch(ids: &[i64]) -> RecordBatch {
    RecordBatch::try_new(
        Arc::new(Schema::new(vec![Field::new("id", DataType::Int64, false)])),
        vec![Arc::new(Int64Array::from(ids.to_vec())) as ArrayRef],
    )
    .unwrap()
}

/// The scan listing the one scan fragment of `fragments` carries.
fn scan_listing(
    fragments: &BTreeMap<StageId, kaveon_core::ExecutableFragment>,
) -> kaveon_core::ScanListing {
    fragments
        .values()
        .flat_map(|fragment| &fragment.nodes)
        .find_map(|node| match &node.operator {
            kaveon_core::FragmentOperator::Scan(scan) => scan.listing.clone(),
            _ => None,
        })
        .expect("the scan carries its listing")
}

fn ids_of(batches: &[RecordBatch]) -> Vec<i64> {
    let mut ids = batches
        .iter()
        .flat_map(|batch| {
            batch
                .column(0)
                .as_any()
                .downcast_ref::<Int64Array>()
                .unwrap()
                .values()
                .to_vec()
        })
        .collect::<Vec<_>>();
    ids.sort_unstable();
    ids
}

/// The listing travels with the plan: the fragment carries the
/// coordinator's listing assigned to the two scan partitions, every task
/// reads exactly its files — a file added after planning is read by no
/// task, and the three files are opened three times in all — and the
/// rows are the planned table's. Without the listing (a fragment of the
/// previous wire version) the tasks list for themselves and read the
/// added file too.
#[test]
fn the_listing_travels_with_the_plan() {
    let directory = std::env::temp_dir().join(format!("kaveon-listing-{}", uuid::Uuid::new_v4()));
    let (manager, planned) = ids_directory_catalog(&directory, &[300, 300, 300]);
    let pool = QueryMemoryPool::new("listing", 64 * 1024 * 1024).unwrap();
    let statement = "SELECT id FROM ids WHERE id >= 0";
    let plan = bound_plan(statement, &manager);
    let graph = crate::planner::build_stage_graph("listing", &plan, 2).unwrap();
    let fragments =
        crate::planner::build_executable_fragments("listing", &plan, &manager, 2).unwrap();
    let listing = scan_listing(&fragments);
    assert_eq!(listing.source.files, 3);
    assert_eq!(listing.files_pruned_by_partition, 0);
    assert_eq!(listing.files_skipped, 0);
    assert!(listing.partition_columns.is_empty());
    let assignment = listing.assignment.as_ref().expect("the files are assigned");
    assert_eq!(assignment.partitions.len(), 2);
    assert_eq!(assignment.first.path, "part-00000.parquet");
    let mut assigned = assignment
        .partitions
        .iter()
        .flatten()
        .map(|file| file.path.clone())
        .collect::<Vec<_>>();
    assigned.sort();
    assert_eq!(
        assigned,
        [
            "part-00000.parquet",
            "part-00001.parquet",
            "part-00002.parquet"
        ],
        "every file is read by exactly one task: {assignment:?}"
    );
    // Three equal files over two partitions: one is split by row group,
    // and with one row group only one partition receives it.
    assert!(
        assignment
            .partitions
            .iter()
            .flatten()
            .all(|file| file.row_groups.as_ref().is_none_or(|groups| groups == &[0])),
        "{assignment:?}"
    );

    // A file lands after planning.
    write(
        &directory.join("ids"),
        "part-00003.parquet",
        &ids_batch(&(900..1200).collect::<Vec<_>>()),
    );
    let run = execute_fragments(&graph, &fragments, &manager, &pool).unwrap();
    assert_eq!(ids_of(&run.batches), planned);
    let opened = run
        .scan_metrics
        .iter()
        .map(|metrics| metrics.snapshot().files_opened)
        .sum::<u64>();
    assert_eq!(opened, 3, "each file once");
    let considered = run
        .scan_metrics
        .iter()
        .map(|metrics| metrics.snapshot().files_considered)
        .sum::<u64>();
    assert_eq!(considered, 3);

    // The previous wire version: no listing, the tasks list now.
    let older = fragments
        .iter()
        .map(|(stage, fragment)| {
            let mut json = serde_json::to_value(fragment).unwrap();
            json["version"] = serde_json::json!(kaveon_core::OLDEST_EXECUTABLE_FRAGMENT_VERSION);
            for node in json["nodes"].as_array_mut().unwrap() {
                if let Some(operator) = node["operator"].as_object_mut() {
                    operator.remove("listing");
                }
            }
            (*stage, serde_json::from_value(json).unwrap())
        })
        .collect::<BTreeMap<StageId, kaveon_core::ExecutableFragment>>();
    assert!(scan_listing_absent(&older));
    let run = execute_fragments(&graph, &older, &manager, &pool).unwrap();
    assert_eq!(ids_of(&run.batches), (0..1200).collect::<Vec<_>>());
    let _ = std::fs::remove_dir_all(&directory);
}

fn scan_listing_absent(fragments: &BTreeMap<StageId, kaveon_core::ExecutableFragment>) -> bool {
    fragments
        .values()
        .flat_map(|fragment| &fragment.nodes)
        .all(|node| match &node.operator {
            kaveon_core::FragmentOperator::Scan(scan) => scan.listing.is_none(),
            _ => true,
        })
}

/// One large file beside two small ones is split by row group across the
/// two partitions: the fragment carries each task's row groups, they are
/// disjoint and together the whole file, and the rows come back once.
#[test]
fn a_split_file_travels_as_row_groups() {
    let directory =
        std::env::temp_dir().join(format!("kaveon-listing-split-{}", uuid::Uuid::new_v4()));
    let (manager, planned) = ids_directory_catalog(&directory, &[20_000, 100, 100]);
    let pool = QueryMemoryPool::new("listing-split", 64 * 1024 * 1024).unwrap();
    let plan = bound_plan("SELECT id FROM ids", &manager);
    let graph = crate::planner::build_stage_graph("split", &plan, 2).unwrap();
    let fragments =
        crate::planner::build_executable_fragments("split", &plan, &manager, 2).unwrap();
    let listing = scan_listing(&fragments);
    let assignment = listing.assignment.as_ref().unwrap();
    let large = |partition: usize| {
        assignment.partitions[partition]
            .iter()
            .find(|file| file.path == "part-00000.parquet")
            .and_then(|file| file.row_groups.clone())
            .expect("the large file is split")
    };
    let (first, second) = (large(0), large(1));
    assert_eq!(first, [0, 2, 4]);
    assert_eq!(second, [1, 3]);
    for partition in &assignment.partitions {
        assert_eq!(partition.len(), 2, "{partition:?}");
    }
    let run = execute_fragments(&graph, &fragments, &manager, &pool).unwrap();
    assert_eq!(ids_of(&run.batches), planned);
    let opened = run
        .scan_metrics
        .iter()
        .map(|metrics| metrics.snapshot().files_opened)
        .sum::<u64>();
    assert_eq!(opened, 4, "the split file is opened by both tasks");
    let selected = run
        .scan_metrics
        .iter()
        .map(|metrics| metrics.snapshot().row_groups_selected)
        .sum::<u64>();
    assert_eq!(selected, 7);
    let _ = std::fs::remove_dir_all(&directory);
}

/// A listing over the fragment listing limit travels as its digest: each
/// task lists the location itself and reads at that listing when it is
/// the coordinator's; a file added after planning fails the task with the
/// two counts.
#[test]
fn a_listing_over_the_limit_travels_as_its_digest() {
    let directory =
        std::env::temp_dir().join(format!("kaveon-listing-digest-{}", uuid::Uuid::new_v4()));
    let (manager, planned) = ids_directory_catalog(&directory, &[300, 300, 300]);
    let pool = QueryMemoryPool::new("listing-digest", 64 * 1024 * 1024).unwrap();
    let plan = bound_plan("SELECT id FROM ids WHERE id < 5000", &manager);
    let graph = crate::planner::build_stage_graph("digest", &plan, 2).unwrap();
    let fragments = crate::planner::build_executable_fragments_with_options(
        "digest",
        &plan,
        &manager,
        2,
        &crate::planner::SourcePins::default(),
        &crate::planner::FragmentBuildOptions {
            listing_max_files: 2,
        },
    )
    .unwrap();
    let listing = scan_listing(&fragments);
    assert!(listing.assignment.is_none());
    assert_eq!(listing.source.files, 3);
    assert_eq!(listing.source.sha256.len(), 64);
    let run = execute_fragments(&graph, &fragments, &manager, &pool).unwrap();
    assert_eq!(ids_of(&run.batches), planned);

    write(
        &directory.join("ids"),
        "part-00003.parquet",
        &ids_batch(&(900..1200).collect::<Vec<_>>()),
    );
    let error = execute_fragments(&graph, &fragments, &manager, &pool)
        .err()
        .expect("a changed listing fails the task")
        .to_string();
    assert!(
        error.contains("lists 4 files on this node where the coordinator listed 3")
            && error.contains("1 more"),
        "{error}"
    );
    let _ = std::fs::remove_dir_all(&directory);
}

/// The events file as a one-table catalog for a targeted differential.
fn events_catalog(directory: &std::path::Path) -> CatalogManager {
    std::fs::create_dir_all(directory).unwrap();
    let mut catalog = MemoryCatalog::new(
        "lake",
        StorageType::Local {
            base_path: PathBuf::from(directory),
        },
    )
    .with_schema("events");
    let meta = write(directory, "events.parquet", &batch(&events(), true));
    catalog.register_table("events", meta).unwrap();
    let mut manager = CatalogManager::new("lake", "events");
    manager.register_catalog(Box::new(catalog));
    manager
}

fn bound_plan(statement: &str, manager: &CatalogManager) -> LogicalPlan {
    let mut plan = kaveon_sql::logical_plan::sql_to_logical_plan_for_binder(statement).unwrap();
    crate::planner::qualify_tables(&mut plan, "lake", "events");
    let plan = kaveon_optim::binder::bind(plan, manager)
        .unwrap_or_else(|error| panic!("{statement}: {error}"));
    let plan = kaveon_optim::rules::push_filter_down(plan);
    kaveon_optim::rules::push_projection_down(plan)
}

/// The grouped partial that stops aggregating hands the final the same
/// answer: each statement through the fragments with a budget whose
/// flush round is one scan batch and the adaptive rule judging every
/// round, against the same fragments with the rule off. The near-unique
/// shapes (the q19 shape; a `COUNT(DISTINCT)` on the row path) pass
/// rows through — the task metrics say so — and the low-cardinality
/// shape never does.
#[test]
fn the_pass_through_partial_matches_the_aggregating_partial() {
    use kaveon_exec::partitioned::AdaptivePartialSettings;
    let directory =
        std::env::temp_dir().join(format!("kaveon-passthrough-{}", uuid::Uuid::new_v4()));
    let manager = events_catalog(&directory);
    // (statement, whether its output is ordered, whether its key is
    // near-unique, the query budget in MiB). A sixth of the budget holds
    // fewer groups than one or two 8192-row scan batches make on a
    // near-unique key, so every round is judged; the row path holds more
    // per group and gets more. The unordered ones have no final sort: the
    // budget is sized for the partial, not for a sort of every group.
    let cases: [(&str, bool, bool, u64); 4] = [
        (
            "SELECT user_id, duration_sec, latency_p75_ms, COUNT(*) AS n, SUM(actions) AS a, MAX(queries_run) AS q, MIN(event_date) AS d FROM events GROUP BY user_id, duration_sec, latency_p75_ms",
            false,
            true,
            4,
        ),
        (
            "SELECT user_id, duration_sec, COUNT(DISTINCT country) AS c, COUNT(*) AS n FROM events GROUP BY user_id, duration_sec",
            false,
            true,
            16,
        ),
        (
            "SELECT user_id, surface, COUNT(*) AS n, AVG(latency_p75_ms) AS l, MIN(country) AS c FROM events GROUP BY user_id, surface ORDER BY n DESC, user_id, surface LIMIT 50",
            true,
            false,
            4,
        ),
        (
            "SELECT region, COUNT(*) AS n, SUM(actions) AS a FROM events GROUP BY region",
            false,
            false,
            4,
        ),
    ];
    let run = |statement: &str, ordered: bool, enabled: bool, budget: u64| -> (Vec<String>, u64) {
        let plan = bound_plan(statement, &manager);
        let pool = QueryMemoryPool::new("passthrough", budget << 20).unwrap();
        // One partial per task, so each reads every row of its task.
        kaveon_exec::local_parallel::set_query_parallelism(&pool, 1).unwrap();
        AdaptivePartialSettings {
            enabled,
            min_rows: 4_000,
            threshold: kaveon_exec::partitioned::ADAPTIVE_PARTIAL_THRESHOLD,
        }
        .register(&pool)
        .unwrap();
        let batches = execute_distributed("passthrough", &plan, &manager, 2, &pool)
            .unwrap_or_else(|error| panic!("{statement}: {error}"));
        assert_eq!(pool.snapshot().current_bytes, 0, "{statement} leaked");
        let passed = kaveon_exec::aggregate::aggregate_metrics(&pool)
            .unwrap()
            .snapshot()
            .partial_passthrough_rows;
        (canonical_rows(&batches, ordered), passed)
    };
    for (statement, ordered, near_unique, budget) in cases {
        let (aggregating, passed_off) = run(statement, ordered, false, budget);
        let (adaptive, passed_on) = run(statement, ordered, true, budget);
        assert_eq!(
            passed_off, 0,
            "{statement}: passed rows through with the rule off"
        );
        assert_eq!(
            passed_on > 0,
            near_unique,
            "{statement}: {passed_on} rows passed through"
        );
        assert_eq!(adaptive, aggregating, "{statement}");
        assert!(!adaptive.is_empty());
    }
    let _ = std::fs::remove_dir_all(&directory);
}

/// A join whose build side the budget refuses spills and answers what
/// the in-memory join answers: inner and left joins of the events with
/// themselves on `user_id` (a build of every row), a semi and an anti
/// join with a residual (Q21's shape) and a `NOT IN` whose set holds
/// every user, each through the node-local planner and through the
/// fragments with a spill registered and a budget that refuses the
/// build, against the same statements with an ample budget. The join
/// spill counters say the spill happened.
#[test]
fn a_join_under_a_refusing_budget_spills_and_matches_the_in_memory_join() {
    let directory =
        std::env::temp_dir().join(format!("kaveon-join-spill-{}", uuid::Uuid::new_v4()));
    let manager = events_catalog(&directory);
    let spill_root = directory.join("spill");
    let cases: [(&str, bool); 6] = [
        (
            "SELECT t.country, COUNT(*) AS n, SUM(o.actions) AS a FROM events t JOIN events o ON o.user_id = t.user_id WHERE t.event_date = '2026-07-04' GROUP BY t.country ORDER BY t.country",
            true,
        ),
        (
            "SELECT t.country, COUNT(*) AS n, COUNT(o.user_id) AS m FROM events t LEFT JOIN events o ON o.user_id = t.actions WHERE t.event_date = '2026-07-04' GROUP BY t.country ORDER BY t.country",
            true,
        ),
        (
            "SELECT t.country, COUNT(*) AS n FROM events t WHERE t.event_date = '2026-07-04' AND EXISTS (SELECT * FROM events o WHERE o.user_id = t.user_id AND o.country <> t.country) GROUP BY t.country ORDER BY t.country",
            true,
        ),
        (
            "SELECT t.country, COUNT(*) AS n FROM events t WHERE t.event_date = '2026-07-04' AND NOT EXISTS (SELECT * FROM events o WHERE o.user_id = t.user_id AND o.country <> t.country AND o.actions > t.actions) GROUP BY t.country ORDER BY t.country",
            true,
        ),
        (
            "SELECT COUNT(*) AS n FROM events t WHERE t.event_date = '2026-07-04' AND t.user_id NOT IN (SELECT user_id FROM events WHERE actions > 5)",
            false,
        ),
        (
            "SELECT t.country, COUNT(*) AS n FROM events t WHERE t.event_date = '2026-07-04' AND t.user_id IN (SELECT user_id FROM events WHERE platform <> 'Web') GROUP BY t.country ORDER BY t.country",
            true,
        ),
    ];
    // (local budget, distributed budget): the fragments' final stage
    // holds more beside the join, so its budget is a little larger; a
    // build of every event row needs more than half of either.
    let run = |statement: &str,
               ordered: bool,
               budgets: (u64, u64),
               spill: bool|
     -> (Vec<String>, Vec<String>, u64) {
        let plan = bound_plan(statement, &manager);
        let pools = |budget: u64| -> QueryMemoryPool {
            let pool = QueryMemoryPool::new("join-spill", budget).unwrap();
            if spill {
                kaveon_exec::partitioned::register_spill(
                    &pool,
                    kaveon_exec::spill::SpillManager::new(&spill_root, 256 << 20).unwrap(),
                    8,
                )
                .unwrap();
            }
            pool
        };
        let pool = pools(budgets.0);
        let mut planned = crate::planner::plan_query_with_memory(&plan, &manager, &pool)
            .unwrap_or_else(|error| panic!("{statement}: {error}"));
        let local = canonical_rows(&drain(planned.operator.as_mut()), ordered);
        drop(planned);
        assert_eq!(pool.snapshot().current_bytes, 0, "{statement} leaked");
        let local_spill = kaveon_exec::partitioned::join_spill_metrics(&pool)
            .unwrap()
            .snapshot();
        let pool = pools(budgets.1);
        let batches = execute_distributed("join-spill", &plan, &manager, 2, &pool)
            .unwrap_or_else(|error| panic!("{statement} (distributed): {error}"));
        assert_eq!(
            pool.snapshot().current_bytes,
            0,
            "{statement} (distributed) leaked"
        );
        let distributed_spill = kaveon_exec::partitioned::join_spill_metrics(&pool)
            .unwrap()
            .snapshot();
        (
            local,
            canonical_rows(&batches, ordered),
            local_spill.partitions + distributed_spill.partitions,
        )
    };
    for (statement, ordered) in cases {
        let (local, distributed, partitions) =
            run(statement, ordered, (256 << 20, 256 << 20), false);
        assert_eq!(partitions, 0);
        assert_eq!(local, distributed, "{statement}: local versus distributed");
        assert!(!local.is_empty(), "{statement}");
        let (spilled_local, spilled_distributed, partitions) =
            run(statement, ordered, (3 << 20, 4 << 20), true);
        assert!(partitions > 0, "{statement}: the build was not spilled");
        assert_eq!(spilled_local, local, "{statement}: spilled local");
        assert_eq!(
            spilled_distributed, local,
            "{statement}: spilled distributed"
        );
    }
    let _ = std::fs::remove_dir_all(&directory);
}

/// The statements the cube answers over the events directory, each with
/// the scanned statement it must equal: the same text, or — for a
/// distinct count the `approximate` setting lowers — the `APPROX_*`
/// spelling on the row path and the exact spelling within the error.
const CUBE_CASES: &[(&str, &str)] = &[
    (
        "cube_grand_total",
        "SELECT COUNT(*), SUM(actions), COUNT(actions), MIN(latency_p75_ms), MAX(latency_p75_ms), SUM(errors) FROM {T}",
    ),
    (
        "cube_subset_of_measures",
        "SELECT MAX(latency_p75_ms) AS slowest FROM {T}",
    ),
    (
        "cube_one_dimension",
        "SELECT region, SUM(actions), COUNT(*) FROM {T} GROUP BY region",
    ),
    (
        "cube_null_dimension",
        "SELECT industry, COUNT(*) AS n, MIN(latency_p75_ms) FROM {T} GROUP BY industry",
    ),
    (
        "cube_two_dimensions",
        "SELECT platform, region, SUM(errors), MAX(latency_p75_ms) FROM {T} GROUP BY region, platform",
    ),
    (
        "cube_time_grain",
        "SELECT event_date, COUNT(*), SUM(actions) FROM {T} GROUP BY event_date",
    ),
    (
        "cube_time_and_dimension",
        "SELECT event_date, platform, SUM(actions) FROM {T} GROUP BY event_date, platform",
    ),
    (
        "cube_predicate_grand_total",
        "SELECT COUNT(*), SUM(actions) FROM {T} WHERE region = 'Europe'",
    ),
    (
        "cube_predicate_in_rolled_up",
        "SELECT platform, SUM(actions), COUNT(actions) FROM {T} WHERE region IN ('Europe', 'North America') GROUP BY platform",
    ),
    (
        "cube_predicate_and_group_same_axis",
        "SELECT region, MIN(latency_p75_ms) FROM {T} WHERE region = 'Asia' GROUP BY region",
    ),
    (
        "cube_predicate_no_match",
        "SELECT COUNT(*), SUM(actions), MAX(latency_p75_ms) FROM {T} WHERE platform = 'Console'",
    ),
    (
        "cube_two_predicates",
        "SELECT SUM(errors) FROM {T} WHERE region = 'Europe' AND platform = 'Web'",
    ),
    (
        "cube_approx_distinct",
        "SELECT region, APPROX_COUNT_DISTINCT(user_id) AS users FROM {T} GROUP BY region",
    ),
    (
        "cube_approx_distinct_filtered",
        "SELECT APPROX_COUNT_DISTINCT(user_id) FROM {T} WHERE platform IN ('Web', 'Mobile')",
    ),
];

#[test]
fn the_cube_answers_what_the_row_path_answers() {
    let directory =
        std::env::temp_dir().join(format!("kaveon-differential-cube-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&directory).unwrap();
    let events = events();
    let mut catalog = MemoryCatalog::new(
        "lake",
        StorageType::Local {
            base_path: PathBuf::from(&directory),
        },
    )
    .with_schema("events");
    let parts = write_directory(&directory, "events_parts", &batch(&events, false), 3);
    let schema = parts.arrow_schema.clone();
    catalog.register_table("events", parts).unwrap();
    let mut manager = CatalogManager::new("lake", "events");
    manager.register_catalog(Box::new(catalog));
    let shape = kaveon_core::TableShape::parse(
        &[
            "region:10".into(),
            "platform:5".into(),
            "industry:10".into(),
        ],
        &[
            "actions:sum,count".into(),
            "latency_p75_ms:min,max".into(),
            "errors:sum".into(),
            "user_id:count_distinct".into(),
        ],
        Some("event_date:day:40"),
    )
    .unwrap();
    let built = kaveon_storage::build_cube(
        directory.join("events_parts").to_str().unwrap(),
        DataFormat::Parquet,
        kaveon_core::TableId::new("table:lake:events:events_parts").unwrap(),
        &shape,
        &kaveon_storage::CubeBuildOptions {
            memory: None,
            threads: 3,
            max_cells: 100_000,
        },
    )
    .unwrap();
    assert!(built.cube.excluded.is_empty(), "{:?}", built.cube.excluded);
    let optimized_as = |statement: &str, approximate: bool| -> LogicalPlan {
        let mut plan = if approximate {
            kaveon_sql::logical_plan::sql_to_logical_plan_for_binder_approximate(statement)
        } else {
            kaveon_sql::logical_plan::sql_to_logical_plan_for_binder(statement)
        }
        .unwrap();
        crate::planner::qualify_tables(&mut plan, "lake", "events");
        let plan = kaveon_optim::binder::bind(plan, &manager)
            .unwrap_or_else(|error| panic!("{statement}: {error}"));
        let plan = kaveon_optim::rules::push_filter_down(plan);
        kaveon_optim::rules::push_projection_down(plan)
    };
    let scanned = |plan: &LogicalPlan, statement: &str| -> (Vec<String>, Vec<String>) {
        let pool = QueryMemoryPool::new("differential-cube", 256 * 1024 * 1024).unwrap();
        let mut planned = crate::planner::plan_query_with_memory(plan, &manager, &pool)
            .unwrap_or_else(|error| panic!("{statement}: {error}"));
        let columns = planned
            .operator
            .schema()
            .fields()
            .iter()
            .map(|field| format!("{}:{}", field.name(), field.data_type()))
            .collect();
        let rows = canonical_rows(&drain(planned.operator.as_mut()), false);
        drop(planned);
        (columns, rows)
    };
    let from_cube = |plan: &LogicalPlan, statement: &str| -> (Vec<String>, Vec<String>) {
        let query = kaveon_optim::cube::match_cube_query(plan, &shape, &schema)
            .unwrap_or_else(|| panic!("{statement}: the cube does not cover it"));
        let batch = kaveon_optim::cube::answer(&query, &built.cube, &schema)
            .unwrap()
            .unwrap_or_else(|| panic!("{statement}: the cube holds no grouping for it"));
        let columns = batch
            .schema()
            .fields()
            .iter()
            .map(|field| format!("{}:{}", field.name(), field.data_type()))
            .collect();
        (columns, canonical_rows(&[batch], false))
    };
    let mut mismatches = Vec::new();
    for (name, template) in CUBE_CASES {
        let statement = template.replace("{T}", "events_parts");
        let plan = optimized_as(&statement, false);
        let (scanned_columns, scanned_rows) = scanned(&plan, &statement);
        let (cube_columns, cube_rows) = from_cube(&plan, &statement);
        assert!(!scanned_rows.is_empty(), "{name} returned no rows");
        if scanned_columns != cube_columns {
            mismatches.push(format!(
                "{name}: scanned columns {scanned_columns:?} versus cube {cube_columns:?}"
            ));
        }
        // Additive measures and the row count are exact; the distinct
        // sketches merge exactly, so the estimates are the same numbers.
        if rows_differ(&scanned_rows, &cube_rows, 0.0) {
            mismatches.push(format!(
                "{name}: scanned {:?} versus cube {:?}",
                scanned_rows.iter().take(3).collect::<Vec<_>>(),
                cube_rows.iter().take(3).collect::<Vec<_>>()
            ));
        }
    }
    // A plain COUNT(DISTINCT) under the `approximate` setting: the cube
    // answers under COUNT's name what the sketch computes, and within the
    // sketch's error of the exact count.
    {
        let statement = "SELECT region, COUNT(DISTINCT user_id) AS users, COUNT(*) AS n FROM events_parts GROUP BY region";
        let approximate = optimized_as(statement, true);
        let (cube_columns, cube_rows) = from_cube(&approximate, statement);
        let (computed_columns, computed_rows) = scanned(&approximate, statement);
        assert_eq!(cube_columns, computed_columns);
        assert_eq!(cube_columns[1], "users:UInt64");
        assert!(!rows_differ(&computed_rows, &cube_rows, 0.0));
        let (_, exact_rows) = scanned(&optimized_as(statement, false), statement);
        let error = kaveon_core::HllSketch::default_precision().standard_error();
        assert!(rows_differ(&exact_rows, &cube_rows, 0.0));
        assert!(!rows_differ(&exact_rows, &cube_rows, 3.0 * error));
        // The exact spelling is never answered from the cube.
        assert!(
            kaveon_optim::cube::match_cube_query(&optimized_as(statement, false), &shape, &schema)
                .is_none()
        );
    }
    let _ = std::fs::remove_dir_all(&directory);
    assert!(mismatches.is_empty(), "{}", mismatches.join("\n"));
}

/// `ANALYZE … WITH (sketches = true)` on the workers is the coordinator's
/// read: the one `COLUMN_STATISTICS` statement over the single-file
/// dictionary table, its scan split by row group across two in-process
/// workers, folded into the metadata document, against the storage
/// builder's full read of the same file — rows, bounds, null counts and
/// exactness flags equal, the HyperLogLog registers identical (merges are
/// exact whatever the partitioning), the KLL sketches agreeing on every
/// decile within the sketch's rank error.
#[test]
fn the_distributed_statistics_read_matches_the_coordinator_build() {
    let directory = std::env::temp_dir().join(format!(
        "kaveon-differential-statistics-{}",
        uuid::Uuid::new_v4()
    ));
    let manager = events_catalog(&directory);
    let location = directory.join("events.parquet");
    let table_id = kaveon_core::TableId::new("table:lake:events:events").unwrap();
    let local = kaveon_storage::full_statistics(
        location.to_str().unwrap(),
        DataFormat::Parquet,
        table_id.clone(),
        &kaveon_storage::FullScanOptions {
            memory: None,
            threads: 1,
            columns: None,
        },
    )
    .unwrap();
    let mut distributed = kaveon_storage::metadata_statistics(
        location.to_str().unwrap(),
        DataFormat::Parquet,
        table_id,
    )
    .unwrap();
    let selected = kaveon_storage::sketched_columns(&distributed, None).unwrap();
    assert_eq!(selected.len(), distributed.columns.len());
    let statement = crate::api::statistics_read_sql(&distributed, &selected, "events");
    assert!(statement.starts_with("SELECT COUNT(*), COLUMN_STATISTICS(\"user_id\"), "));
    let plan = bound_plan(&statement, &manager);
    let pool = QueryMemoryPool::new("differential-statistics", 256 * 1024 * 1024).unwrap();
    // Two workers over one file: each scan task takes the row groups of
    // its partition, so the rows are read once between them.
    let batches = execute_distributed(&statement, &plan, &manager, 2, &pool).unwrap();
    let rows = crate::api::batches_to_json(&batches);
    assert_eq!(rows.len(), 1);
    crate::api::apply_statistics_read_row(&mut distributed, &selected, &rows[0]).unwrap();

    assert_eq!(distributed.depth, kaveon_core::StatisticsDepth::Full);
    assert_eq!(distributed.rows, local.rows);
    assert_eq!(distributed.rows, ROWS as u64);
    assert_eq!(distributed.source_version, local.source_version);
    assert_eq!(distributed.per_file, local.per_file);
    for (column, expected) in distributed.columns.iter().zip(&local.columns) {
        assert_eq!(column.name, expected.name);
        assert_eq!(column.null_count, expected.null_count, "{}", column.name);
        assert_eq!(column.min, expected.min, "{}", column.name);
        assert_eq!(column.max, expected.max, "{}", column.name);
        assert!(column.bounds_exact, "{}", column.name);
        assert_eq!(column.distinct, expected.distinct, "{}", column.name);
        assert_eq!(column.distinct_exact, None);
        match (&column.quantiles, &expected.quantiles) {
            (Some(theirs), Some(ours)) => {
                assert_eq!(theirs.count(), ours.count(), "{}", column.name);
                // Each sketch's rank of a value is within its error of the
                // true rank, so the two agree within twice it — at every
                // decile of the coordinator's sketch (a value's rank, not
                // the decile itself: the columns hold ties).
                let error = 2.0 * ours.rank_error();
                for decile in 1..10 {
                    let fraction = f64::from(decile) / 10.0;
                    let value = ours.quantile(fraction).unwrap();
                    let (rank, expected) = (theirs.rank(value), ours.rank(value));
                    assert!(
                        (rank - expected).abs() <= error,
                        "{}: {value} ranks {rank} in the workers' sketch, {expected} in the coordinator's",
                        column.name
                    );
                }
            }
            (None, None) => assert!(!kaveon_core::sketch::quantile_sketchable(&column.data_type)),
            (theirs, ours) => panic!("{}: {theirs:?} versus {ours:?}", column.name),
        }
    }
    // What the document says is what a client reads back.
    let bytes = distributed.to_json_bytes().unwrap();
    assert_eq!(
        kaveon_core::TableStatistics::from_json_bytes(&bytes).unwrap(),
        distributed
    );
    let _ = std::fs::remove_dir_all(&directory);
}

/// `ANALYZE … WITH (cube = true)` on the workers is the coordinator's
/// build: every grouping of the declared shape as one `GROUP BY`
/// statement through the two-worker fragments, its rows typed back into
/// cells, against the storage builder's cube over the same directory —
/// cell for cell, the additive measures exact and the distinct sketches
/// bit-identical. An axis over its cap is excluded the same way: the
/// single-axis statement reads one row past the cap and stops.
#[test]
fn the_distributed_cube_read_matches_the_coordinator_build() {
    let directory = std::env::temp_dir().join(format!(
        "kaveon-differential-cube-read-{}",
        uuid::Uuid::new_v4()
    ));
    std::fs::create_dir_all(&directory).unwrap();
    let events = events();
    let mut catalog = MemoryCatalog::new(
        "lake",
        StorageType::Local {
            base_path: PathBuf::from(&directory),
        },
    )
    .with_schema("events");
    let parts = write_directory(&directory, "events_parts", &batch(&events, false), 3);
    catalog.register_table("events", parts).unwrap();
    let mut manager = CatalogManager::new("lake", "events");
    manager.register_catalog(Box::new(catalog));
    let location = directory.join("events_parts");
    let table_id = kaveon_core::TableId::new("table:lake:events:events_parts").unwrap();
    let statistics = kaveon_storage::metadata_statistics(
        location.to_str().unwrap(),
        DataFormat::Parquet,
        table_id.clone(),
    )
    .unwrap();
    let pool = QueryMemoryPool::new("differential-cube-read", 256 * 1024 * 1024).unwrap();
    let dimensions = |country: Option<&str>| {
        let mut dimensions = vec![
            "region:10".to_owned(),
            "platform:5".into(),
            "industry:10".into(),
        ];
        dimensions.extend(country.map(str::to_owned));
        dimensions
    };
    let measures = [
        "actions:sum,count".to_owned(),
        "latency_p75_ms:min,max".into(),
        "errors:sum".into(),
        "user_id:count_distinct".into(),
    ];
    // Within every cap, and with a country axis over its cap of three.
    for country in [None, Some("country:3")] {
        let shape = kaveon_core::TableShape::parse(
            &dimensions(country),
            &measures,
            Some("event_date:day:40"),
        )
        .unwrap();
        let local = kaveon_storage::build_cube(
            location.to_str().unwrap(),
            DataFormat::Parquet,
            table_id.clone(),
            &shape,
            &kaveon_storage::CubeBuildOptions {
                memory: None,
                threads: 3,
                max_cells: 100_000,
            },
        )
        .unwrap()
        .cube;
        let axes = shape.axes();
        let mut excluded = Vec::new();
        let mut read = Vec::new();
        for grouping in shape.groupings() {
            if grouping.iter().any(|axis| excluded.contains(axis)) {
                continue;
            }
            let statement = crate::api::cube_grouping_read(
                &shape,
                &statistics.columns,
                &grouping,
                "events_parts",
            );
            let plan = bound_plan(&statement.sql, &manager);
            let batches = execute_distributed(&statement.sql, &plan, &manager, 2, &pool)
                .unwrap_or_else(|error| panic!("{}: {error}", statement.sql));
            let rows = crate::api::batches_to_json(&batches);
            if let (Some(limit), [axis]) = (statement.limit, grouping.as_slice())
                && rows.len() as u64 >= limit
            {
                excluded.push(*axis);
                assert_eq!(axes[*axis].column, "country");
                continue;
            }
            read.push(
                crate::api::cube_grouping_from_rows(&shape, &statistics.columns, &grouping, &rows)
                    .unwrap(),
            );
        }
        assert_eq!(
            local
                .excluded
                .iter()
                .map(|axis| axis.column.as_str())
                .collect::<Vec<_>>(),
            excluded
                .iter()
                .map(|axis| axes[*axis].column.as_str())
                .collect::<Vec<_>>()
        );
        assert_eq!(read.len(), local.groupings.len());
        // (), the four axes within their caps, their six pairs.
        assert_eq!(read.len(), 11);
        for (grouping, expected) in read.iter().zip(&local.groupings) {
            assert_eq!(grouping.axes, expected.axes);
            assert_eq!(
                grouping.cells.len(),
                expected.cells.len(),
                "grouping {:?}",
                grouping.axes
            );
            for (cell, expected) in grouping.cells.iter().zip(&expected.cells) {
                assert_eq!(cell, expected, "grouping {:?}", grouping.axes);
            }
        }
    }
    let _ = std::fs::remove_dir_all(&directory);
}
