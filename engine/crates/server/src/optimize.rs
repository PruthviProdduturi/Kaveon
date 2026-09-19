//! `OPTIMIZE [catalog.][schema.]table [WHERE predicate]`: rewrite a Parquet
//! table's files in the layout its definition declares.
//!
//! The statement composes three things the Engine already has: the
//! storage layer's [`RewriteTarget`] (the table's files, the files a
//! predicate may touch by footer statistics, a source over every row of
//! them, staging and crash-safe publication — `kaveon_storage::table_rewrite`
//! documents the order), the executor's `SortOperator` — the bounded
//! external sort over the statement's admitted memory pool and the spill
//! machinery, which is what keeps a file's rows from ever being held whole
//! — and the storage layer's `ClusteredParquetWriter`, which lays the
//! sorted rows out as row groups the readers skip. A table with no
//! clustering columns is compacted to the layout without a sort.
//!
//! Only Parquet tables are rewritten. A Delta or Iceberg table's files are
//! named by a log this Engine does not write; the statement refuses them
//! by name rather than leave a log pointing at files it moved. `WHERE`
//! selects files, not rows: a file whose path values or statistics admit
//! the predicate is rewritten whole, the others are left as they are, so
//! the rewritten part of the table is clustered and the rest untouched. A
//! Hive-partitioned directory is rewritten one partition directory at a
//! time, each group's files landing under its own `key=value` path; a
//! clustering column that is a partition column is refused, since it is
//! constant within every file. One rewrite runs per location at a time.

use std::collections::HashSet;
use std::sync::{Arc, Mutex, OnceLock};

use axum::http::StatusCode;
use kaveon_core::{
    AccessPattern, BatchOperator, CatalogLifecycle, DataFormat, KaveonError, QueryMemoryPool,
    ResolvedTable, StoragePredicate, TableMeta,
};
use kaveon_exec::scan::ScanOperator;
use kaveon_exec::sort::{SortExpr, SortOperator};
use kaveon_exec::spill::SpillManager;
use kaveon_sql::ddl::{OptimizeOptions, QualifiedName, quote_identifier};
use kaveon_sql::logical_plan::LogicalPlan;
use kaveon_storage::{ClusteringLayout, RewriteReport, RewriteTarget};
use serde_json::json;

use crate::AppState;
use crate::api::ColumnInfo;
use crate::catalog_ddl::{CatalogStatementResult, locate_table, table_target};
use crate::security::{Identity, Role};

/// Disk the sort may spill when the statement's memory does not hold the
/// table, when no query spill is configured: 10 GiB under the temp
/// directory (`KAVEON_HASH_SPILL_ROOT` and `KAVEON_HASH_SPILL_BYTES` take
/// precedence, as for every operator).
const DEFAULT_SPILL_BYTES: u64 = 10 * 1024 * 1024 * 1024;

/// A refused or failed `OPTIMIZE`: the HTTP status, a stable code and a
/// message naming what is wrong.
#[derive(Debug)]
pub(crate) struct OptimizeError {
    pub(crate) status: StatusCode,
    pub(crate) code: &'static str,
    pub(crate) message: String,
}

impl OptimizeError {
    fn new(status: StatusCode, code: &'static str, message: impl Into<String>) -> Self {
        Self {
            status,
            code,
            message: message.into(),
        }
    }
    fn invalid(message: impl Into<String>) -> Self {
        Self::new(StatusCode::BAD_REQUEST, "OPTIMIZE_INVALID", message)
    }
    fn failed(message: impl Into<String>) -> Self {
        Self::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            "OPTIMIZE_FAILED",
            message,
        )
    }
}

/// The locations being rewritten by this process: a second `OPTIMIZE` of
/// the same table waits for the first to finish rather than race its
/// listing.
fn in_flight() -> &'static Mutex<HashSet<String>> {
    static IN_FLIGHT: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();
    IN_FLIGHT.get_or_init(|| Mutex::new(HashSet::new()))
}

struct InFlightGuard(String);

impl InFlightGuard {
    fn acquire(location: &str) -> Option<Self> {
        let mut set = in_flight()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        set.insert(location.to_owned())
            .then(|| Self(location.to_owned()))
    }
}

impl Drop for InFlightGuard {
    fn drop(&mut self) {
        let mut set = in_flight()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        set.remove(&self.0);
    }
}

