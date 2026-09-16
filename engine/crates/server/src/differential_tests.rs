//! The differential sweep as a gate: the statements of
//! `scripts/differential-cases.py`, run against the same rows held in two
//! Parquet encodings — an Arrow dictionary schema for every text column,
//! and plain UTF-8 — through the node-local planner. Any divergence is an
//! Engine defect, whatever the cluster later says. The encodings exercise
//! different operator paths (dictionary-aware predicates, coded folds, the
//! columnar aggregate's arena keys) against one truth.
use std::collections::BTreeMap;
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
    AccessPattern, BatchOperator, CatalogManager, CatalogProvider, DataFormat, MemoryCatalog,
    QueryMemoryPool, StorageType, TableMeta,
};
use parquet::arrow::ArrowWriter;
use parquet::file::properties::WriterProperties;

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

/// Rows as text, one string per row, so both encodings compare alike.
fn canonical_rows(operator: &mut dyn BatchOperator, ordered: bool) -> Vec<String> {
    let mut rows = Vec::new();
    while let Some(batch) = operator.next_batch().unwrap() {
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

#[test]
fn the_differential_sweep_matches_across_parquet_encodings() {
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
    let mut manager = CatalogManager::new("lake", "events");
    manager.register_catalog(Box::new(catalog));

    let run = |statement: &str, ordered: bool| -> Vec<String> {
        let mut plan = kaveon_sql::logical_plan::sql_to_logical_plan(statement).unwrap();
        crate::planner::qualify_tables(&mut plan, "lake", "events");
        let plan = kaveon_optim::rules::push_filter_down(plan);
        let plan = kaveon_optim::rules::push_projection_down(plan);
        let pool = QueryMemoryPool::new("differential", 256 * 1024 * 1024).unwrap();
        let mut planned = crate::planner::plan_query_with_memory(&plan, &manager, &pool)
            .unwrap_or_else(|error| panic!("{statement}: {error}"));
        let rows = canonical_rows(planned.operator.as_mut(), ordered);
        drop(planned);
        assert_eq!(
            pool.snapshot().current_bytes,
            0,
            "{statement} leaked reservations"
        );
        rows
    };
    let mut mismatches = Vec::new();
    for (name, template, ordered) in CASES {
        let dictionary = run(
            &template
                .replace("{T}", "events_dictionary")
                .replace("{U}", "users"),
            *ordered,
        );
        let plain = run(
            &template
                .replace("{T}", "events_plain")
                .replace("{U}", "users"),
            *ordered,
        );
        assert!(!dictionary.is_empty(), "{name} returned no rows");
        if dictionary != plain {
            mismatches.push(format!(
                "{name}: dictionary {:?} versus plain {:?}",
                dictionary.iter().take(3).collect::<Vec<_>>(),
                plain.iter().take(3).collect::<Vec<_>>()
            ));
        }
    }
    let _ = std::fs::remove_dir_all(&directory);
    assert!(mismatches.is_empty(), "{}", mismatches.join("\n"));
}