/// Run `OPTIMIZE` for `identity` under the statement's admitted memory.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn execute_optimize(
    state: &Arc<AppState>,
    identity: &Identity,
    context_catalog: &str,
    context_schema: &str,
    name: QualifiedName,
    filter: Option<String>,
    options: OptimizeOptions,
    pool: QueryMemoryPool,
) -> Result<CatalogStatementResult, OptimizeError> {
    if identity.role != Role::Admin {
        return Err(OptimizeError::new(
            StatusCode::FORBIDDEN,
            "FORBIDDEN",
            "OPTIMIZE requires the admin role",
        ));
    }
    let target = table_target(&name, context_catalog, context_schema);
    let qualified = target.qualified();
    let (catalog, _, table) = locate_table(&state.catalog_store, &target)
        .map_err(|error| OptimizeError::new(error.status, error.code, error.message))?;
    let Some(table) = table else {
        return Err(OptimizeError::new(
            StatusCode::BAD_REQUEST,
            "TABLE_NOT_FOUND",
            format!("table {qualified} not found"),
        ));
    };
    if table.lifecycle() != CatalogLifecycle::Active {
        return Err(OptimizeError::invalid(format!(
            "table {qualified} is {:?}; only an active table is rewritten",
            table.lifecycle()
        )));
    }
    match table.format() {
        DataFormat::Parquet => {}
        DataFormat::Delta => {
            return Err(OptimizeError::new(
                StatusCode::BAD_REQUEST,
                "OPTIMIZE_UNSUPPORTED",
                format!(
                    "table {qualified} is a Delta table; OPTIMIZE rewrites Parquet tables only — \
                     the Engine has no Delta commit writer, and a Delta table's files are named by \
                     its log, so they are rewritten by the writer that owns the log"
                ),
            ));
        }
        DataFormat::Iceberg => {
            return Err(OptimizeError::new(
                StatusCode::BAD_REQUEST,
                "OPTIMIZE_UNSUPPORTED",
                format!(
                    "table {qualified} is an Iceberg table; OPTIMIZE rewrites Parquet tables only — \
                     an Iceberg table's files are named by its manifests, which the Engine does not write"
                ),
            ));
        }
    }
    let arrow_schema = Arc::new(arrow::datatypes::Schema::new(
        table
            .columns()
            .iter()
            .map(|column| {
                arrow::datatypes::Field::new(
                    column.name(),
                    column.data_type().clone(),
                    column.nullable(),
                )
            })
            .collect::<Vec<_>>(),
    ));
    let resolved = ResolvedTable {
        catalog: catalog.name().to_owned(),
        schema: target.schema.clone(),
        table: Arc::new(TableMeta {
            name: target.table.clone(),
            arrow_schema,
            location: table.location().to_owned(),
            access: AccessPattern::Shortcut,
            format: DataFormat::Parquet,
        }),
        storage: catalog.storage().clone(),
    };
    let location = resolved.full_path();
    let catalog_schema = Arc::clone(&resolved.table.arrow_schema);
    let predicate = match filter {
        Some(filter) => Some(storage_predicate(state, &target, &filter).await?),
        None => None,
    };
    if let Some(column) = table
        .partitions()
        .iter()
        .map(|column| column.name())
        .find(|name| {
            table
                .layout()
                .clustered_by()
                .iter()
                .any(|column| column == name)
        })
    {
        return Err(OptimizeError::invalid(format!(
            "table {qualified} is clustered by '{column}', a partition column: a partition \
             column is constant within every file, so it cannot order rows within one; cluster \
             by a file column"
        )));
    }
    let mut layout = ClusteringLayout::new(
        table.layout().clustered_by().to_vec(),
        table.layout().bloom().to_vec(),
    );
    if let Some(rows) = options.row_group_rows {
        layout = layout.with_max_row_group_rows(usize::try_from(rows).map_err(|_| {
            OptimizeError::invalid("row_group_rows is larger than this platform addresses")
        })?);
    }
    if let Some(bytes) = options.row_group_bytes {
        layout = layout.with_target_row_group_bytes(bytes);
    }
    if let Some(bytes) = options.file_bytes {
        layout = layout.with_target_file_bytes(Some(bytes));
    }
    let Some(_guard) = InFlightGuard::acquire(&location) else {
        return Err(OptimizeError::new(
            StatusCode::CONFLICT,
            "OPTIMIZE_IN_PROGRESS",
            format!("table {qualified} is being rewritten by another statement"),
        ));
    };
    let (report, recovered) = tokio::task::spawn_blocking(move || {
        rewrite(&location, predicate, &catalog_schema, layout, &pool)
    })
    .await
    .map_err(|error| OptimizeError::failed(format!("OPTIMIZE did not complete: {error}")))??;
    Ok(CatalogStatementResult {
        columns: [
            "table",
            "files_replaced",
            "files_written",
            "rows",
            "row_groups",
            "bytes_before",
            "bytes_after",
            "clustered_by",
            "recovered",
        ]
        .into_iter()
        .map(|name| ColumnInfo {
            name: name.into(),
            data_type: if name == "table" || name == "clustered_by" {
                "Utf8".into()
            } else {
                "Int64".into()
            },
        })
        .collect(),
        rows: vec![vec![
            json!(qualified),
            json!(report.files_replaced),
            json!(report.files_written),
            json!(report.rows),
            json!(report.row_groups),
            json!(report.bytes_before),
            json!(report.bytes_after),
            json!(
                table
                    .layout()
                    .clustered_by()
                    .iter()
                    .map(|column| quote_identifier(column))
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            json!(recovered),
        ]],
    })
}

/// The storage predicate of `OPTIMIZE … WHERE`, lowered through the query
/// pipeline against the published table so a column or literal that does
/// not fit is refused the way a query's would be.
async fn storage_predicate(
    state: &AppState,
    target: &crate::catalog_ddl::TableTarget,
    filter: &str,
) -> Result<StoragePredicate, OptimizeError> {
    let statement = format!(
        "SELECT * FROM {}.{}.{} WHERE {filter}",
        quote_identifier(&target.catalog),
        quote_identifier(&target.schema),
        quote_identifier(&target.table)
    );
    let mut plan = kaveon_sql::logical_plan::sql_to_logical_plan_for_binder(&statement)
        .map_err(|error| OptimizeError::invalid(format!("OPTIMIZE WHERE: {error}")))?;
    crate::planner::qualify_tables(&mut plan, &target.catalog, &target.schema);
    let snapshot = state.catalog.read().await.clone();
    let plan = kaveon_optim::binder::bind(plan, &snapshot)
        .map_err(|error| OptimizeError::invalid(format!("OPTIMIZE WHERE: {error}")))?;
    let Some(expression) = filter_expression(&plan) else {
        return Err(OptimizeError::invalid(
            "OPTIMIZE WHERE did not lower to a filter",
        ));
    };
    kaveon_optim::rules::to_storage_predicate(expression).ok_or_else(|| {
        OptimizeError::invalid(
            "OPTIMIZE WHERE must be a predicate the storage layer evaluates on file statistics: \
             a column compared with a literal, IN, IS [NOT] NULL, [NOT] LIKE with a literal \
             pattern, combined with AND, OR and NOT",
        )
    })
}

fn filter_expression(plan: &LogicalPlan) -> Option<&kaveon_core::Expr> {
    match plan {
        LogicalPlan::Filter { predicate, .. } => Some(predicate),
        LogicalPlan::Project { input, .. } => filter_expression(input),
        _ => None,
    }
}

/// The blocking rewrite: files selected, rows sorted under `pool`, files
/// staged and published. Returns the report and how many interrupted
/// rewrites were finished or rolled back on open.
fn rewrite(
    location: &str,
    predicate: Option<StoragePredicate>,
    catalog_schema: &arrow::datatypes::SchemaRef,
    layout: ClusteringLayout,
    pool: &QueryMemoryPool,
) -> Result<(RewriteReport, u64), OptimizeError> {
    let failed = |error: KaveonError| match error {
        KaveonError::Execution(message) if message.contains("cancel") => {
            OptimizeError::new(StatusCode::CONFLICT, "CANCELED", message)
        }
        other => OptimizeError::failed(other.to_string()),
    };
    let target = RewriteTarget::open(location).map_err(failed)?;
    let recovered = target.recovery().finished + target.recovery().rolled_back;
    // The predicate meets the files' columns here: a column the files do
    // not have, or a literal of the wrong type, is the statement's fault.
    let selected = target
        .select(predicate.as_ref(), Some(catalog_schema))
        .map_err(|error| OptimizeError::invalid(format!("OPTIMIZE WHERE: {error}")))?;
    let mut report = RewriteReport {
        files_replaced: 0,
        files_written: 0,
        rows: 0,
        row_groups: 0,
        bytes_before: 0,
        bytes_after: 0,
    };
    // One group per partition directory (one for a flat table), each
    // published on its own: a rewrite that stops between groups leaves
    // every partition consistent, some in the new layout and some in the
    // old.
    for group in target.groups(&selected).map_err(failed)? {
        let published = rewrite_group(&target, &group, layout.clone(), pool).map_err(failed)?;
        report.files_replaced += published.files_replaced;
        report.files_written += published.files_written;
        report.rows += published.rows;
        report.row_groups += published.row_groups;
        report.bytes_before += published.bytes_before;
        report.bytes_after += published.bytes_after;
    }
    Ok((report, recovered))
}

fn rewrite_group(
    target: &RewriteTarget,
    group: &kaveon_storage::RewriteGroup,
    layout: ClusteringLayout,
    pool: &QueryMemoryPool,
) -> kaveon_core::Result<RewriteReport> {
    let selected = &group.files;
    let source = target.source(selected)?;
    let scan = ScanOperator::new(source, None)?;
    let schema = scan.schema().clone();
    let mut input: Box<dyn BatchOperator> = Box::new(scan);
    if !layout.clustered_by.is_empty() {
        let ordering = layout
            .clustered_by
            .iter()
            .map(|column| SortExpr::new(kaveon_core::Expr::Column(column.clone()), true))
            .collect();
        let memory = pool.operator("optimize-sort")?;
        let spill = match kaveon_exec::partitioned::spill_from_environment(pool)? {
            Some((spill, _)) => spill,
            None => SpillManager::new(
                std::env::temp_dir().join("kaveon-optimize-spill"),
                DEFAULT_SPILL_BYTES,
            )?,
        };
        input = Box::new(SortOperator::new_with_spill(
            input, ordering, memory, spill,
        )?);
    }
    let (mut writer, staging) = target.stage(layout, schema, &group.directory)?;
    writer.write_all(input.as_mut())?;
    drop(input);
    let written = writer.finish()?;
    target.publish(staging, &written, selected)
}

#[cfg(test)]
mod tests {
    use super::*;
    use arrow::array::{Int64Array, StringArray};
    use arrow::datatypes::{DataType, Field, Schema};
    use arrow::record_batch::RecordBatch;
    use kaveon_core::{
        CatalogAdapter, CatalogDefinition, CatalogId, CompareOp, ScalarValue, StorageType,
    };
    use kaveon_sql::ddl::parse_catalog_statement;
    use kaveon_storage::{ParquetReader, ScanMetrics};
    use parquet::arrow::ArrowWriter;
    use parquet::file::properties::WriterProperties;
    use parquet::file::reader::{FileReader, SerializedFileReader};

    const ROWS_PER_FILE: i64 = 20_000;

    fn admin() -> Identity {
        Identity {
            principal: "admin@example.com".into(),
            display_identity: None,
            role: Role::Admin,
        }
    }

    fn analyst() -> Identity {
        Identity {
            principal: "analyst@example.com".into(),
            display_identity: None,
            role: Role::Analyst,
        }
    }

    fn mix(seed: u64) -> u64 {
        let mut value = seed.wrapping_mul(0x9E37_79B9_7F4A_7C15);
        value ^= value >> 29;
        value = value.wrapping_mul(0xBF58_476D_1CE4_E5B9);
        value ^ (value >> 32)
    }

    /// `ROWS_PER_FILE` rows whose keys are scattered over `start..start +
    /// ROWS_PER_FILE`.
    fn scattered(start: i64) -> RecordBatch {
        let schema = Arc::new(Schema::new(vec![
            Field::new("key", DataType::Int64, false),
            Field::new("region", DataType::Utf8, true),
            Field::new("amount", DataType::Int64, false),
        ]));
        let keys = (0..ROWS_PER_FILE)
            .map(|row| start + (mix((start + row) as u64) % ROWS_PER_FILE as u64) as i64)
            .collect::<Vec<_>>();
        RecordBatch::try_new(
            schema,
            vec![
                Arc::new(Int64Array::from(keys.clone())),
                Arc::new(StringArray::from_iter(keys.iter().map(|key| {
                    (key % 11 != 0).then(|| format!("region-{}", key % 5))
                }))),
                Arc::new(Int64Array::from(
                    keys.iter().map(|key| key * 2).collect::<Vec<_>>(),
                )),
            ],
        )
        .unwrap()
    }

    fn write(path: &std::path::Path, batch: &RecordBatch) {
        let properties = WriterProperties::builder()
            .set_max_row_group_size(1_000_000)
            .set_offset_index_disabled(true)
            .build();
        let mut writer = ArrowWriter::try_new(
            std::fs::File::create(path).unwrap(),
            batch.schema(),
            Some(properties),
        )
        .unwrap();
        writer.write(batch).unwrap();
        writer.close().unwrap();
    }

    fn base(label: &str) -> std::path::PathBuf {
        let directory = std::env::temp_dir().join(format!(
            "kaveon-optimize-{label}-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4().simple()
        ));
        std::fs::create_dir_all(&directory).unwrap();
        directory
    }

    /// Three unsorted files of consecutive key ranges under `events/`, the
    /// same three under `parts/dt=<n>/` (a Hive-partitioned directory), a
    /// single unsorted file `hits.parquet`, and a Delta table `log/`.
    fn lake(base: &std::path::Path) {
        let events = base.join("events");
        std::fs::create_dir_all(&events).unwrap();
        for part in 0..3 {
            write(
                &events.join(format!("part-{part}.parquet")),
                &scattered(part * ROWS_PER_FILE),
            );
            let partition = base.join("parts").join(format!("dt=2026-0{}", part + 1));
            std::fs::create_dir_all(&partition).unwrap();
            write(
                &partition.join("part-0.parquet"),
                &scattered(part * ROWS_PER_FILE),
            );
        }
        std::fs::write(events.join("_SUCCESS"), b"").unwrap();
        write(&base.join("hits.parquet"), &scattered(0));
        let delta = base.join("log");
        std::fs::create_dir_all(delta.join("_delta_log")).unwrap();
        write(&delta.join("part-0.parquet"), &scattered(0));
        std::fs::write(
            delta.join("_delta_log").join("00000000000000000000.json"),
            "{\"add\":{\"path\":\"part-0.parquet\"}}",
        )
        .unwrap();
    }

    fn state(base: &std::path::Path) -> Arc<AppState> {
        let state = crate::api::catalog_test_state();
        let catalog = CatalogDefinition::new(
            CatalogId::new("catalog:lake").unwrap(),
            "lake",
            CatalogAdapter::Native,
            StorageType::Local {
                base_path: base.to_path_buf(),
            },
        )
        .unwrap()
        .transition(CatalogLifecycle::Active)
        .unwrap();
        state
            .catalog_store
            .create_catalog("test", &catalog)
            .unwrap();
        Arc::new(state)
    }

    async fn ddl(state: &Arc<AppState>, sql: &str) {
        let statement = parse_catalog_statement(sql).unwrap().unwrap();
        crate::catalog_ddl::execute_catalog_statement(state, &admin(), "lake", "sales", statement)
            .await
            .unwrap_or_else(|error| panic!("{sql}: {} {}", error.code, error.message));
    }

    /// Run `sql` under a pool of `budget` bytes with a spill attached the
    /// way the query's operators find it; the spill's counters come back.
    async fn optimize(
        state: &Arc<AppState>,
        who: &Identity,
        sql: &str,
        budget: u64,
    ) -> Result<(Vec<serde_json::Value>, kaveon_exec::spill::SpillSnapshot), OptimizeError> {
        let Some(kaveon_sql::ddl::CatalogStatement::Optimize {
            name,
            filter,
            options,
        }) = parse_catalog_statement(sql).unwrap()
        else {
            panic!("{sql} is not OPTIMIZE");
        };
        let pool = QueryMemoryPool::new("optimize-test", budget).unwrap();
        let spill = SpillManager::new(
            std::env::temp_dir().join("kaveon-optimize-test-spill"),
            1024 * 1024 * 1024,
        )
        .unwrap();
        let attached = spill.clone();
        pool.shared_resource("kaveon.exec.hash-spill.v1", move || {
            Ok((attached, 16_usize))
        })
        .unwrap();
        let result = execute_optimize(
            state,
            who,
            "lake",
            "sales",
            name,
            filter,
            options,
            pool.clone(),
        )
        .await?;
        assert_eq!(
            pool.snapshot().current_bytes,
            0,
            "{sql} leaked reservations"
        );
        Ok((result.rows.into_iter().next().unwrap(), spill.snapshot()))
    }

    /// The keys of a file in file order, and its row-group count.
    fn keys(path: &std::path::Path) -> (Vec<i64>, usize) {
        let reader = ParquetReader::new(path);
        let row_groups = reader.metadata().unwrap().row_group_count;
        let keys = reader
            .read_batches()
            .unwrap()
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
            .collect();
        (keys, row_groups)
    }

    fn data_files(directory: &std::path::Path) -> Vec<std::path::PathBuf> {
        let mut files = std::fs::read_dir(directory)
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .filter(|path| {
                path.extension()
                    .is_some_and(|extension| extension == "parquet")
                    && !path.file_name().unwrap().to_str().unwrap().starts_with('_')
            })
            .collect::<Vec<_>>();
        files.sort();
        files
    }

    #[tokio::test]
    async fn optimize_rewrites_a_directory_table_in_clustering_order_and_a_filter_reads_less() {
        let base = base("directory");
        lake(&base);
        let state = state(&base);
        ddl(&state, "CREATE SCHEMA lake.sales").await;
        ddl(
            &state,
            "CREATE TABLE events WITH (location = 'events', format = 'parquet', clustered_by = ARRAY['key'], bloom = ARRAY['region'])",
        )
        .await;
        let events = base.join("events");
        let mut expected = (0..3)
            .flat_map(|part| keys(&events.join(format!("part-{part}.parquet"))).0)
            .collect::<Vec<_>>();
        expected.sort_unstable();
        let predicate = StoragePredicate::Compare {
            column: "key".into(),
            op: CompareOp::Eq,
            value: ScalarValue::Int64(45_000),
        };
        let before = ScanMetrics::default();
        let rows_before = ParquetReader::new(&events)
            .with_predicate(predicate.clone())
            .with_metrics(before.clone())
            .read_batches()
            .unwrap()
            .iter()
            .map(RecordBatch::num_rows)
            .sum::<usize>();

        // A 64 MiB budget holds a fraction of the sort's input at a time
        // (one run per limit/18): the sort spills its runs and the result
        // is still one order. Row groups of 5 000 rows, so a table this
        // small has row groups to skip.
        let (row, spill) = optimize(
            &state,
            &admin(),
            "OPTIMIZE events WITH (row_group_rows = 5000)",
            64 * 1024 * 1024,
        )
        .await
        .unwrap();
        assert!(spill.runs_written > 0, "the sort spilled no run: {spill:?}");
        assert_eq!(row[0], serde_json::json!("lake.sales.events"));
        assert_eq!(row[1], serde_json::json!(3), "files replaced");
        assert_eq!(row[3], serde_json::json!(3 * ROWS_PER_FILE), "rows");
        assert_eq!(row[7], serde_json::json!("key"));
        assert_eq!(row[8], serde_json::json!(0), "recovered");
        let files = data_files(&events);
        assert_eq!(files.len() as u64, row[2].as_u64().unwrap());
        assert!(events.join("_SUCCESS").exists());
        assert!(
            !events
                .join(kaveon_storage::table_rewrite::REWRITE_AREA)
                .exists()
        );
        let mut previous = i64::MIN;
        let mut all = Vec::new();
        for file in &files {
            let (keys, _) = keys(file);
            assert!(keys.windows(2).all(|pair| pair[0] <= pair[1]), "{file:?}");
            assert!(keys[0] >= previous, "files are in key order");
            previous = *keys.last().unwrap();
            all.extend(keys);
            let reader = SerializedFileReader::new(std::fs::File::open(file).unwrap()).unwrap();
            for group in reader.metadata().row_groups() {
                assert!(group.sorting_columns().is_some());
                assert!(group.column(0).bloom_filter_offset().is_some());
                assert!(group.column(1).bloom_filter_offset().is_some());
                assert!(group.column(2).bloom_filter_offset().is_none());
                assert!(group.column(0).offset_index_offset().is_some());
            }
        }
        all.sort_unstable();
        assert_eq!(all, expected);

        // The same point filter through the directory reader: the same
        // rows, fewer of them examined.
        let after = ScanMetrics::default();
        let rows_after = ParquetReader::new(&events)
            .with_predicate(predicate)
            .with_metrics(after.clone())
            .read_batches()
            .unwrap()
            .iter()
            .map(RecordBatch::num_rows)
            .sum::<usize>();
        assert_eq!(rows_after, rows_before);
        let (before, after) = (before.snapshot(), after.snapshot());
        assert!(
            after.row_filter_rows_examined < before.row_filter_rows_examined,
            "examined {} before, {} after",
            before.row_filter_rows_examined,
            after.row_filter_rows_examined
        );
        assert!(after.compressed_bytes_selected < before.compressed_bytes_selected);

        // A second OPTIMIZE finds nothing out of order and rewrites the
        // same rows again; the table is unchanged.
        let (row, spill) = optimize(&state, &admin(), "OPTIMIZE lake.sales.events", 256 << 20)
            .await
            .unwrap();
        assert_eq!(row[3], serde_json::json!(3 * ROWS_PER_FILE));
        assert_eq!(
            spill.runs_written, 0,
            "a budget that holds the rows spills nothing"
        );
        let _ = std::fs::remove_dir_all(&base);
    }

    #[tokio::test]
    async fn optimize_where_rewrites_the_files_the_predicate_may_touch() {
        let base = base("where");
        lake(&base);
        let state = state(&base);
        ddl(&state, "CREATE SCHEMA lake.sales").await;
        ddl(
            &state,
            "CREATE TABLE events WITH (location = 'events', format = 'parquet')",
        )
        .await;
        ddl(&state, "ALTER TABLE events SET CLUSTERED BY (key)").await;
        let events = base.join("events");
        let (row, _) = optimize(
            &state,
            &admin(),
            "OPTIMIZE events WHERE key >= 40000 AND region IS NOT NULL",
            256 << 20,
        )
        .await
        .unwrap();
        assert_eq!(row[1], serde_json::json!(1), "one file selected");
        assert_eq!(row[3], serde_json::json!(ROWS_PER_FILE));
        let files = data_files(&events);
        let names = files
            .iter()
            .map(|file| file.file_name().unwrap().to_str().unwrap().to_owned())
            .collect::<Vec<_>>();
        assert!(names.contains(&"part-0.parquet".to_owned()), "{names:?}");
        assert!(names.contains(&"part-1.parquet".to_owned()), "{names:?}");
        assert!(!names.contains(&"part-2.parquet".to_owned()), "{names:?}");
        let rewritten = files
            .iter()
            .find(|file| {
                !file
                    .file_name()
                    .unwrap()
                    .to_str()
                    .unwrap()
                    .starts_with("part-")
            })
            .or_else(|| {
                files.iter().find(|file| {
                    let name = file.file_name().unwrap().to_str().unwrap();
                    name.starts_with("part-") && name.len() > "part-0.parquet".len()
                })
            })
            .unwrap();
        let (keys, _) = keys(rewritten);
        assert!(keys.windows(2).all(|pair| pair[0] <= pair[1]));
        assert_eq!(keys.len() as i64, ROWS_PER_FILE);

        // Nothing selected: nothing rewritten, no error.
        let (row, _) = optimize(&state, &admin(), "OPTIMIZE events WHERE key < 0", 256 << 20)
            .await
            .unwrap();
        assert_eq!(row[1], serde_json::json!(0));
        assert_eq!(row[2], serde_json::json!(0));

        // A predicate the storage layer cannot evaluate is refused by name,
        // as is an unknown column.
        let error = optimize(
            &state,
            &admin(),
            "OPTIMIZE events WHERE key + 1 = 2",
            1 << 20,
        )
        .await
        .unwrap_err();
        assert_eq!(error.code, "OPTIMIZE_INVALID");
        assert!(
            error.message.contains("file statistics"),
            "{}",
            error.message
        );
        let error = optimize(&state, &admin(), "OPTIMIZE events WHERE nope = 1", 1 << 20)
            .await
            .unwrap_err();
        assert_eq!(error.code, "OPTIMIZE_INVALID", "{}", error.message);
        let _ = std::fs::remove_dir_all(&base);
    }

    /// A Hive-partitioned directory: `WHERE` on the partition column
    /// selects by the paths, the rewritten files land under their own
    /// partition directory, the rest of the table is untouched, and the
    /// table still reads as partitioned. Clustering by the partition
    /// column is refused.
    #[tokio::test]
    async fn optimize_rewrites_a_partitioned_directory_one_partition_at_a_time() {
        let base = base("partitioned");
        lake(&base);
        let state = state(&base);
        ddl(&state, "CREATE SCHEMA lake.sales").await;
        ddl(
            &state,
            "CREATE TABLE parts WITH (location = 'parts', format = 'parquet', clustered_by = ARRAY['key'])",
        )
        .await;
        let parts = base.join("parts");
        let (row, _) = optimize(
            &state,
            &admin(),
            "OPTIMIZE parts WITH (row_group_rows = 5000) WHERE dt = '2026-02' AND key >= 0",
            256 << 20,
        )
        .await
        .unwrap();
        assert_eq!(
            row[1],
            serde_json::json!(1),
            "one partition's file replaced"
        );
        assert_eq!(row[3], serde_json::json!(ROWS_PER_FILE));
        assert!(parts.join("dt=2026-01").join("part-0.parquet").exists());
        assert!(parts.join("dt=2026-03").join("part-0.parquet").exists());
        assert!(!parts.join("dt=2026-02").join("part-0.parquet").exists());
        let rewritten = data_files(&parts.join("dt=2026-02"));
        assert_eq!(rewritten.len() as u64, row[2].as_u64().unwrap());
        assert!(data_files(&parts).is_empty(), "nothing lands at the root");
        assert!(
            !parts
                .join(kaveon_storage::table_rewrite::REWRITE_AREA)
                .exists()
        );
        let mut all = Vec::new();
        for file in &rewritten {
            let (keys, _) = keys(file);
            assert!(keys.windows(2).all(|pair| pair[0] <= pair[1]), "{file:?}");
            all.extend(keys);
        }
        all.sort_unstable();
        let mut expected = scattered(ROWS_PER_FILE)
            .column(0)
            .as_any()
            .downcast_ref::<Int64Array>()
            .unwrap()
            .values()
            .to_vec();
        expected.sort_unstable();
        assert_eq!(all, expected);
        // The directory reader still sees one partitioned table of three
        // partitions, the rewritten one included.
        let schema = state
            .catalog
            .read()
            .await
            .manager
            .resolve_table(&kaveon_core::TableReference::Full {
                catalog: "lake".into(),
                schema: "sales".into(),
                table: "parts".into(),
            })
            .unwrap()
            .table
            .arrow_schema
            .clone();
        assert_eq!(schema.fields().last().unwrap().name(), "dt");
        let rows = ParquetReader::new(&parts)
            .with_catalog_schema(schema)
            .with_predicate(StoragePredicate::Compare {
                column: "dt".into(),
                op: CompareOp::Eq,
                value: ScalarValue::Utf8("2026-02".into()),
            })
            .read_batches()
            .unwrap()
            .iter()
            .map(RecordBatch::num_rows)
            .sum::<usize>();
        assert_eq!(rows as i64, ROWS_PER_FILE);

        ddl(&state, "ALTER TABLE parts SET CLUSTERED BY (dt)").await;
        let error = optimize(&state, &admin(), "OPTIMIZE parts", 1 << 20)
            .await
            .unwrap_err();
        assert_eq!(error.code, "OPTIMIZE_INVALID");
        assert!(
            error.message.contains("partition column"),
            "{}",
            error.message
        );
        let _ = std::fs::remove_dir_all(&base);
    }

    #[tokio::test]
    async fn optimize_replaces_a_single_file_and_refuses_what_it_cannot_rewrite() {
        let base = base("single");
        lake(&base);
        let state = state(&base);
        ddl(&state, "CREATE SCHEMA lake.sales").await;
        ddl(
            &state,
            "CREATE TABLE hits WITH (location = 'hits.parquet', format = 'parquet', clustered_by = ARRAY['key'])",
        )
        .await;
        ddl(
            &state,
            "CREATE TABLE log WITH (location = 'log', format = 'delta', clustered_by = ARRAY['key'])",
        )
        .await;
        ddl(
            &state,
            "CREATE TABLE plain WITH (location = 'events', format = 'parquet')",
        )
        .await;

        let hits = base.join("hits.parquet");
        let (before, groups_before) = keys(&hits);
        assert_eq!(groups_before, 1);
        let (row, _) = optimize(&state, &admin(), "OPTIMIZE hits", 256 << 20)
            .await
            .unwrap();
        assert_eq!(row[1], serde_json::json!(1));
        assert_eq!(row[2], serde_json::json!(1));
        let (after, _) = keys(&hits);
        assert!(after.windows(2).all(|pair| pair[0] <= pair[1]));
        let mut sorted = before;
        sorted.sort_unstable();
        assert_eq!(after, sorted);
        assert!(data_files(&base).len() == 1);

        // A table without clustering columns is compacted, not sorted.
        let (row, _) = optimize(&state, &admin(), "OPTIMIZE plain", 256 << 20)
            .await
            .unwrap();
        assert_eq!(row[1], serde_json::json!(3));
        assert_eq!(row[7], serde_json::json!(""));

        let error = optimize(&state, &admin(), "OPTIMIZE log", 1 << 20)
            .await
            .unwrap_err();
        assert_eq!(error.code, "OPTIMIZE_UNSUPPORTED");
        assert!(error.message.contains("Delta"), "{}", error.message);
        let error = optimize(&state, &analyst(), "OPTIMIZE hits", 1 << 20)
            .await
            .unwrap_err();
        assert_eq!(error.code, "FORBIDDEN");
        let error = optimize(&state, &admin(), "OPTIMIZE missing", 1 << 20)
            .await
            .unwrap_err();
        assert_eq!(error.code, "TABLE_NOT_FOUND");
        let _ = std::fs::remove_dir_all(&base);
    }
}
