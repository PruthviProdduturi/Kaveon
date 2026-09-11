use crate::AppState;
use crate::cluster::{NodeInfo, NodeRole};
use crate::lifecycle::{CancellationToken, TaskClaim, TaskOutcome, TaskOwner};
use crate::security::Identity;
use axum::body::Body;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, post};
use axum::{Extension, Json, Router};
use futures::StreamExt;
use kaveon_catalog::{
    CascadePolicy,
    product_commit::{CommitOutcome, ProductDocuments},
    product_manifest::{
        CatalogChange, ImmutableFileRef, PrepareChange, RuntimeTableSourceRef, TableStatisticsRef,
    },
};
use kaveon_core::collect_batches;
use kaveon_core::{
    AdmittedQueryMemory, CatalogDefinition, CatalogId, CatalogLifecycle, CatalogRevision,
    ColumnDefinition, ExchangeId, ExecutableFragment, MemoryAdmissionController, SchemaDefinition,
    SchemaId, StageId, TableDefinition, TableId, TaskId,
};
use kaveon_exec::sort::SortExpr;
use kaveon_exec::topn::merge_top_n;
use kaveon_sql::logical_plan::sql_to_logical_plan;
use kaveon_sql::logical_plan::{AggregateExpr, LogicalPlan};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::cmp::Reverse;
use std::collections::{BTreeMap, HashMap};
#[cfg(test)]
use std::io::Cursor;
use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
};
use std::time::Duration;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use uuid::Uuid;

use tokio::sync::RwLock;

use crate::orchestrator::{CoordinatorOrchestrator, TaskDispatch};

struct QueryStore {
    queries: HashMap<String, QueryRecord>,
}

#[derive(Clone, Serialize)]
struct QueryRecord {
    rows_are_preview: bool,
    scan_metrics_complete: bool,
    id: String,
    sql: String,
    state: QueryState,
    columns: Vec<ColumnInfo>,
    rows: Vec<Vec<serde_json::Value>>,
    error: Option<String>,
    elapsed_ms: u64,
    submitted_at_ms: u64,
    completed_at_ms: u64,
    timings: QueryTimings,
    plan: QueryPlan,
    scans: Vec<ScanTelemetry>,
    stages: Vec<StageTelemetry>,
    context: QueryContext,
}

#[derive(Clone, Serialize)]
struct StageTelemetry {
    stage_id: u32,
    state: &'static str,
    task_count: usize,
    completed_tasks: usize,
    elapsed_us: u64,
    tasks: Vec<TaskTelemetry>,
}

#[derive(Clone, Serialize)]
struct TaskTelemetry {
    task_id: String,
    node_id: String,
    partition_index: usize,
    elapsed_us: u64,
    output_rows: usize,
    output_batches: usize,
    output_bytes: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    execution: Option<TaskExecutionMetrics>,
    #[serde(skip_serializing_if = "Option::is_none")]
    scan: Option<TaskScanMetrics>,
}

#[derive(Clone, Default, Serialize, Deserialize)]
struct TaskExecutionMetrics {
    compute_cpu_us: Option<u64>,
    admission_wait_us: u64,
    exchange_input_payloads: u64,
    exchange_input_bytes: u64,
    exchange_fetch_us: u64,
    exchange_decode_batches: u64,
    exchange_decode_bytes: u64,
    exchange_decode_us: u64,
    exchange_output_copies: u64,
    exchange_output_bytes: u64,
    exchange_encode_us: u64,
    exchange_upload_us: u64,
    memory_peak_bytes: u64,
    memory_reservation_calls: u64,
    memory_reservation_bytes: u64,
    aggregate_input_rows: u64,
    aggregate_groups_created: u64,
    aggregate_distinct_values_admitted: u64,
    spill_peak_bytes: u64,
    spill_bytes_written: u64,
    spill_runs_written: u64,
    spill_compactions: u64,
    spill_compaction_input_bytes: u64,
}

#[derive(Default)]
struct ExchangeDecodeMetrics {
    batches: AtomicU64,
    bytes: AtomicU64,
    elapsed_us: AtomicU64,
}

/// Counters emitted by a worker's storage readers, never derived from query output.
#[derive(Clone, Default, Serialize, Deserialize)]
struct TaskScanMetrics {
    files_considered: u64,
    files_opened: u64,
    row_groups_considered: u64,
    row_groups_selected: u64,
    rows_selected: u64,
    rows_emitted: u64,
    compressed_bytes_selected: u64,
    batches_emitted: u64,
    snapshot_ns: u64,
    footer_ns: u64,
    read_ns: u64,
}

#[derive(Clone, Serialize)]
struct QueryContext {
    engine_version: String,
    environment: String,
    principal: Option<String>,
    user: Option<String>,
    source: Option<String>,
    client: Option<String>,
    catalog: String,
    schema: String,
    time_zone: Option<String>,
    client_address: Option<String>,
    client_tags: Vec<String>,
    result_delivery: Option<String>,
    catalog_snapshot_id: String,
}

#[derive(Clone, Serialize)]
struct ScanTelemetry {
    files_considered: u64,
    files_opened: u64,
    row_groups_considered: u64,
    row_groups_read: u64,
    row_groups_pruned: u64,
    rows_selected: u64,
    rows_emitted: u64,
    batches_emitted: u64,
    compressed_bytes_selected: u64,
    snapshot_ns: u64,
    footer_ns: u64,
    read_ns: u64,
    rows_per_second: f64,
    compressed_bytes_per_second: f64,
}

#[derive(Clone, Serialize)]
struct QueryTimings {
    analysis_us: Option<u64>,
    planning_us: Option<u64>,
    execution_us: Option<u64>,
    result_serialization_us: Option<u64>,
}

#[derive(Clone, Serialize)]
struct QueryPlan {
    logical: Option<kaveon_core::PlanNode>,
    optimized: Option<kaveon_core::PlanNode>,
    physical: Option<kaveon_core::PlanNode>,
}

const QUERY_HISTORY_LIMIT: usize = 100;

#[derive(Clone, Copy, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
enum QueryState {
    Running,
    Finished,
    Failed,
    Canceled,
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
struct ColumnInfo {
    name: String,
    #[serde(rename = "type")]
    data_type: String,
}

static QUERY_STORE: std::sync::LazyLock<RwLock<QueryStore>> = std::sync::LazyLock::new(|| {
    RwLock::new(QueryStore {
        queries: HashMap::new(),
    })
});

pub fn build_router(state: Arc<AppState>) -> Router {
    Router::new()
        .merge(crate::exchange::routes())
        .route("/v1/statement", post(submit_statement))
        .merge(crate::transaction_api::routes())
        .route("/v1/task", post(execute_task))
        .route(
            "/v1/internal/query/{query_id}/finish",
            post(finish_worker_query),
        )
        .route("/v1/query", get(list_queries))
        .route("/v1/query/{query_id}", get(get_query))
        .route("/v1/query/{query_id}/results/{page}", get(get_result_page))
        .route("/v1/query/{query_id}", delete(cancel_query))
        .route("/v1/cluster", get(get_cluster))
        .route("/v1/node", get(get_node))
        .route("/v1/node/heartbeat", post(receive_heartbeat))
        .route(
            "/v1/internal/catalog/snapshot",
            get(catalog_replica_snapshot),
        )
        .route("/v1/catalog", get(list_catalogs))
        .route(
            "/v1/catalog/definitions",
            get(list_catalog_definitions).post(create_catalog_definition),
        )
        .route(
            "/v1/catalog/definitions/{catalog_id}",
            get(get_catalog_definition)
                .put(replace_catalog_definition)
                .delete(delete_catalog_definition),
        )
        .route(
            "/v1/catalog/definitions/{catalog_id}/schemas",
            get(list_schema_definitions).post(create_schema_definition),
        )
        .route(
            "/v1/catalog/schemas/{schema_id}",
            get(get_schema_definition)
                .put(replace_schema_definition)
                .delete(delete_schema_definition),
        )
        .route(
            "/v1/catalog/schemas/{schema_id}/tables",
            get(list_table_definitions).post(create_table_definition),
        )
        .route(
            "/v1/catalog/tables/{table_id}",
            get(get_table_definition)
                .put(replace_table_definition)
                .delete(delete_table_definition),
        )
        .route("/v1/catalog/{catalog}/schema", get(list_schemas))
        .route(
            "/v1/catalog/{catalog}/schema/{schema}/table",
            get(list_tables),
        )
        .route("/ui", get(crate::ui::dashboard))
        .route("/ui/msal-browser.min.js", get(crate::ui::msal_script))
        .route("/v1/auth/config", get(crate::entra::public_config))
        .route("/v1/capabilities", get(capabilities))
        .route("/v1/statistics", get(statistics_diagnostics))
        .route("/health", get(health))
        .route("/ready", get(ready))
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            crate::security::authorize,
        ))
        .with_state(state)
}

// --- Statement Submission ---

#[derive(Deserialize)]
struct StatementRequest {
    query: String,
    #[serde(default)]
    catalog: Option<String>,
    #[serde(default)]
    schema: Option<String>,
    #[serde(default)]
    source: Option<String>,
    #[serde(default)]
    client: Option<String>,
    // Kept only for wire compatibility. The authenticated identity supplies query history user.
    #[serde(default)]
    user: Option<String>,
    #[serde(default)]
    time_zone: Option<String>,
    #[serde(default)]
    client_tags: Vec<String>,
    #[serde(default)]
    result_delivery: Option<String>,
}

#[derive(Serialize, Deserialize)]
struct TaskRequest {
    query_id: String,
    stage_id: u32,
    attempt: u32,
    #[serde(default)]
    query: String,
    #[serde(default)]
    catalog: String,
    #[serde(default)]
    schema: String,
    /// Deterministic identity of the coordinator's selected catalog view.
    /// Optional only for rolling compatibility with older task senders.
    #[serde(default)]
    catalog_snapshot_id: Option<String>,
    #[serde(default)]
    partition_index: usize,
    #[serde(default)]
    partition_count: usize,
    #[serde(default)]
    fragment: Option<ExecutableFragment>,
    #[serde(default)]
    execution_partition: Option<ExecutionPartitionRequest>,
    #[serde(default)]
    exchange_inputs: Vec<ExchangeLocationRequest>,
    #[serde(default)]
    exchange_outputs: Vec<ExchangeLocationRequest>,
}

#[derive(Clone, Copy, Serialize, Deserialize)]
struct ExecutionPartitionRequest {
    index: usize,
    count: usize,
}

#[derive(Clone, Serialize, Deserialize)]
struct ExchangeLocationRequest {
    exchange_id: ExchangeId,
    producer: TaskId,
    output_partition: usize,
    worker_uri: String,
}

#[derive(Serialize)]
struct TaskResponse {
    columns: Vec<ColumnInfo>,
    data: Vec<Vec<serde_json::Value>>,
    elapsed_us: u64,
}

#[derive(Serialize)]
struct StatementResponse {
    #[serde(skip_serializing_if = "Option::is_none")]
    next_uri: Option<String>,
    id: String,
    state: QueryState,
    #[serde(skip_serializing_if = "Option::is_none")]
    columns: Option<Vec<ColumnInfo>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    data: Option<Vec<Vec<serde_json::Value>>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
    elapsed_ms: u64,
}

async fn execute_task(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(req): Json<TaskRequest>,
) -> impl IntoResponse {
    if state.config.coordinator {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({
                "error": "partition tasks must be submitted to a worker",
                "code": "NOT_WORKER"
            })),
        )
            .into_response();
    }
    if req.fragment.is_some() {
        let expected = state.config.exchange_token.as_deref().unwrap_or_default();
        if crate::exchange::validate_bearer_header(
            headers
                .get(header::AUTHORIZATION)
                .and_then(|value| value.to_str().ok()),
            expected,
        )
        .is_err()
        {
            return StatusCode::UNAUTHORIZED.into_response();
        }
    }
    let task_id = TaskId {
        query_id: req.query_id.clone(),
        stage_id: StageId(req.stage_id),
        partition: req.partition_index,
        attempt: req.attempt,
    };
    let cancellation = match state.lifecycle.cancellations.token(&req.query_id) {
        Ok(token) => token,
        Err(error) => return lifecycle_error_response(error.to_string()),
    };
    if cancellation.is_cancelled() {
        return canceled_task_response();
    }
    let claim = match state.lifecycle.tasks.claim(task_id) {
        Ok(claim) => claim,
        Err(error) => return lifecycle_error_response(error.to_string()),
    };
    let owner = match claim {
        TaskClaim::Owner(owner) => owner,
        TaskClaim::Completed(outcome) => return task_outcome_response(outcome),
        TaskClaim::Waiter(waiter) => {
            return tokio::select! {
                outcome = waiter.wait() => match outcome {
                    Ok(outcome) => task_outcome_response(outcome),
                    Err(error) => lifecycle_error_response(error.to_string()),
                },
                () = cancellation.cancelled() => canceled_task_response(),
            };
        }
    };
    execute_owned_task(&state, req, owner, cancellation).await
}

async fn finish_worker_query(
    State(state): State<Arc<AppState>>,
    Path(query_id): Path<String>,
    headers: HeaderMap,
) -> Response {
    let expected = state.config.exchange_token.as_deref();
    let supplied = headers
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "));
    if expected.is_none() || supplied != expected {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    match state.lifecycle.finish_query(&query_id) {
        Ok(_) => StatusCode::NO_CONTENT.into_response(),
        Err(error) => lifecycle_error_response(error.to_string()),
    }
}

async fn execute_owned_task(
    state: &Arc<AppState>,
    req: TaskRequest,
    owner: TaskOwner<crate::transport::CachedTaskResult>,
    cancellation: CancellationToken,
) -> Response {
    if let Err(response) = validate_task_catalog_snapshot(state, &req).await {
        let _ = owner.complete(TaskOutcome::Failed(Arc::from(
            "worker catalog snapshot does not match coordinator",
        )));
        return *response;
    }
    let requested_partition = req
        .execution_partition
        .unwrap_or(ExecutionPartitionRequest {
            index: req.partition_index,
            count: req.partition_count,
        });
    let partition = match kaveon_storage::ScanPartition::new(
        requested_partition.index,
        requested_partition.count,
    ) {
        Ok(partition) => partition,
        Err(error) => {
            let message = error.to_string();
            let _ = owner.complete(TaskOutcome::Failed(Arc::from(message.clone())));
            return task_failure_response(StatusCode::BAD_REQUEST, &message);
        }
    };
    let admission_started = Instant::now();
    let admitted = match await_task_memory(
        &state.memory_admission,
        format!(
            "{}:{}:{}:{}",
            req.query_id, req.stage_id, req.partition_index, req.attempt
        ),
        state.config.query_memory_limit_bytes,
        &cancellation,
    )
    .await
    {
        Ok(admitted) => admitted,
        Err(error) => {
            let message = error;
            let _ = owner.complete(TaskOutcome::Failed(Arc::from(message.clone())));
            return if cancellation.is_cancelled() {
                canceled_task_response()
            } else {
                task_failure_response(StatusCode::SERVICE_UNAVAILABLE, &message)
            };
        }
    };
    let memory_cancellation = cancellation.clone();
    if let Err(error) = admitted
        .pool()
        .set_cancellation_probe(move || memory_cancellation.is_cancelled())
    {
        let message = error.to_string();
        let _ = owner.complete(TaskOutcome::Failed(Arc::from(message.clone())));
        return lifecycle_error_response(message);
    }
    let started = Instant::now();
    if let Some(fragment) = req.fragment.as_ref() {
        let result = execute_fragment_task(
            state,
            &req,
            fragment,
            partition,
            admitted.pool(),
            elapsed_us(admission_started),
        )
        .await;
        if cancellation.is_cancelled() {
            let _ = owner.complete(TaskOutcome::Failed(Arc::from("query canceled")));
            return canceled_task_response();
        }
        return complete_owned_task(owner, started, result);
    }
    let mut plan = match sql_to_logical_plan(req.query.trim().trim_end_matches(';')) {
        Ok(plan) => plan,
        Err(error) => {
            let message = error.to_string();
            let _ = owner.complete(TaskOutcome::Failed(Arc::from(message.clone())));
            return task_failure_response(StatusCode::BAD_REQUEST, &message);
        }
    };
    crate::planner::qualify_tables(&mut plan, &req.catalog, &req.schema);
    let plan = kaveon_optim::rules::push_filter_down(plan);
    let plan = kaveon_optim::rules::push_projection_down(plan);
    let plan = {
        let catalog = state.catalog.read().await;
        kaveon_optim::statistics::optimize_join_builds(plan, &catalog)
    };
    let execution_state = Arc::clone(state);
    let result = tokio::task::spawn_blocking(move || {
        let catalog = execution_state.catalog.blocking_read();
        let result = crate::planner::plan_partitioned_query_with_memory(
            &plan,
            &catalog,
            partition,
            admitted.pool(),
        )
        .and_then(|mut planned| {
            let schema = planned.operator.schema().clone();
            collect_batches(&mut *planned.operator).map(|batches| {
                (
                    schema,
                    batches,
                    merge_task_scan_metrics(planned.scan_metrics.iter()),
                )
            })
        });
        (result, admitted)
    })
    .await;
    let (result, _admitted) = match result {
        Ok(result) => result,
        Err(error) => {
            let message = format!("task execution failed: {error}");
            let _ = owner.complete(TaskOutcome::Failed(Arc::from(message.clone())));
            return task_failure_response(StatusCode::INTERNAL_SERVER_ERROR, &message);
        }
    };
    if cancellation.is_cancelled() {
        let _ = owner.complete(TaskOutcome::Failed(Arc::from("query canceled")));
        return canceled_task_response();
    }
    match result {
        Ok((schema, batches, scan)) => match encode_arrow_stream(&schema, &batches) {
            Ok(bytes) => {
                let elapsed = elapsed_us(started);
                let scan_metrics_header = serde_json::to_string(&scan).ok();
                let cached = match crate::transport::CachedTaskResult::new(
                    bytes,
                    elapsed,
                    scan_metrics_header,
                    None,
                ) {
                    Ok(cached) => cached,
                    Err(error) => {
                        let _ = owner.complete(TaskOutcome::Failed(Arc::from(error.clone())));
                        return task_failure_response(StatusCode::SERVICE_UNAVAILABLE, &error);
                    }
                };
                let outcome = TaskOutcome::Success(Arc::new(cached));
                let response = task_outcome_response(outcome.clone());
                let _ = owner.complete(outcome);
                response
            }
            Err(error) => {
                let _ = owner.complete(TaskOutcome::Failed(Arc::from(error.clone())));
                task_failure_response(StatusCode::INTERNAL_SERVER_ERROR, &error)
            }
        },
        Err(error) => {
            let message = error.to_string();
            let _ = owner.complete(TaskOutcome::Failed(Arc::from(message.clone())));
            task_failure_response(StatusCode::INTERNAL_SERVER_ERROR, &message)
        }
    }
}

/// Admission pressure is backpressure, not a task failure. A worker can have all
/// of its memory budget in use while another stage of the same distributed query
/// becomes ready. Returning 429 made the coordinator burn through its bounded
/// fault retries before any running task released memory. Keep the request queued
/// at the worker and remain cancellation-responsive instead.
async fn await_task_memory(
    admission: &MemoryAdmissionController,
    task_id: String,
    limit_bytes: u64,
    cancellation: &CancellationToken,
) -> Result<AdmittedQueryMemory, String> {
    const RETRY_INTERVAL: Duration = Duration::from_millis(10);
    loop {
        match admission.admit(task_id.clone(), limit_bytes) {
            Ok(admitted) => return Ok(admitted),
            Err(error) if cancellation.is_cancelled() => return Err(error.to_string()),
            Err(_) => {
                tokio::select! {
                    () = cancellation.cancelled() => {
                        return Err("query canceled while waiting for memory admission".into());
                    }
                    () = tokio::time::sleep(RETRY_INTERVAL) => {}
                }
            }
        }
    }
}

async fn validate_task_catalog_snapshot(
    state: &AppState,
    request: &TaskRequest,
) -> Result<(), Box<Response>> {
    let Some(expected) = request.catalog_snapshot_id.as_deref() else {
        if request.fragment.is_some() {
            return Ok(());
        }
        return Err(Box::new(task_failure_response(
            StatusCode::BAD_REQUEST,
            "raw SQL task requires catalog_snapshot_id",
        )));
    };
    let catalog = state.catalog.read().await;
    let actual = &catalog.snapshot_id;
    if actual != expected {
        return Err(Box::new(
            (
                StatusCode::CONFLICT,
                Json(serde_json::json!({
                    "error": "worker catalog snapshot does not match coordinator",
                    "code": "CATALOG_SNAPSHOT_MISMATCH"
                })),
            )
                .into_response(),
        ));
    }
    Ok(())
}

#[cfg(test)]
fn catalog_snapshot_identity(
    manager: &kaveon_core::CatalogManager,
    catalog_name: &str,
) -> kaveon_core::Result<String> {
    let catalog = manager.catalog(catalog_name).ok_or_else(|| {
        kaveon_core::KaveonError::Execution(format!("catalog '{catalog_name}' not found"))
    })?;
    let mut schemas = catalog.schema_names();
    schemas.sort_unstable();
    let mut digest = Sha256::new();
    digest.update(b"kaveon-catalog-snapshot-v2");
    digest_field(&mut digest, catalog_name.as_bytes());
    match catalog.storage_type() {
        kaveon_core::StorageType::Local { base_path } => {
            digest_field(&mut digest, b"local");
            digest_field(&mut digest, base_path.to_string_lossy().as_bytes());
        }
        kaveon_core::StorageType::AdlsGen2 {
            account,
            container,
            root_path,
        } => {
            digest_field(&mut digest, b"adls-gen2");
            digest_field(&mut digest, account.as_bytes());
            digest_field(&mut digest, container.as_bytes());
            digest_field(&mut digest, root_path.as_bytes());
        }
        kaveon_core::StorageType::S3 {
            bucket,
            region,
            prefix,
        } => {
            digest_field(&mut digest, b"s3");
            digest_field(&mut digest, bucket.as_bytes());
            digest_field(&mut digest, region.as_bytes());
            digest_field(&mut digest, prefix.as_bytes());
        }
    }
    for schema in schemas {
        digest_field(&mut digest, schema.as_bytes());
        let mut tables = catalog.table_names(&schema)?;
        tables.sort_unstable();
        for table_name in tables {
            let table = catalog.table(&schema, &table_name)?.ok_or_else(|| {
                kaveon_core::KaveonError::Execution(format!(
                    "table '{catalog_name}.{schema}.{table_name}' disappeared while identifying catalog snapshot"
                ))
            })?;
            digest_field(&mut digest, table_name.as_bytes());
            digest_field(&mut digest, table.location.as_bytes());
            digest_field(
                &mut digest,
                match table.access {
                    kaveon_core::AccessPattern::Shortcut => b"shortcut",
                    kaveon_core::AccessPattern::Optimized => b"optimized",
                },
            );
            digest_field(
                &mut digest,
                match table.format {
                    kaveon_core::DataFormat::Parquet => b"parquet",
                    kaveon_core::DataFormat::Delta => b"delta",
                    kaveon_core::DataFormat::Iceberg => b"iceberg",
                },
            );
            let schema = serde_json::to_value(table.arrow_schema.as_ref()).map_err(|error| {
                kaveon_core::KaveonError::Execution(format!(
                    "cannot encode table schema for catalog identity: {error}"
                ))
            })?;
            digest_canonical_json(&mut digest, &schema);
        }
    }
    Ok(format!("sha256:{:x}", digest.finalize()))
}

#[cfg(test)]
fn digest_field(digest: &mut Sha256, bytes: &[u8]) {
    digest.update((bytes.len() as u64).to_be_bytes());
    digest.update(bytes);
}

#[cfg(test)]
fn digest_canonical_json(digest: &mut Sha256, value: &serde_json::Value) {
    match value {
        serde_json::Value::Null => digest_field(digest, b"null"),
        serde_json::Value::Bool(value) => {
            digest_field(digest, if *value { b"true" } else { b"false" })
        }
        serde_json::Value::Number(value) => digest_field(digest, value.to_string().as_bytes()),
        serde_json::Value::String(value) => digest_field(digest, value.as_bytes()),
        serde_json::Value::Array(values) => {
            digest_field(digest, b"array");
            digest.update((values.len() as u64).to_be_bytes());
            for value in values {
                digest_canonical_json(digest, value);
            }
        }
        serde_json::Value::Object(values) => {
            digest_field(digest, b"object");
            digest.update((values.len() as u64).to_be_bytes());
            let mut keys = values.keys().collect::<Vec<_>>();
            keys.sort_unstable();
            for key in keys {
                digest_field(digest, key.as_bytes());
                digest_canonical_json(digest, &values[key]);
            }
        }
    }
}

struct PrefetchedExchangeInputs {
    inputs: HashMap<ExchangeId, Vec<crate::transport::ArrowPayload>>,
    memory: kaveon_core::OperatorMemoryAccount,
    decode_metrics: Arc<ExchangeDecodeMetrics>,
}

async fn buffered_ordered<T, U, E, F, Fut>(
    items: Vec<T>,
    concurrency: usize,
    operation: F,
) -> Vec<Result<U, E>>
where
    F: Fn(T) -> Fut,
    Fut: std::future::Future<Output = Result<U, E>>,
{
    futures::stream::iter(items.into_iter().map(operation))
        .buffered(concurrency.max(1))
        .collect()
        .await
}

struct DiskExchangeInput {
    schema: arrow::datatypes::SchemaRef,
    payloads: std::collections::VecDeque<crate::transport::ArrowPayload>,
    memory: kaveon_core::OperatorMemoryAccount,
    encoded: Option<kaveon_core::MemoryReservation>,
    decoded_extra: Option<kaveon_core::MemoryReservation>,
    metrics: Arc<ExchangeDecodeMetrics>,
}
impl kaveon_core::BatchOperator for DiskExchangeInput {
    fn schema(&self) -> &arrow::datatypes::SchemaRef {
        &self.schema
    }
    fn next_batch(&mut self) -> kaveon_core::Result<Option<arrow::record_batch::RecordBatch>> {
        self.decoded_extra = None;
        self.memory.check_cancelled()?;
        while let Some(payload) = self.payloads.front_mut() {
            if self.encoded.is_none() {
                self.encoded = Some(self.memory.reserve(payload.bytes() as u64)?);
            }
            let decode_started = Instant::now();
            if let Some(batch) = payload
                .next_batch()
                .map_err(kaveon_core::KaveonError::Execution)?
            {
                self.metrics
                    .elapsed_us
                    .fetch_add(elapsed_us(decode_started), Ordering::AcqRel);
                self.metrics.batches.fetch_add(1, Ordering::AcqRel);
                self.metrics
                    .bytes
                    .fetch_add(batch.get_array_memory_size() as u64, Ordering::AcqRel);
                let extra =
                    (batch.get_array_memory_size() as u64).saturating_sub(payload.bytes() as u64);
                if extra > 0 {
                    self.decoded_extra = Some(self.memory.reserve(extra)?);
                }
                return Ok(Some(batch));
            }
            self.payloads.pop_front();
            self.encoded = None;
        }
        Ok(None)
    }
}
impl crate::fragment_exec::ExchangeInputProvider for PrefetchedExchangeInputs {
    fn read(
        &self,
        _exchange_id: &ExchangeId,
    ) -> kaveon_core::Result<crate::fragment_exec::ExchangeBatches> {
        Err(kaveon_core::KaveonError::Execution(
            "disk exchange inputs require streaming open".into(),
        ))
    }
    fn open(
        &self,
        exchange_id: &ExchangeId,
    ) -> kaveon_core::Result<Box<dyn kaveon_core::BatchOperator>> {
        let inputs = self.inputs.get(exchange_id).ok_or_else(|| {
            kaveon_core::KaveonError::Execution(format!("missing exchange {}", exchange_id.0))
        })?;
        let schema = inputs
            .first()
            .ok_or_else(|| {
                kaveon_core::KaveonError::Execution("empty exchange payload set".into())
            })?
            .schema();
        let payloads = inputs
            .iter()
            .map(|payload| payload.fork().map_err(kaveon_core::KaveonError::Execution))
            .collect::<kaveon_core::Result<_>>()?;
        Ok(Box::new(DiskExchangeInput {
            schema,
            payloads,
            memory: self.memory.clone(),
            encoded: None,
            decoded_extra: None,
            metrics: Arc::clone(&self.decode_metrics),
        }))
    }
}

async fn execute_fragment_task(
    state: &Arc<AppState>,
    req: &TaskRequest,
    fragment: &ExecutableFragment,
    partition: kaveon_storage::ScanPartition,
    memory: &kaveon_core::QueryMemoryPool,
    admission_wait_us: u64,
) -> Result<
    (
        arrow::datatypes::SchemaRef,
        Vec<arrow::record_batch::RecordBatch>,
        Option<TaskScanMetrics>,
        TaskExecutionMetrics,
    ),
    String,
> {
    let mut metrics = TaskExecutionMetrics {
        admission_wait_us,
        ..Default::default()
    };
    let fetch_started = Instant::now();
    // A client owns its connection pool. Constructing one for every fragment
    // discarded reusable coordinator/exchange connections and put setup on the
    // critical path even for source stages with no exchange inputs.
    let client = state.internal_http_client.clone();
    let token = state
        .config
        .exchange_token
        .as_deref()
        .ok_or_else(|| "fragment execution requires an exchange bearer token".to_owned())?;
    let mut inputs = HashMap::<ExchangeId, Vec<crate::transport::ArrowPayload>>::new();
    let input_account = memory
        .operator("prefetched-exchanges")
        .map_err(|error| error.to_string())?;
    // A repartitioned join has one input per producer for both sides. Fetching
    // those spools serially put every network round trip and disk read on the
    // task's critical path. Keep a small fixed fan-out and `buffered` ordering:
    // latency overlaps without making producer order or memory use unbounded.
    let fetched = buffered_ordered(req.exchange_inputs.clone(), 8, |location| {
        let client = client.clone();
        async move {
            let identity = crate::exchange::ExchangeIdentity {
                exchange_id: location.exchange_id.clone(),
                task_id: location.producer.clone(),
                output_partition: location.output_partition,
            };
            let payload =
                crate::exchange::fetch_payload(&client, &location.worker_uri, token, &identity)
                    .await
                    .map_err(|error| {
                        format!(
                            "cannot fetch exchange '{}': {error}",
                            location.exchange_id.0
                        )
                    })?;
            Ok::<_, String>((location, payload))
        }
    })
    .await;
    for fetched in fetched {
        let (location, payload) = fetched?;
        metrics.exchange_input_payloads += 1;
        metrics.exchange_input_bytes = metrics
            .exchange_input_bytes
            .saturating_add(payload.bytes() as u64);
        let schema = payload.schema();
        let entry = inputs.entry(location.exchange_id.clone()).or_default();
        if let Some(first) = entry.first()
            && first.schema() != schema
        {
            return Err(format!(
                "exchange '{}' producers returned incompatible schemas: expected {:?}, producer {} returned {:?}",
                location.exchange_id.0,
                first.schema(),
                location.producer,
                schema
            ));
        }
        entry.push(payload);
    }
    metrics.exchange_fetch_us = elapsed_us(fetch_started);
    let decode_metrics = Arc::new(ExchangeDecodeMetrics::default());
    let spill = kaveon_exec::partitioned::spill_from_environment(memory)
        .map_err(|error| error.to_string())?
        .map(|(spill, _)| spill);
    let spill_before = spill.as_ref().map(|spill| spill.snapshot());
    let execution_state = Arc::clone(state);
    let execution_fragment = fragment.clone();
    let execution_memory = memory.clone();
    let worker_decode_metrics = Arc::clone(&decode_metrics);
    let execution = tokio::task::spawn_blocking(move || {
        let cpu_started = thread_cpu_us();
        let catalog = execution_state.catalog.blocking_read();
        let result = crate::fragment_exec::execute_fragment_with_memory(
            &execution_fragment,
            &catalog,
            &PrefetchedExchangeInputs {
                inputs,
                memory: input_account,
                decode_metrics: worker_decode_metrics,
            },
            partition,
            Some(&execution_memory),
        )
        .map_err(|error| error.to_string());
        let cpu_us =
            cpu_started.and_then(|started| thread_cpu_us().map(|end| end.saturating_sub(started)));
        (result, cpu_us)
    })
    .await
    .map_err(|error| format!("fragment execution task failed: {error}"))?;
    let (execution, compute_cpu_us) = execution;
    let execution = execution?;
    metrics.compute_cpu_us = compute_cpu_us;
    metrics.exchange_decode_batches = decode_metrics.batches.load(Ordering::Acquire);
    metrics.exchange_decode_bytes = decode_metrics.bytes.load(Ordering::Acquire);
    metrics.exchange_decode_us = decode_metrics.elapsed_us.load(Ordering::Acquire);
    for (exchange_id, output) in execution.exchange_outputs {
        for (output_partition, batches) in output.partitions.iter().enumerate() {
            let destinations = req.exchange_outputs.iter().filter(|location| {
                location.exchange_id == exchange_id && location.output_partition == output_partition
            });
            let mut destination_count = 0_usize;
            for destination in destinations {
                destination_count += 1;
                if destination.producer.query_id != req.query_id
                    || destination.producer.stage_id != StageId(req.stage_id)
                    || destination.producer.partition != requested_partition_index(req)
                    || destination.producer.attempt != req.attempt
                {
                    return Err(format!(
                        "exchange '{}' destination declares a producer that does not match this task",
                        exchange_id.0
                    ));
                }
                let identity = crate::exchange::ExchangeIdentity {
                    exchange_id: exchange_id.clone(),
                    task_id: destination.producer.clone(),
                    output_partition,
                };
                let encode_started = Instant::now();
                let chunks = crate::exchange::encode_batches(
                    identity,
                    &output.schema,
                    batches,
                    crate::exchange::ExchangeLimits::default(),
                )
                .map_err(|error| format!("cannot encode exchange '{}': {error}", exchange_id.0))?;
                metrics.exchange_encode_us = metrics
                    .exchange_encode_us
                    .saturating_add(elapsed_us(encode_started));
                metrics.exchange_output_copies += 1;
                metrics.exchange_output_bytes = metrics.exchange_output_bytes.saturating_add(
                    chunks
                        .iter()
                        .map(|chunk| chunk.payload.len() as u64)
                        .sum::<u64>(),
                );
                let upload_started = Instant::now();
                crate::exchange::upload_chunks(
                    &client,
                    &destination.worker_uri,
                    token,
                    &chunks,
                    crate::exchange::ExchangeLimits::default(),
                )
                .await
                .map_err(|error| format!("cannot upload exchange '{}': {error}", exchange_id.0))?;
                metrics.exchange_upload_us = metrics
                    .exchange_upload_us
                    .saturating_add(elapsed_us(upload_started));
            }
            if destination_count == 0 {
                return Err(format!(
                    "exchange '{}' output partition {output_partition} has no destination",
                    exchange_id.0
                ));
            }
        }
    }
    let scan = execution
        .scan_metrics_complete
        .then(|| merge_task_scan_metrics(execution.scan_metrics.iter()));
    let memory_snapshot = memory.snapshot();
    metrics.memory_peak_bytes = memory_snapshot.peak_bytes;
    metrics.memory_reservation_calls = memory_snapshot.reservation_calls;
    metrics.memory_reservation_bytes = memory_snapshot.reservation_bytes;
    let aggregate_metrics = kaveon_exec::aggregate::aggregate_metrics(memory)
        .map_err(|error| error.to_string())?
        .snapshot();
    metrics.aggregate_input_rows = aggregate_metrics.input_rows;
    metrics.aggregate_groups_created = aggregate_metrics.groups_created;
    metrics.aggregate_distinct_values_admitted = aggregate_metrics.distinct_values_admitted;
    if let (Some(before), Some(after)) = (spill_before, spill.map(|spill| spill.snapshot())) {
        metrics.spill_peak_bytes = after.peak_bytes;
        metrics.spill_bytes_written = after.bytes_written.saturating_sub(before.bytes_written);
        metrics.spill_runs_written = after.runs_written.saturating_sub(before.runs_written);
        metrics.spill_compactions = after.compactions.saturating_sub(before.compactions);
        metrics.spill_compaction_input_bytes = after
            .compaction_input_bytes
            .saturating_sub(before.compaction_input_bytes);
    }
    Ok((
        execution.result_schema,
        execution.result_batches,
        scan,
        metrics,
    ))
}

#[cfg(unix)]
fn thread_cpu_us() -> Option<u64> {
    let mut time = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    // SAFETY: `time` is a valid writable timespec and CLOCK_THREAD_CPUTIME_ID
    // does not retain the pointer.
    let status = unsafe { libc::clock_gettime(libc::CLOCK_THREAD_CPUTIME_ID, &mut time) };
    (status == 0).then(|| {
        (time.tv_sec as u64)
            .saturating_mul(1_000_000)
            .saturating_add((time.tv_nsec as u64) / 1_000)
    })
}

#[cfg(not(unix))]
fn thread_cpu_us() -> Option<u64> {
    None
}

fn requested_partition_index(req: &TaskRequest) -> usize {
    req.execution_partition
        .map(|partition| partition.index)
        .unwrap_or(req.partition_index)
}

fn complete_owned_task(
    owner: TaskOwner<crate::transport::CachedTaskResult>,
    started: Instant,
    result: Result<
        (
            arrow::datatypes::SchemaRef,
            Vec<arrow::record_batch::RecordBatch>,
            Option<TaskScanMetrics>,
            TaskExecutionMetrics,
        ),
        String,
    >,
) -> Response {
    match result {
        Ok((schema, batches, scan, execution)) => match encode_arrow_stream(&schema, &batches) {
            Ok(bytes) => {
                let elapsed = elapsed_us(started);
                let cached = match crate::transport::CachedTaskResult::new(
                    bytes,
                    elapsed,
                    scan.and_then(|scan| serde_json::to_string(&scan).ok()),
                    serde_json::to_string(&execution).ok(),
                ) {
                    Ok(cached) => cached,
                    Err(error) => {
                        let _ = owner.complete(TaskOutcome::Failed(Arc::from(error.clone())));
                        return task_failure_response(StatusCode::SERVICE_UNAVAILABLE, &error);
                    }
                };
                let outcome = TaskOutcome::Success(Arc::new(cached));
                let response = task_outcome_response(outcome.clone());
                let _ = owner.complete(outcome);
                response
            }
            Err(error) => {
                let _ = owner.complete(TaskOutcome::Failed(Arc::from(error.clone())));
                task_failure_response(StatusCode::INTERNAL_SERVER_ERROR, &error)
            }
        },
        Err(message) => {
            let _ = owner.complete(TaskOutcome::Failed(Arc::from(message.clone())));
            task_failure_response(StatusCode::INTERNAL_SERVER_ERROR, &message)
        }
    }
}

fn task_outcome_response(outcome: TaskOutcome<crate::transport::CachedTaskResult>) -> Response {
    match outcome {
        TaskOutcome::Success(result) => Response::builder()
            .status(StatusCode::OK)
            .header(header::CONTENT_TYPE, "application/vnd.apache.arrow.stream")
            .header("x-kaveon-task-elapsed-us", result.elapsed_us)
            .header(
                "x-kaveon-task-scan-metrics",
                result.scan_metrics_header.as_deref().unwrap_or(""),
            )
            .header(
                "x-kaveon-task-execution-metrics",
                result.execution_metrics_header.as_deref().unwrap_or(""),
            )
            .body(Body::from_stream(futures::stream::unfold(
                (result, 0usize),
                |(result, offset)| async move {
                    if offset >= result.bytes.len() {
                        return None;
                    }
                    let end = (offset + 64 * 1024).min(result.bytes.len());
                    let chunk = axum::body::Bytes::copy_from_slice(&result.bytes[offset..end]);
                    Some((Ok::<_, std::io::Error>(chunk), (result, end)))
                },
            )))
            .unwrap_or_else(|error| lifecycle_error_response(error.to_string())),
        TaskOutcome::Failed(message) => {
            task_failure_response(StatusCode::INTERNAL_SERVER_ERROR, &message)
        }
    }
}

fn canceled_task_response() -> Response {
    (
        StatusCode::CONFLICT,
        Json(serde_json::json!({ "error": "query canceled", "code": "QUERY_CANCELED" })),
    )
        .into_response()
}

fn lifecycle_error_response(message: String) -> Response {
    task_failure_response(StatusCode::SERVICE_UNAVAILABLE, &message)
}

fn task_failure_response(status: StatusCode, message: &str) -> Response {
    (status, Json(serde_json::json!({ "error": message }))).into_response()
}

// Release the bounded query registry on every statement exit, including
// parser/planner errors and dropped HTTP futures. Active blocking operators
// retain a token clone and observe cancellation cooperatively.
struct StatementLifecycleGuard {
    state: Arc<AppState>,
    query_id: String,
}
impl Drop for StatementLifecycleGuard {
    fn drop(&mut self) {
        let _ = self.state.lifecycle.cancellations.cancel(&self.query_id);
        let _ = self.state.lifecycle.finish_query(&self.query_id);
    }
}

async fn submit_statement(
    State(state): State<Arc<AppState>>,
    Extension(identity): Extension<Identity>,
    Json(req): Json<StatementRequest>,
) -> impl IntoResponse {
    let _submitted_user = req.user.as_deref();
    if !state.config.coordinator {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({
                "error": "queries must be submitted to the coordinator",
                "code": "NOT_COORDINATOR"
            })),
        )
            .into_response();
    }

    if !matches!(
        req.result_delivery.as_deref(),
        None | Some("inline") | Some("paged")
    ) {
        return (
            StatusCode::BAD_REQUEST,
            "result_delivery must be inline or paged",
        )
            .into_response();
    }
    prune_query_history().await;
    let paged = req.result_delivery.as_deref() == Some("paged");
    let _principal_permit = match state
        .principal_admission
        .admit(&identity.principal, state.config.principal_query_limit)
    {
        Ok(permit) => permit,
        Err(status) => return status.into_response(),
    };
    let _group_permit = match state
        .principal_admission
        .admit_group(&identity.principal, &state.config.security)
        .await
    {
        Ok(permit) => permit,
        Err(status) => return status.into_response(),
    };
    let query_id = Uuid::new_v4().to_string();
    let query_memory = match state
        .memory_admission
        .admit(query_id.clone(), state.config.query_memory_limit_bytes)
    {
        Ok(memory) => memory,
        Err(error) => {
            return (
                StatusCode::TOO_MANY_REQUESTS,
                Json(serde_json::json!({
                    "error": error.to_string(),
                    "code": "MEMORY_ADMISSION_REJECTED"
                })),
            )
                .into_response();
        }
    };
    let sql = req.query.trim().trim_end_matches(';').to_owned();
    let submitted_at_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64;
    let start = Instant::now();
    // Pin one immutable catalog manager for validation, optimization and
    // physical planning. Publishing a newer manager swaps the outer Arc and
    // cannot change the definitions observed by this query.
    let catalog_snapshot = state.catalog.read().await.clone();
    let requested_catalog = req
        .catalog
        .as_deref()
        .unwrap_or_else(|| catalog_snapshot.default_catalog());
    if catalog_snapshot.catalog(requested_catalog).is_none() {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({
                "error": format!("catalog '{requested_catalog}' not found"),
                "code": "CATALOG_NOT_FOUND"
            })),
        )
            .into_response();
    }
    let catalog_snapshot_id = catalog_snapshot.snapshot_id.clone();
    let context = {
        let catalog = &catalog_snapshot;
        let catalog_name = req
            .catalog
            .as_deref()
            .unwrap_or_else(|| catalog.default_catalog());
        let schema_name = req
            .schema
            .as_deref()
            .unwrap_or_else(|| catalog.default_schema());
        let Some(selected_catalog) = catalog.catalog(catalog_name) else {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({
                    "error": format!("catalog '{catalog_name}' not found"),
                    "code": "CATALOG_NOT_FOUND"
                })),
            )
                .into_response();
        };
        if !selected_catalog
            .schema_names()
            .iter()
            .any(|name| name == schema_name)
        {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({
                    "error": format!(
                        "schema '{schema_name}' not found in catalog '{catalog_name}'"
                    ),
                    "code": "SCHEMA_NOT_FOUND"
                })),
            )
                .into_response();
        }
        QueryContext {
            engine_version: env!("CARGO_PKG_VERSION").to_owned(),
            environment: state.config.environment.clone(),
            principal: Some(identity.principal.clone()),
            user: Some(identity.display_name().to_owned()),
            source: req.source,
            client: req.client,
            catalog: catalog_name.to_owned(),
            schema: schema_name.to_owned(),
            time_zone: req.time_zone,
            client_address: None,
            client_tags: req.client_tags,
            result_delivery: req.result_delivery,
            catalog_snapshot_id,
        }
    };
    let cancellation = match state.lifecycle.cancellations.token(&query_id) {
        Ok(token) => token,
        Err(error) => return lifecycle_error_response(error.to_string()),
    };
    let _lifecycle_guard = StatementLifecycleGuard {
        state: Arc::clone(&state),
        query_id: query_id.clone(),
    };
    let memory_cancellation = cancellation.clone();
    if let Err(error) = query_memory
        .pool()
        .set_cancellation_probe(move || memory_cancellation.is_cancelled())
    {
        return lifecycle_error_response(error.to_string());
    }

    QUERY_STORE.write().await.queries.insert(
        query_id.clone(),
        QueryRecord {
            rows_are_preview: true,
            scan_metrics_complete: false,
            id: query_id.clone(),
            sql: sql.clone(),
            state: QueryState::Running,
            columns: vec![],
            rows: vec![],
            error: None,
            elapsed_ms: 0,
            submitted_at_ms,
            completed_at_ms: 0,
            timings: QueryTimings {
                analysis_us: None,
                planning_us: None,
                execution_us: None,
                result_serialization_us: None,
            },
            plan: QueryPlan {
                logical: None,
                optimized: None,
                physical: None,
            },
            scans: vec![],
            stages: vec![],
            context: context.clone(),
        },
    );

    if let Some(table) = parse_analyze_table(&sql) {
        return execute_analyze(&state, &identity, &query_id, &context, table, start).await;
    }

    let analysis_start = Instant::now();
    let mut plan = match sql_to_logical_plan(&sql) {
        Ok(p) => p,
        Err(e) => {
            let message = format!("SQL parse error: {e}");
            finish_failed_query(&query_id, message.clone(), start, None, None, None).await;
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({
                    "error": message,
                    "code": "SYNTAX_ERROR"
                })),
            )
                .into_response();
        }
    };
    crate::planner::qualify_tables(&mut plan, &context.catalog, &context.schema);
    let analysis_us = elapsed_us(analysis_start);
    let logical_plan = crate::planner::logical_plan_tree(&plan);
    let plan = kaveon_optim::rules::push_filter_down(plan);
    let plan = kaveon_optim::rules::push_projection_down(plan);
    let plan = optimize_with_durable_statistics(&state, plan, &catalog_snapshot).await;
    let optimized_plan = crate::planner::optimized_plan_tree(&plan);
    let physical_plan = crate::planner::physical_plan_tree(&plan);
    if let Some(record) = QUERY_STORE.write().await.queries.get_mut(&query_id) {
        record.timings.analysis_us = Some(analysis_us);
        record.plan.logical = Some(logical_plan.clone());
        record.plan.optimized = Some(optimized_plan.clone());
        record.plan.physical = Some(physical_plan.clone());
    }

    if let Some(distributed) =
        execute_distributed_fragments(&state, &query_id, &context, &plan, &catalog_snapshot).await
    {
        match distributed {
            Ok((result, stages, planning_us)) => {
                let mut result = result;
                let next_uri = if paged {
                    match spool_rows(&state, &query_id, &identity.principal, &mut result.data) {
                        Ok(uri) => Some(uri),
                        Err(error) => {
                            finish_failed_query(
                                &query_id,
                                error.to_string(),
                                start,
                                Some(analysis_us),
                                None,
                                None,
                            )
                            .await;
                            cleanup_distributed_query(&state, &query_id).await;
                            return task_failure_response(
                                StatusCode::INSUFFICIENT_STORAGE,
                                "result disk quota or write failure",
                            );
                        }
                    }
                } else {
                    None
                };

                let elapsed = start.elapsed().as_millis() as u64;
                let (scans, scan_metrics_complete) = distributed_scan_telemetry(&stages);
                let record = QueryRecord {
                    rows_are_preview: true,
                    scan_metrics_complete,
                    id: query_id.clone(),
                    sql,
                    state: QueryState::Finished,
                    columns: result.columns.clone(),
                    rows: history_preview(&result.data),
                    error: None,
                    elapsed_ms: elapsed,
                    submitted_at_ms,
                    completed_at_ms: unix_time_ms(),
                    timings: QueryTimings {
                        analysis_us: Some(analysis_us),
                        planning_us: Some(planning_us),
                        execution_us: Some(result.elapsed_us),
                        result_serialization_us: None,
                    },
                    plan: QueryPlan {
                        logical: Some(logical_plan),
                        optimized: Some(optimized_plan),
                        physical: Some(physical_plan),
                    },
                    scans,
                    stages,
                    context,
                };
                if !commit_query_record(record).await {
                    state.results.remove(&query_id);
                    cleanup_distributed_query(&state, &query_id).await;
                    return canceled_task_response();
                }
                cleanup_distributed_query(&state, &query_id).await;
                return Json(StatementResponse {
                    next_uri,
                    id: query_id,
                    state: QueryState::Finished,
                    columns: Some(result.columns),
                    data: Some(result.data),
                    error: None,
                    elapsed_ms: elapsed,
                })
                .into_response();
            }
            Err(error) => {
                finish_failed_query(
                    &query_id,
                    error.clone(),
                    start,
                    Some(analysis_us),
                    None,
                    Some(logical_plan),
                )
                .await;
                cleanup_distributed_query(&state, &query_id).await;
                if cancellation.is_cancelled() {
                    return canceled_task_response();
                }
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(serde_json::json!({ "error": error, "code": "DISTRIBUTED_EXECUTION_ERROR" })),
                )
                    .into_response();
            }
        }
    }

    if let Some(distributed) = execute_distributed_aggregate(
        &state,
        &query_id,
        &sql,
        &context,
        &plan,
        query_memory.pool(),
    )
    .await
    {
        match distributed {
            Ok((result, stage)) => {
                let mut result = result;
                let next_uri = if paged {
                    match spool_rows(&state, &query_id, &identity.principal, &mut result.data) {
                        Ok(uri) => Some(uri),
                        Err(error) => {
                            finish_failed_query(
                                &query_id,
                                error.to_string(),
                                start,
                                Some(analysis_us),
                                None,
                                None,
                            )
                            .await;
                            cleanup_distributed_query(&state, &query_id).await;
                            return task_failure_response(
                                StatusCode::INSUFFICIENT_STORAGE,
                                "result disk quota or write failure",
                            );
                        }
                    }
                } else {
                    None
                };

                let elapsed = start.elapsed().as_millis() as u64;
                let stages = vec![stage];
                let (scans, scan_metrics_complete) = distributed_scan_telemetry(&stages);
                let record = QueryRecord {
                    rows_are_preview: true,
                    scan_metrics_complete,
                    id: query_id.clone(),
                    sql,
                    state: QueryState::Finished,
                    columns: result.columns.clone(),
                    rows: history_preview(&result.data),
                    error: None,
                    elapsed_ms: elapsed,
                    submitted_at_ms,
                    completed_at_ms: unix_time_ms(),
                    timings: QueryTimings {
                        analysis_us: Some(analysis_us),
                        planning_us: None,
                        execution_us: Some(result.elapsed_us),
                        result_serialization_us: None,
                    },
                    plan: QueryPlan {
                        logical: Some(logical_plan),
                        optimized: Some(optimized_plan),
                        physical: Some(physical_plan),
                    },
                    scans,
                    stages,
                    context,
                };
                if !commit_query_record(record).await {
                    state.results.remove(&query_id);
                    cleanup_distributed_query(&state, &query_id).await;
                    return canceled_task_response();
                }
                cleanup_distributed_query(&state, &query_id).await;
                return Json(StatementResponse {
                    next_uri,
                    id: query_id,
                    state: QueryState::Finished,
                    columns: Some(result.columns),
                    data: Some(result.data),
                    error: None,
                    elapsed_ms: elapsed,
                })
                .into_response();
            }
            Err(error) => {
                finish_failed_query(
                    &query_id,
                    error.clone(),
                    start,
                    Some(analysis_us),
                    None,
                    Some(logical_plan),
                )
                .await;
                cleanup_distributed_query(&state, &query_id).await;
                if cancellation.is_cancelled() {
                    return canceled_task_response();
                }
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(serde_json::json!({ "error": error, "code": "DISTRIBUTED_EXECUTION_ERROR" })),
                )
                    .into_response();
            }
        }
    }

    if let Some(distributed) =
        execute_distributed_top_n(&state, &query_id, &sql, &context, &plan).await
    {
        match distributed {
            Ok((result, stage)) => {
                let mut result = result;
                let next_uri = if paged {
                    match spool_rows(&state, &query_id, &identity.principal, &mut result.data) {
                        Ok(uri) => Some(uri),
                        Err(error) => {
                            finish_failed_query(
                                &query_id,
                                error.to_string(),
                                start,
                                Some(analysis_us),
                                None,
                                None,
                            )
                            .await;
                            cleanup_distributed_query(&state, &query_id).await;
                            return task_failure_response(
                                StatusCode::INSUFFICIENT_STORAGE,
                                "result disk quota or write failure",
                            );
                        }
                    }
                } else {
                    None
                };

                let elapsed = start.elapsed().as_millis() as u64;
                let stages = vec![stage];
                let (scans, scan_metrics_complete) = distributed_scan_telemetry(&stages);
                let record = QueryRecord {
                    rows_are_preview: true,
                    scan_metrics_complete,
                    id: query_id.clone(),
                    sql,
                    state: QueryState::Finished,
                    columns: result.columns.clone(),
                    rows: history_preview(&result.data),
                    error: None,
                    elapsed_ms: elapsed,
                    submitted_at_ms,
                    completed_at_ms: unix_time_ms(),
                    timings: QueryTimings {
                        analysis_us: Some(analysis_us),
                        planning_us: None,
                        execution_us: Some(result.elapsed_us),
                        result_serialization_us: None,
                    },
                    plan: QueryPlan {
                        logical: Some(logical_plan),
                        optimized: Some(optimized_plan),
                        physical: Some(physical_plan),
                    },
                    scans,
                    stages,
                    context,
                };
                if !commit_query_record(record).await {
                    state.results.remove(&query_id);
                    cleanup_distributed_query(&state, &query_id).await;
                    return canceled_task_response();
                }
                cleanup_distributed_query(&state, &query_id).await;
                return Json(StatementResponse {
                    next_uri,
                    id: query_id,
                    state: QueryState::Finished,
                    columns: Some(result.columns),
                    data: Some(result.data),
                    error: None,
                    elapsed_ms: elapsed,
                })
                .into_response();
            }
            Err(error) => {
                finish_failed_query(
                    &query_id,
                    error.clone(),
                    start,
                    Some(analysis_us),
                    None,
                    Some(logical_plan),
                )
                .await;
                cleanup_distributed_query(&state, &query_id).await;
                if cancellation.is_cancelled() {
                    return canceled_task_response();
                }
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(serde_json::json!({ "error": error, "code": "DISTRIBUTED_EXECUTION_ERROR" })),
                )
                    .into_response();
            }
        }
    }

    let mut result_writer = if paged {
        match state.results.writer() {
            Ok(writer) => Some(writer),
            Err(_) => return StatusCode::INSUFFICIENT_STORAGE.into_response(),
        }
    } else {
        None
    };
    let local_catalog_snapshot = Arc::clone(&catalog_snapshot);
    // Build non-Send operators inside the blocking task. Retain admission until
    // both execution and result publication complete, even if the HTTP future drops.
    let local_execution = tokio::task::spawn_blocking(move || {
        let mut local_columns = Vec::new();
        let planned_execution = {
            let planning_start = Instant::now();
            crate::planner::plan_query_with_memory(
                &plan,
                &local_catalog_snapshot,
                query_memory.pool(),
            )
            .map(|planned| {
                let planning_us = elapsed_us(planning_start);
                let scan_handles = planned.scan_metrics;
                let mut operator = planned.operator;
                local_columns = operator
                    .schema()
                    .fields()
                    .iter()
                    .map(|field| ColumnInfo {
                        name: field.name().clone(),
                        data_type: field.data_type().to_string(),
                    })
                    .collect();
                let execution_start = Instant::now();
                let result = if let Some(writer) = result_writer.as_mut() {
                    spool_operator(&mut *operator, writer)
                } else {
                    collect_inline_bounded(&mut *operator)
                };
                let execution_us = elapsed_us(execution_start);
                let scans = scan_handles.iter().map(scan_telemetry).collect::<Vec<_>>();
                (planning_us, execution_us, scans, result)
            })
        };
        (
            planned_execution,
            local_columns,
            result_writer,
            query_memory,
            _principal_permit,
            _group_permit,
        )
    })
    .await;
    let (
        planned_execution,
        local_columns,
        result_writer,
        _query_memory,
        _principal_permit,
        _group_permit,
    ) = match local_execution {
        Ok(execution) => execution,
        Err(error) => {
            finish_failed_query(
                &query_id,
                format!("local execution task failed: {error}"),
                start,
                Some(analysis_us),
                None,
                Some(logical_plan),
            )
            .await;
            return task_failure_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "local execution task failed",
            );
        }
    };
    let (planning_us, execution_us, scans, exec_result) = match planned_execution {
        Ok(execution) => execution,
        Err(error) => {
            let message = format!("planning error: {error}");
            finish_failed_query(
                &query_id,
                message.clone(),
                start,
                Some(analysis_us),
                None,
                Some(logical_plan),
            )
            .await;
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({
                    "error": message,
                    "code": "PLANNING_ERROR"
                })),
            )
                .into_response();
        }
    };

    let batches = match exec_result {
        Ok(b) => b,
        Err(e) => {
            let elapsed = start.elapsed().as_millis() as u64;
            let record = QueryRecord {
                rows_are_preview: true,
                scan_metrics_complete: true,
                id: query_id.clone(),
                sql: sql.clone(),
                state: QueryState::Failed,
                columns: vec![],
                rows: vec![],
                error: Some(format!("{e}")),
                elapsed_ms: elapsed,
                submitted_at_ms,
                completed_at_ms: unix_time_ms(),
                timings: QueryTimings {
                    analysis_us: Some(analysis_us),
                    planning_us: Some(planning_us),
                    execution_us: Some(execution_us),
                    result_serialization_us: None,
                },
                plan: QueryPlan {
                    logical: Some(logical_plan),
                    optimized: Some(optimized_plan),
                    physical: Some(physical_plan),
                },
                scans,
                stages: vec![],
                context: context.clone(),
            };
            if !commit_query_record(record).await {
                state.results.remove(&query_id);
                cleanup_distributed_query(&state, &query_id).await;
                return canceled_task_response();
            }
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({
                    "error": format!("execution error: {e}"),
                    "code": "EXECUTION_ERROR"
                })),
            )
                .into_response();
        }
    };

    let columns: Vec<ColumnInfo> = if paged {
        local_columns
    } else if let Some(first) = batches.first() {
        first
            .schema()
            .fields()
            .iter()
            .map(|f| ColumnInfo {
                name: f.name().clone(),
                data_type: format!("{}", f.data_type()),
            })
            .collect()
    } else {
        vec![]
    };

    let serialization_start = Instant::now();
    let rows = batches_to_json(&batches);
    let next_uri = if let Some(writer) = result_writer {
        if state
            .results
            .publish(&query_id, &identity.principal, writer)
            .is_err()
        {
            finish_failed_query(
                &query_id,
                "result disk quota or write failure".into(),
                start,
                Some(analysis_us),
                None,
                None,
            )
            .await;
            return StatusCode::INSUFFICIENT_STORAGE.into_response();
        }
        Some(format!("/v1/query/{query_id}/results/0"))
    } else {
        None
    };
    let result_serialization_us = elapsed_us(serialization_start);
    let elapsed = start.elapsed().as_millis() as u64;

    let record = QueryRecord {
        rows_are_preview: true,
        scan_metrics_complete: true,
        id: query_id.clone(),
        sql,
        state: QueryState::Finished,
        columns: columns.clone(),
        rows: history_preview(&rows),
        error: None,
        elapsed_ms: elapsed,
        submitted_at_ms,
        completed_at_ms: unix_time_ms(),
        timings: QueryTimings {
            analysis_us: Some(analysis_us),
            planning_us: Some(planning_us),
            execution_us: Some(execution_us),
            result_serialization_us: Some(result_serialization_us),
        },
        plan: QueryPlan {
            logical: Some(logical_plan),
            optimized: Some(optimized_plan),
            physical: Some(physical_plan),
        },
        scans,
        stages: vec![],
        context,
    };
    if !commit_query_record(record).await {
        state.results.remove(&query_id);
        cleanup_distributed_query(&state, &query_id).await;
        return canceled_task_response();
    }

    let resp = StatementResponse {
        next_uri,
        id: query_id,
        state: QueryState::Finished,
        columns: Some(columns),
        data: Some(rows),
        error: None,
        elapsed_ms: elapsed,
    };

    Json(resp).into_response()
}

fn parse_analyze_table(sql: &str) -> Option<String> {
    let rest = sql
        .strip_prefix("ANALYZE ")
        .or_else(|| sql.strip_prefix("analyze "))?
        .trim();
    if rest.is_empty() || rest.bytes().any(|b| b.is_ascii_whitespace() || b == b';') {
        return None;
    }
    let parts = rest
        .split('.')
        .map(|p| p.trim_matches('"'))
        .collect::<Vec<_>>();
    if !(1..=3).contains(&parts.len())
        || parts
            .iter()
            .any(|p| p.is_empty() || !p.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_'))
    {
        return None;
    }
    Some(parts.join("."))
}

async fn execute_analyze(
    state: &Arc<AppState>,
    identity: &Identity,
    query_id: &str,
    context: &QueryContext,
    table: String,
    started: Instant,
) -> Response {
    if identity.role != crate::security::Role::Admin {
        finish_failed_query(
            query_id,
            "ANALYZE requires admin role".into(),
            started,
            None,
            None,
            None,
        )
        .await;
        return (
            StatusCode::FORBIDDEN,
            Json(serde_json::json!({"error":"ANALYZE requires admin role","code":"FORBIDDEN"})),
        )
            .into_response();
    }
    let Some(commit) = state.product_transactions.catalog() else {
        finish_failed_query(
            query_id,
            "native ANALYZE is disabled".into(),
            started,
            None,
            None,
            None,
        )
        .await;
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(
                serde_json::json!({"error":"native ANALYZE is disabled","code":"ANALYZE_DISABLED"}),
            ),
        )
            .into_response();
    };
    let qualified = match table.split('.').count() {
        1 => format!("{}.{}.{}", context.catalog, context.schema, table),
        2 => format!("{}.{}", context.catalog, table),
        _ => table,
    };
    let resolved = match state
        .catalog
        .read()
        .await
        .resolve_table(&kaveon_core::TableReference::parse(&qualified))
    {
        Ok(value) => value,
        Err(error) => {
            return analyze_failure(
                query_id,
                started,
                StatusCode::BAD_REQUEST,
                "TABLE_NOT_FOUND",
                error.to_string(),
            )
            .await;
        }
    };
    let first = match kaveon_storage::analyze_source(&resolved.full_path(), resolved.table.format) {
        Ok(value) => value,
        Err(error) => {
            return analyze_failure(
                query_id,
                started,
                StatusCode::BAD_REQUEST,
                "ANALYZE_FAILED",
                error.to_string(),
            )
            .await;
        }
    };
    let second = match kaveon_storage::analyze_source(&resolved.full_path(), resolved.table.format)
    {
        Ok(value) if value.identity_sha256 == first.identity_sha256 => value,
        Ok(_) => {
            return analyze_failure(
                query_id,
                started,
                StatusCode::CONFLICT,
                "SOURCE_CHANGED",
                "table source changed during ANALYZE".into(),
            )
            .await;
        }
        Err(error) => {
            return analyze_failure(
                query_id,
                started,
                StatusCode::BAD_REQUEST,
                "ANALYZE_FAILED",
                error.to_string(),
            )
            .await;
        }
    };
    let catalog_snapshot_sha256 = format!(
        "{:x}",
        Sha256::digest(context.catalog_snapshot_id.as_bytes())
    );
    let document = serde_json::to_vec(&serde_json::json!({"version":1,"table":qualified,"catalog_snapshot_sha256":catalog_snapshot_sha256,"source_identity_sha256":second.identity_sha256,"row_count":second.row_count,"columns":second.columns})).unwrap();
    let document_sha = format!("{:x}", Sha256::digest(&document));
    let current = match commit.read_current().await {
        Ok(v) => v,
        Err(_) => {
            return analyze_failure(
                query_id,
                started,
                StatusCode::SERVICE_UNAVAILABLE,
                "CATALOG_UNAVAILABLE",
                "cannot read product catalog head".into(),
            )
            .await;
        }
    };
    let operation = Uuid::new_v4().simple().to_string();
    let path = format!("statistics/{operation}.json");
    let request = PrepareChange {
        base: current.reference(),
        snapshot_id: format!("analyze-{operation}"),
        operation_id: format!("analyze-{operation}"),
        request_digest: document_sha.clone(),
        changes: vec![
            CatalogChange::PutRuntimeTableSource {
                table: qualified.clone(),
                source: RuntimeTableSourceRef {
                    catalog_snapshot_sha256: catalog_snapshot_sha256.clone(),
                    source_identity_sha256: second.identity_sha256.clone(),
                },
            },
            CatalogChange::PutStatistics {
                table: qualified.clone(),
                statistics: TableStatisticsRef {
                    catalog_snapshot_sha256,
                    source_identity_sha256: second.identity_sha256,
                    document: ImmutableFileRef {
                        path: path.clone(),
                        sha256: document_sha,
                    },
                    row_count: second.row_count,
                },
            },
        ],
    };
    match commit
        .commit_with_documents(request, ProductDocuments::from([(path, document)]))
        .await
    {
        CommitOutcome::Committed(_) | CommitOutcome::Replayed(_) => {}
        CommitOutcome::Conflict => {
            return analyze_failure(
                query_id,
                started,
                StatusCode::CONFLICT,
                "CATALOG_CONFLICT",
                "catalog head changed during ANALYZE; retry".into(),
            )
            .await;
        }
        CommitOutcome::Rejected => {
            return analyze_failure(
                query_id,
                started,
                StatusCode::BAD_REQUEST,
                "ANALYZE_REJECTED",
                "statistics publication was rejected".into(),
            )
            .await;
        }
        CommitOutcome::Indeterminate => {
            return analyze_failure(
                query_id,
                started,
                StatusCode::SERVICE_UNAVAILABLE,
                "ANALYZE_INDETERMINATE",
                "statistics publication outcome is indeterminate".into(),
            )
            .await;
        }
    }
    let elapsed = started.elapsed().as_millis().try_into().unwrap_or(u64::MAX);
    let columns = vec![
        ColumnInfo {
            name: "table".into(),
            data_type: "VARCHAR".into(),
        },
        ColumnInfo {
            name: "row_count".into(),
            data_type: "BIGINT".into(),
        },
    ];
    let rows = vec![vec![
        serde_json::json!(qualified),
        serde_json::json!(second.row_count),
    ]];
    if let Some(record) = QUERY_STORE.write().await.queries.get_mut(query_id) {
        record.state = QueryState::Finished;
        record.columns = columns.clone();
        record.rows = rows.clone();
        record.elapsed_ms = elapsed;
        record.completed_at_ms = unix_time_ms();
    }
    Json(StatementResponse {
        next_uri: None,
        id: query_id.into(),
        state: QueryState::Finished,
        columns: Some(columns),
        data: Some(rows),
        error: None,
        elapsed_ms: elapsed,
    })
    .into_response()
}

async fn analyze_failure(
    query_id: &str,
    started: Instant,
    status: StatusCode,
    code: &str,
    message: String,
) -> Response {
    finish_failed_query(query_id, message.clone(), started, None, None, None).await;
    (
        status,
        Json(serde_json::json!({"error":message,"code":code})),
    )
        .into_response()
}

async fn capabilities(State(state): State<Arc<AppState>>) -> Json<serde_json::Value> {
    Json(
        serde_json::json!({"native_analyze": state.config.coordinator && state.product_transactions.catalog().is_some()}),
    )
}

const MAX_DIAGNOSTIC_STATISTICS: usize = 100;

#[derive(Debug, Serialize)]
struct StatisticsDiagnostic {
    table: String,
    row_count: u64,
    catalog_digest_prefix: String,
    source_digest_prefix: String,
    current: bool,
}

async fn statistics_diagnostics(
    State(state): State<Arc<AppState>>,
    Extension(identity): Extension<Identity>,
) -> Response {
    if identity.role != crate::security::Role::Admin {
        return StatusCode::FORBIDDEN.into_response();
    }
    let Some(commit) = state.product_transactions.catalog() else {
        return (StatusCode::SERVICE_UNAVAILABLE, Json(serde_json::json!({"error":"durable statistics are disabled","code":"STATISTICS_DISABLED"}))).into_response();
    };
    let snapshot = match commit.read_current().await {
        Ok(value) => value,
        Err(_) => return (StatusCode::SERVICE_UNAVAILABLE, Json(serde_json::json!({"error":"cannot read durable statistics","code":"STATISTICS_UNAVAILABLE"}))).into_response(),
    };
    let catalog = state.catalog.read().await.clone();
    let total = snapshot.table_statistics.len();
    let statistics = snapshot
        .table_statistics
        .iter()
        .take(MAX_DIAGNOSTIC_STATISTICS)
        .map(|(table, stored)| StatisticsDiagnostic {
            table: table.clone(),
            row_count: stored.row_count,
            catalog_digest_prefix: stored.catalog_snapshot_sha256.chars().take(12).collect(),
            source_digest_prefix: stored.source_identity_sha256.chars().take(12).collect(),
            current: durable_relation_statistics(&catalog, &snapshot, table).is_some(),
        })
        .collect::<Vec<_>>();
    Json(serde_json::json!({"statistics":statistics,"total":total,"truncated":total > MAX_DIAGNOSTIC_STATISTICS})).into_response()
}

async fn optimize_with_durable_statistics(
    state: &AppState,
    plan: LogicalPlan,
    catalog: &crate::PublishedCatalog,
) -> LogicalPlan {
    let mut tables = std::collections::BTreeSet::new();
    collect_join_statistics_tables(&plan, &mut tables);
    if tables.is_empty() {
        return plan;
    }
    let durable = match state.product_transactions.catalog() {
        Some(commit) => commit.read_current().await.ok(),
        None => None,
    };
    let mut loads = tokio::task::JoinSet::new();
    for table in tables {
        let Ok(resolved) = catalog.resolve_table(&kaveon_core::TableReference::parse(&table))
        else {
            continue;
        };
        let location = resolved.full_path();
        let format = resolved.table.format;
        let qualified = format!(
            "{}.{}.{}",
            resolved.catalog, resolved.schema, resolved.table.name
        );
        loads.spawn_blocking(move || {
            (
                table,
                qualified,
                kaveon_storage::analyze_source(&location, format).ok(),
            )
        });
    }
    let catalog_digest = format!("{:x}", Sha256::digest(catalog.snapshot_id.as_bytes()));
    let mut cache = HashMap::new();
    while let Some(loaded) = loads.join_next().await {
        let Ok((table, qualified, current)) = loaded else {
            continue;
        };
        let value = current.map(|current| {
            let rows = durable
                .as_ref()
                .and_then(|snapshot| snapshot.table_statistics.get(&qualified))
                .filter(|stored| {
                    stored.catalog_snapshot_sha256 == catalog_digest
                        && stored.source_identity_sha256 == current.identity_sha256
                })
                .map_or(current.row_count, |stored| stored.row_count);
            kaveon_optim::statistics::RelationStatistics {
                rows,
                columns: current.columns,
            }
        });
        cache.insert(table, value);
    }
    kaveon_optim::statistics::optimize_with_statistics(plan, &mut |table| {
        cache.get(table).cloned().flatten()
    })
}

/// Collects only relations for which the statistics optimizer will request
/// exact cardinality. Metadata reads are independent and can safely overlap;
/// every result remains bound to its own immutable source identity.
fn collect_join_statistics_tables(
    plan: &LogicalPlan,
    tables: &mut std::collections::BTreeSet<String>,
) {
    match plan {
        LogicalPlan::Join { left, right, .. } => {
            if let LogicalPlan::Scan { table, .. } = left.as_ref() {
                tables.insert(table.clone());
            }
            if let LogicalPlan::Scan { table, .. } = right.as_ref() {
                tables.insert(table.clone());
            }
            collect_join_statistics_tables(left, tables);
            collect_join_statistics_tables(right, tables);
        }
        LogicalPlan::Project { input, .. }
        | LogicalPlan::Filter { input, .. }
        | LogicalPlan::Aggregate { input, .. }
        | LogicalPlan::Sort { input, .. }
        | LogicalPlan::Limit { input, .. }
        | LogicalPlan::Offset { input, .. }
        | LogicalPlan::Distinct { input }
        | LogicalPlan::Window { input, .. } => collect_join_statistics_tables(input, tables),
        LogicalPlan::Union { inputs, .. } => {
            for input in inputs {
                collect_join_statistics_tables(input, tables);
            }
        }
        LogicalPlan::Intersect { left, right }
        | LogicalPlan::Except { left, right }
        | LogicalPlan::SemiJoin { left, right, .. }
        | LogicalPlan::AntiJoin { left, right, .. } => {
            collect_join_statistics_tables(left, tables);
            collect_join_statistics_tables(right, tables);
        }
        LogicalPlan::Scan { .. } => {}
    }
}

/// Derives exact planning statistics directly from the immutable source
/// metadata when no current ANALYZE publication exists. The caller caches the
/// result for the planning pass, so repeated references to one relation do not
/// reopen its metadata. Failures stay conservative and retain partitioned joins.
#[cfg(test)]
fn exact_source_statistics(
    catalog: &crate::PublishedCatalog,
    table: &str,
) -> Option<kaveon_optim::statistics::RelationStatistics> {
    let resolved = catalog
        .resolve_table(&kaveon_core::TableReference::parse(table))
        .ok()?;
    let current =
        kaveon_storage::analyze_source(&resolved.full_path(), resolved.table.format).ok()?;
    Some(kaveon_optim::statistics::RelationStatistics {
        rows: current.row_count,
        columns: current.columns,
    })
}

fn durable_relation_statistics(
    catalog: &crate::PublishedCatalog,
    durable: &kaveon_catalog::product_manifest::CatalogSnapshot,
    table: &str,
) -> Option<kaveon_optim::statistics::RelationStatistics> {
    let resolved = catalog
        .resolve_table(&kaveon_core::TableReference::parse(table))
        .ok()?;
    let qualified = format!(
        "{}.{}.{}",
        resolved.catalog, resolved.schema, resolved.table.name
    );
    let stored = durable.table_statistics.get(&qualified)?;
    let catalog_digest = format!("{:x}", Sha256::digest(catalog.snapshot_id.as_bytes()));
    if stored.catalog_snapshot_sha256 != catalog_digest {
        return None;
    }
    let current =
        kaveon_storage::analyze_source(&resolved.full_path(), resolved.table.format).ok()?;
    (current.identity_sha256 == stored.source_identity_sha256).then_some(
        kaveon_optim::statistics::RelationStatistics {
            rows: stored.row_count,
            columns: current.columns,
        },
    )
}

async fn commit_query_record(record: QueryRecord) -> bool {
    let mut store = QUERY_STORE.write().await;
    if store
        .queries
        .get(&record.id)
        .is_some_and(|existing| matches!(existing.state, QueryState::Canceled))
    {
        return false;
    }
    store.queries.insert(record.id.clone(), record);
    true
}

fn history_preview(rows: &[Vec<serde_json::Value>]) -> Vec<Vec<serde_json::Value>> {
    let mut bytes = 0;
    rows.iter()
        .take(100)
        .take_while(|row| {
            bytes += serde_json::to_vec(row).map_or(usize::MAX / 2, |encoded| encoded.len());
            bytes <= 64 * 1024
        })
        .cloned()
        .collect()
}

async fn prune_query_history() {
    let mut store = QUERY_STORE.write().await;
    let mut terminal: Vec<_> = store
        .queries
        .values()
        .filter(|record| !matches!(record.state, QueryState::Running))
        .map(|record| (record.submitted_at_ms, record.id.clone()))
        .collect();
    terminal.sort_unstable();
    let remove = terminal.len().saturating_sub(QUERY_HISTORY_LIMIT - 1);
    for (_, id) in terminal.into_iter().take(remove) {
        store.queries.remove(&id);
    }
}

fn spool_rows(
    state: &AppState,
    id: &str,
    principal: &str,
    rows: &mut Vec<Vec<serde_json::Value>>,
) -> std::io::Result<String> {
    if state.results.contains(id) {
        return Ok(format!("/v1/query/{id}/results/0"));
    }
    let mut writer = state.results.writer()?;
    for row in rows.drain(..) {
        writer.push(row)?;
    }
    state.results.publish(id, principal, writer)?;
    Ok(format!("/v1/query/{id}/results/0"))
}

fn spool_operator(
    operator: &mut dyn kaveon_core::BatchOperator,
    writer: &mut crate::results::ResultWriter,
) -> kaveon_core::Result<Vec<arrow::record_batch::RecordBatch>> {
    while let Some(batch) = operator.next_batch()? {
        for row in batches_to_json(&[batch]) {
            writer
                .push(row)
                .map_err(|error| kaveon_core::KaveonError::Execution(error.to_string()))?;
        }
    }
    Ok(Vec::new())
}

fn collect_inline_bounded(
    operator: &mut dyn kaveon_core::BatchOperator,
) -> kaveon_core::Result<Vec<arrow::record_batch::RecordBatch>> {
    let mut batches = Vec::new();
    let mut bytes = 0usize;
    while let Some(batch) = operator.next_batch()? {
        bytes = bytes.saturating_add(batch.get_array_memory_size());
        if bytes > 16 * 1024 * 1024 {
            return Err(kaveon_core::KaveonError::Execution(
                "inline results exceed 16 MiB; request result_delivery=paged".into(),
            ));
        }
        batches.push(batch);
    }
    Ok(batches)
}

async fn get_result_page(
    State(state): State<Arc<AppState>>,
    Path((id, page)): Path<(String, usize)>,
    Extension(identity): Extension<Identity>,
) -> Response {
    match state.results.page(&id, page, &identity) {
        Ok(value) => Json(value).into_response(),
        Err(status) => status.into_response(),
    }
}

async fn list_queries(Extension(identity): Extension<Identity>) -> Json<Vec<QueryRecord>> {
    let store = QUERY_STORE.read().await;
    let mut queries: Vec<QueryRecord> = store
        .queries
        .values()
        .filter(|record| identity.can_view(record.context.principal.as_deref()))
        .cloned()
        .collect();
    queries.sort_unstable_by_key(|query| Reverse(query.submitted_at_ms));
    queries.truncate(QUERY_HISTORY_LIMIT);
    Json(queries)
}

async fn get_query(
    Path(query_id): Path<String>,
    Extension(identity): Extension<Identity>,
) -> impl IntoResponse {
    let store = QUERY_STORE.read().await;
    match store.queries.get(&query_id) {
        Some(record) if identity.can_view(record.context.principal.as_deref()) => {
            Json(record.clone()).into_response()
        }
        _ => (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({
                "error": format!("query '{query_id}' not found"),
                "code": "QUERY_NOT_FOUND"
            })),
        )
            .into_response(),
    }
}

async fn cancel_query(
    State(state): State<Arc<AppState>>,
    Extension(identity): Extension<Identity>,
    Path(query_id): Path<String>,
) -> impl IntoResponse {
    if !state.config.coordinator {
        if state.lifecycle.cancellations.token(&query_id).is_err()
            || state.lifecycle.cancellations.cancel(&query_id).is_err()
        {
            return lifecycle_error_response("cannot register worker cancellation".into());
        }
        return StatusCode::NO_CONTENT.into_response();
    }
    let mut store = QUERY_STORE.write().await;
    let Some(record) = store.queries.get_mut(&query_id) else {
        return (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({
                "error": format!("query '{query_id}' not found"),
                "code": "QUERY_NOT_FOUND"
            })),
        )
            .into_response();
    };
    if !identity.can_view(record.context.principal.as_deref()) {
        return StatusCode::NOT_FOUND.into_response();
    }
    let was_running = matches!(record.state, QueryState::Running);
    if was_running {
        record.state = QueryState::Canceled;
        record.error = Some("query canceled by client".into());
        record.completed_at_ms = unix_time_ms();
        let _ = state.lifecycle.cancellations.cancel(&query_id);
    }
    drop(store);
    state.results.remove(&query_id);

    if let Some(store) = &state.disk_exchange_store {
        store.finish_query(&query_id);
    }
    if !was_running {
        return StatusCode::NO_CONTENT.into_response();
    }
    let workers = {
        let mut cluster = state.cluster.write().await;
        cluster.remove_stale_workers();
        cluster.workers.values().cloned().collect::<Vec<_>>()
    };
    let client = state.internal_http_client.clone();
    for worker in workers {
        let url = format!(
            "{}/v1/query/{query_id}",
            worker.address.trim_end_matches('/')
        );
        let mut request = client.delete(url);
        if let Some(token) = &state.config.exchange_token {
            request = request.bearer_auth(token);
        }
        let _ = request.send().await;
    }
    StatusCode::NO_CONTENT.into_response()
}

// --- Cluster / Node ---

#[derive(Serialize)]
struct ClusterResponse {
    environment: String,
    required_catalog_snapshot_id: String,
    coordinator: NodeInfo,
    workers: Vec<NodeInfo>,
    active_workers: usize,
    compatible_workers: usize,
    total_nodes: usize,
}

async fn get_cluster(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let required_catalog_snapshot_id = state.catalog.read().await.snapshot_id.clone();
    let mut cluster = state.cluster.write().await;
    cluster.this_node.catalog_snapshot_id = Some(required_catalog_snapshot_id.clone());
    let nodes = cluster.all_nodes();

    let coordinator = nodes
        .iter()
        .find(|n| n.role == NodeRole::Coordinator)
        .cloned()
        .unwrap_or_else(|| cluster.this_node.clone());

    let workers: Vec<NodeInfo> = nodes
        .iter()
        .filter(|n| n.role == NodeRole::Worker)
        .cloned()
        .collect();

    Json(ClusterResponse {
        environment: state.config.environment.clone(),
        required_catalog_snapshot_id: required_catalog_snapshot_id.clone(),
        coordinator,
        workers: workers.clone(),
        active_workers: workers.len(),
        compatible_workers: workers
            .iter()
            .filter(|worker| {
                worker.catalog_snapshot_id.as_deref() == Some(required_catalog_snapshot_id.as_str())
            })
            .count(),
        total_nodes: nodes.len(),
    })
}

async fn get_node(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let snapshot_id = state.catalog.read().await.snapshot_id.clone();
    let mut cluster = state.cluster.write().await;
    cluster.update_uptime();
    cluster.this_node.catalog_snapshot_id = Some(snapshot_id);
    Json(cluster.this_node.clone())
}

async fn receive_heartbeat(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(info): Json<NodeInfo>,
) -> Response {
    if !state.config.coordinator {
        return StatusCode::BAD_REQUEST.into_response();
    }
    if crate::exchange::validate_bearer_header(
        headers
            .get(header::AUTHORIZATION)
            .and_then(|v| v.to_str().ok()),
        state.config.exchange_token.as_deref().unwrap_or_default(),
    )
    .is_err()
    {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let required = state.catalog.read().await.snapshot_id.clone();
    let mut cluster = state.cluster.write().await;
    cluster.register_worker(info);
    Json(serde_json::json!({"required_catalog_snapshot_id": required})).into_response()
}

const MAX_CATALOG_REPLICA_BYTES: usize = 16 * 1024 * 1024;

async fn catalog_replica_snapshot(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Response {
    if !state.config.coordinator {
        return StatusCode::BAD_REQUEST.into_response();
    }
    if crate::exchange::validate_bearer_header(
        headers
            .get(header::AUTHORIZATION)
            .and_then(|v| v.to_str().ok()),
        state.config.exchange_token.as_deref().unwrap_or_default(),
    )
    .is_err()
    {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let snapshot = match state.catalog_store.export_replica_snapshot() {
        Ok(snapshot) => snapshot,
        Err(error) => {
            return task_failure_response(StatusCode::INTERNAL_SERVER_ERROR, &error.to_string());
        }
    };
    let bytes = match serde_json::to_vec(&snapshot) {
        Ok(bytes) if bytes.len() <= MAX_CATALOG_REPLICA_BYTES => bytes,
        Ok(_) => {
            return task_failure_response(
                StatusCode::PAYLOAD_TOO_LARGE,
                "catalog replica snapshot exceeds 16 MiB",
            );
        }
        Err(error) => {
            return task_failure_response(StatusCode::INTERNAL_SERVER_ERROR, &error.to_string());
        }
    };
    ([(header::CONTENT_TYPE, "application/json")], bytes).into_response()
}

// --- Catalog ---

async fn list_catalogs(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let catalog = state.catalog.read().await;
    let names = catalog.catalog_names();
    Json(serde_json::json!({ "catalogs": names }))
}

const ACTOR_HEADER: &str = "x-kaveon-actor";

fn mutation_actor<'a>(state: &AppState, headers: &'a HeaderMap) -> Result<&'a str, Box<Response>> {
    if !state.config.coordinator {
        return Err(Box::new(
            (
                StatusCode::FORBIDDEN,
                Json(serde_json::json!({
                    "error": "catalog mutations are accepted only by the coordinator"
                })),
            )
                .into_response(),
        ));
    }
    let Some(expected_token) = state
        .config
        .catalog_admin_token
        .as_deref()
        .filter(|token| !token.is_empty())
    else {
        return Err(Box::new(
            (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(serde_json::json!({
                    "error": "catalog mutations are disabled because no admin token is configured"
                })),
            )
                .into_response(),
        ));
    };
    let supplied_token = headers
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "));
    if supplied_token != Some(expected_token) {
        return Err(Box::new(
            (
                StatusCode::UNAUTHORIZED,
                Json(serde_json::json!({ "error": "invalid catalog authorization" })),
            )
                .into_response(),
        ));
    }
    headers
        .get(ACTOR_HEADER)
        .and_then(|value| value.to_str().ok())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            Box::new(
                (
                    StatusCode::BAD_REQUEST,
                    Json(serde_json::json!({
                        "error": format!("missing non-empty {ACTOR_HEADER} header")
                    })),
                )
                    .into_response(),
            )
        })
}

fn expected_revision(headers: &HeaderMap) -> Result<CatalogRevision, Box<Response>> {
    let raw = headers
        .get(header::IF_MATCH)
        .and_then(|value| value.to_str().ok())
        .map(|value| value.trim().trim_matches('"'))
        .ok_or_else(|| {
            Box::new(
                (
                    StatusCode::PRECONDITION_REQUIRED,
                    Json(serde_json::json!({ "error": "missing If-Match revision header" })),
                )
                    .into_response(),
            )
        })?;
    let value =
        raw.parse::<u64>().map_err(|_| {
            Box::new((
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": "If-Match must be a positive revision number" })),
        )
            .into_response())
        })?;
    CatalogRevision::new(value).map_err(|error| Box::new(catalog_error_response(error)))
}

fn catalog_error_response(error: kaveon_core::KaveonError) -> Response {
    let message = error.to_string();
    let status = if message.contains("not found") {
        StatusCode::NOT_FOUND
    } else if message.contains("revision")
        || message.contains("already exists")
        || message.contains("contains")
    {
        StatusCode::CONFLICT
    } else {
        StatusCode::BAD_REQUEST
    };
    (status, Json(serde_json::json!({ "error": message }))).into_response()
}

fn validate_new_catalog(value: &CatalogDefinition) -> kaveon_core::Result<()> {
    CatalogId::new(value.id().as_str())?;
    let mut validated = CatalogDefinition::new(
        value.id().clone(),
        value.name(),
        value.adapter(),
        value.storage().clone(),
    )?;
    if let Some(credential) = value.credential() {
        let credential =
            kaveon_core::CredentialReference::new(credential.kind(), credential.reference())?;
        validated = validated.with_credential(credential);
    }
    if value.lifecycle() != CatalogLifecycle::Draft
        || value.revision() != CatalogRevision::initial()
        || &validated != value
    {
        return Err(kaveon_core::KaveonError::Execution(
            "new catalog definitions must be valid draft revision 1 values".into(),
        ));
    }
    Ok(())
}

fn validate_catalog_fields(value: &CatalogDefinition) -> kaveon_core::Result<()> {
    CatalogId::new(value.id().as_str())?;
    CatalogDefinition::new(
        value.id().clone(),
        value.name(),
        value.adapter(),
        value.storage().clone(),
    )?;
    if let Some(credential) = value.credential() {
        kaveon_core::CredentialReference::new(credential.kind(), credential.reference())?;
    }
    Ok(())
}

fn validate_new_schema(value: &SchemaDefinition) -> kaveon_core::Result<()> {
    SchemaId::new(value.id().as_str())?;
    CatalogId::new(value.catalog_id().as_str())?;
    let validated =
        SchemaDefinition::new(value.id().clone(), value.catalog_id().clone(), value.name())?;
    if value.lifecycle() != CatalogLifecycle::Draft
        || value.revision() != CatalogRevision::initial()
        || &validated != value
    {
        return Err(kaveon_core::KaveonError::Execution(
            "new schema definitions must be valid draft revision 1 values".into(),
        ));
    }
    Ok(())
}

fn validate_schema_fields(value: &SchemaDefinition) -> kaveon_core::Result<()> {
    SchemaId::new(value.id().as_str())?;
    CatalogId::new(value.catalog_id().as_str())?;
    SchemaDefinition::new(value.id().clone(), value.catalog_id().clone(), value.name())?;
    Ok(())
}

fn validate_new_table(value: &TableDefinition) -> kaveon_core::Result<()> {
    TableId::new(value.id().as_str())?;
    SchemaId::new(value.schema_id().as_str())?;
    let columns = value
        .columns()
        .iter()
        .map(|column| {
            ColumnDefinition::new(column.name(), column.data_type().clone(), column.nullable())
        })
        .collect::<kaveon_core::Result<Vec<_>>>()?;
    let validated = TableDefinition::new(
        value.id().clone(),
        value.schema_id().clone(),
        value.name(),
        value.location(),
        value.access(),
        value.format(),
        columns,
    )?;
    if value.lifecycle() != CatalogLifecycle::Draft
        || value.revision() != CatalogRevision::initial()
        || &validated != value
    {
        return Err(kaveon_core::KaveonError::Execution(
            "new table definitions must be valid draft revision 1 values".into(),
        ));
    }
    Ok(())
}

fn validate_table_fields(value: &TableDefinition) -> kaveon_core::Result<()> {
    TableId::new(value.id().as_str())?;
    SchemaId::new(value.schema_id().as_str())?;
    let columns = value
        .columns()
        .iter()
        .map(|column| {
            ColumnDefinition::new(column.name(), column.data_type().clone(), column.nullable())
        })
        .collect::<kaveon_core::Result<Vec<_>>>()?;
    TableDefinition::new(
        value.id().clone(),
        value.schema_id().clone(),
        value.name(),
        value.location(),
        value.access(),
        value.format(),
        columns,
    )?;
    Ok(())
}

fn validate_replacement(
    current_revision: CatalogRevision,
    current_lifecycle: CatalogLifecycle,
    new_revision: CatalogRevision,
    new_lifecycle: CatalogLifecycle,
) -> kaveon_core::Result<()> {
    if new_revision != current_revision.next()? {
        return Err(kaveon_core::KaveonError::Execution(format!(
            "replacement revision must be {}",
            current_revision.next()?.value()
        )));
    }
    if current_lifecycle == new_lifecycle {
        Ok(())
    } else {
        current_lifecycle.validate_transition(new_lifecycle)
    }
}

pub(crate) async fn refresh_catalog_snapshot(state: &AppState) -> Result<(), Box<Response>> {
    let snapshot =
        crate::config::catalog_manager_snapshot(&state.catalog_store).map_err(|error| {
            Box::new(
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(
                        serde_json::json!({ "error": format!("catalog snapshot failed: {error}") }),
                    ),
                )
                    .into_response(),
            )
        })?;
    let snapshot_id = state.catalog_store.snapshot_identity().map_err(|error| {
        Box::new(
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({
                    "error": format!("catalog identity failed: {error}")
                })),
            )
                .into_response(),
        )
    })?;
    *state.catalog.write().await = Arc::new(crate::PublishedCatalog {
        manager: snapshot,
        snapshot_id,
    });
    Ok(())
}

async fn list_catalog_definitions(State(state): State<Arc<AppState>>) -> Response {
    match state.catalog_store.list_catalogs() {
        Ok(values) => Json(values).into_response(),
        Err(error) => catalog_error_response(error),
    }
}

async fn get_catalog_definition(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Response {
    let id = match CatalogId::new(id) {
        Ok(id) => id,
        Err(error) => return catalog_error_response(error),
    };
    match state.catalog_store.catalog(&id) {
        Ok(Some(value)) => Json(value).into_response(),
        Ok(None) => StatusCode::NOT_FOUND.into_response(),
        Err(error) => catalog_error_response(error),
    }
}

async fn create_catalog_definition(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(value): Json<CatalogDefinition>,
) -> Response {
    let actor = match mutation_actor(&state, &headers) {
        Ok(actor) => actor,
        Err(response) => return *response,
    };
    if let Err(error) = validate_new_catalog(&value) {
        return catalog_error_response(error);
    }
    if let Err(error) = state.catalog_store.create_catalog(actor, &value) {
        return catalog_error_response(error);
    }
    if let Err(response) = refresh_catalog_snapshot(&state).await {
        return *response;
    }
    (StatusCode::CREATED, Json(value)).into_response()
}

async fn replace_catalog_definition(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    headers: HeaderMap,
    Json(value): Json<CatalogDefinition>,
) -> Response {
    let actor = match mutation_actor(&state, &headers) {
        Ok(actor) => actor,
        Err(response) => return *response,
    };
    let expected = match expected_revision(&headers) {
        Ok(revision) => revision,
        Err(response) => return *response,
    };
    if value.id().as_str() != id {
        return catalog_error_response(kaveon_core::KaveonError::Execution(
            "catalog path ID does not match request body".into(),
        ));
    }
    if let Err(error) = validate_catalog_fields(&value) {
        return catalog_error_response(error);
    }
    let current = match state.catalog_store.catalog(value.id()) {
        Ok(Some(current)) => current,
        Ok(None) => return StatusCode::NOT_FOUND.into_response(),
        Err(error) => return catalog_error_response(error),
    };
    if let Err(error) = validate_replacement(
        current.revision(),
        current.lifecycle(),
        value.revision(),
        value.lifecycle(),
    ) {
        return catalog_error_response(error);
    }
    if let Err(error) = state.catalog_store.replace_catalog(actor, expected, &value) {
        return catalog_error_response(error);
    }
    if let Err(response) = refresh_catalog_snapshot(&state).await {
        return *response;
    }
    Json(value).into_response()
}

#[derive(Deserialize)]
struct DeleteCatalogQuery {
    #[serde(default)]
    cascade: bool,
}

async fn delete_catalog_definition(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    axum::extract::Query(query): axum::extract::Query<DeleteCatalogQuery>,
    headers: HeaderMap,
) -> Response {
    let actor = match mutation_actor(&state, &headers) {
        Ok(actor) => actor,
        Err(response) => return *response,
    };
    let expected = match expected_revision(&headers) {
        Ok(revision) => revision,
        Err(response) => return *response,
    };
    let id = match CatalogId::new(id) {
        Ok(id) => id,
        Err(error) => return catalog_error_response(error),
    };
    let policy = if query.cascade {
        CascadePolicy::Cascade
    } else {
        CascadePolicy::Restrict
    };
    if let Err(error) = state
        .catalog_store
        .delete_catalog(actor, &id, expected, policy)
    {
        return catalog_error_response(error);
    }
    if let Err(response) = refresh_catalog_snapshot(&state).await {
        return *response;
    }
    StatusCode::NO_CONTENT.into_response()
}

async fn list_schema_definitions(
    State(state): State<Arc<AppState>>,
    Path(catalog_id): Path<String>,
) -> Response {
    let id = match CatalogId::new(catalog_id) {
        Ok(id) => id,
        Err(error) => return catalog_error_response(error),
    };
    match state.catalog_store.list_schemas(&id) {
        Ok(values) => Json(values).into_response(),
        Err(error) => catalog_error_response(error),
    }
}

async fn get_schema_definition(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Response {
    let id = match SchemaId::new(id) {
        Ok(id) => id,
        Err(error) => return catalog_error_response(error),
    };
    match state.catalog_store.schema(&id) {
        Ok(Some(value)) => Json(value).into_response(),
        Ok(None) => StatusCode::NOT_FOUND.into_response(),
        Err(error) => catalog_error_response(error),
    }
}

async fn create_schema_definition(
    State(state): State<Arc<AppState>>,
    Path(catalog_id): Path<String>,
    headers: HeaderMap,
    Json(value): Json<SchemaDefinition>,
) -> Response {
    let actor = match mutation_actor(&state, &headers) {
        Ok(actor) => actor,
        Err(response) => return *response,
    };
    if value.catalog_id().as_str() != catalog_id {
        return catalog_error_response(kaveon_core::KaveonError::Execution(
            "catalog path ID does not match schema parent ID".into(),
        ));
    }
    if let Err(error) = validate_new_schema(&value) {
        return catalog_error_response(error);
    }
    if let Err(error) = state.catalog_store.create_schema(actor, &value) {
        return catalog_error_response(error);
    }
    if let Err(response) = refresh_catalog_snapshot(&state).await {
        return *response;
    }
    (StatusCode::CREATED, Json(value)).into_response()
}

async fn replace_schema_definition(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    headers: HeaderMap,
    Json(value): Json<SchemaDefinition>,
) -> Response {
    let actor = match mutation_actor(&state, &headers) {
        Ok(actor) => actor,
        Err(response) => return *response,
    };
    let expected = match expected_revision(&headers) {
        Ok(revision) => revision,
        Err(response) => return *response,
    };
    if value.id().as_str() != id {
        return catalog_error_response(kaveon_core::KaveonError::Execution(
            "schema path ID does not match request body".into(),
        ));
    }
    if let Err(error) = validate_schema_fields(&value) {
        return catalog_error_response(error);
    }
    let current = match state.catalog_store.schema(value.id()) {
        Ok(Some(current)) => current,
        Ok(None) => return StatusCode::NOT_FOUND.into_response(),
        Err(error) => return catalog_error_response(error),
    };
    if current.catalog_id() != value.catalog_id() {
        return catalog_error_response(kaveon_core::KaveonError::Execution(
            "schema catalog cannot change during replacement".into(),
        ));
    }
    if let Err(error) = validate_replacement(
        current.revision(),
        current.lifecycle(),
        value.revision(),
        value.lifecycle(),
    ) {
        return catalog_error_response(error);
    }
    if let Err(error) = state.catalog_store.replace_schema(actor, expected, &value) {
        return catalog_error_response(error);
    }
    if let Err(response) = refresh_catalog_snapshot(&state).await {
        return *response;
    }
    Json(value).into_response()
}

async fn delete_schema_definition(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    axum::extract::Query(query): axum::extract::Query<DeleteCatalogQuery>,
    headers: HeaderMap,
) -> Response {
    let actor = match mutation_actor(&state, &headers) {
        Ok(actor) => actor,
        Err(response) => return *response,
    };
    let expected = match expected_revision(&headers) {
        Ok(revision) => revision,
        Err(response) => return *response,
    };
    let id = match SchemaId::new(id) {
        Ok(id) => id,
        Err(error) => return catalog_error_response(error),
    };
    let policy = if query.cascade {
        CascadePolicy::Cascade
    } else {
        CascadePolicy::Restrict
    };
    if let Err(error) = state
        .catalog_store
        .delete_schema(actor, &id, expected, policy)
    {
        return catalog_error_response(error);
    }
    if let Err(response) = refresh_catalog_snapshot(&state).await {
        return *response;
    }
    StatusCode::NO_CONTENT.into_response()
}

async fn list_table_definitions(
    State(state): State<Arc<AppState>>,
    Path(schema_id): Path<String>,
) -> Response {
    let id = match SchemaId::new(schema_id) {
        Ok(id) => id,
        Err(error) => return catalog_error_response(error),
    };
    match state.catalog_store.list_tables(&id) {
        Ok(values) => Json(values).into_response(),
        Err(error) => catalog_error_response(error),
    }
}

async fn get_table_definition(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Response {
    let id = match TableId::new(id) {
        Ok(id) => id,
        Err(error) => return catalog_error_response(error),
    };
    match state.catalog_store.table(&id) {
        Ok(Some(value)) => Json(value).into_response(),
        Ok(None) => StatusCode::NOT_FOUND.into_response(),
        Err(error) => catalog_error_response(error),
    }
}

async fn create_table_definition(
    State(state): State<Arc<AppState>>,
    Path(schema_id): Path<String>,
    headers: HeaderMap,
    Json(value): Json<TableDefinition>,
) -> Response {
    let actor = match mutation_actor(&state, &headers) {
        Ok(actor) => actor,
        Err(response) => return *response,
    };
    if value.schema_id().as_str() != schema_id {
        return catalog_error_response(kaveon_core::KaveonError::Execution(
            "schema path ID does not match table parent ID".into(),
        ));
    }
    if let Err(error) = validate_new_table(&value) {
        return catalog_error_response(error);
    }
    if let Err(error) = state.catalog_store.create_table(actor, &value) {
        return catalog_error_response(error);
    }
    if let Err(response) = refresh_catalog_snapshot(&state).await {
        return *response;
    }
    (StatusCode::CREATED, Json(value)).into_response()
}

async fn replace_table_definition(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    headers: HeaderMap,
    Json(value): Json<TableDefinition>,
) -> Response {
    let actor = match mutation_actor(&state, &headers) {
        Ok(actor) => actor,
        Err(response) => return *response,
    };
    let expected = match expected_revision(&headers) {
        Ok(revision) => revision,
        Err(response) => return *response,
    };
    if value.id().as_str() != id {
        return catalog_error_response(kaveon_core::KaveonError::Execution(
            "table path ID does not match request body".into(),
        ));
    }
    if let Err(error) = validate_table_fields(&value) {
        return catalog_error_response(error);
    }
    let current = match state.catalog_store.table(value.id()) {
        Ok(Some(current)) => current,
        Ok(None) => return StatusCode::NOT_FOUND.into_response(),
        Err(error) => return catalog_error_response(error),
    };
    if current.schema_id() != value.schema_id() {
        return catalog_error_response(kaveon_core::KaveonError::Execution(
            "table schema cannot change during replacement".into(),
        ));
    }
    if let Err(error) = validate_replacement(
        current.revision(),
        current.lifecycle(),
        value.revision(),
        value.lifecycle(),
    ) {
        return catalog_error_response(error);
    }
    if let Err(error) = state.catalog_store.replace_table(actor, expected, &value) {
        return catalog_error_response(error);
    }
    if let Err(response) = refresh_catalog_snapshot(&state).await {
        return *response;
    }
    Json(value).into_response()
}

async fn delete_table_definition(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Response {
    let actor = match mutation_actor(&state, &headers) {
        Ok(actor) => actor,
        Err(response) => return *response,
    };
    let expected = match expected_revision(&headers) {
        Ok(revision) => revision,
        Err(response) => return *response,
    };
    let id = match TableId::new(id) {
        Ok(id) => id,
        Err(error) => return catalog_error_response(error),
    };
    if let Err(error) = state.catalog_store.delete_table(actor, &id, expected) {
        return catalog_error_response(error);
    }
    if let Err(response) = refresh_catalog_snapshot(&state).await {
        return *response;
    }
    StatusCode::NO_CONTENT.into_response()
}

async fn list_schemas(
    State(state): State<Arc<AppState>>,
    Path(catalog_name): Path<String>,
) -> impl IntoResponse {
    let catalog = state.catalog.read().await;
    match catalog.catalog(&catalog_name) {
        Some(cat) => {
            let names = cat.schema_names();
            Json(serde_json::json!({ "schemas": names })).into_response()
        }
        None => (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({
                "error": format!("catalog '{catalog_name}' not found"),
                "code": "CATALOG_NOT_FOUND"
            })),
        )
            .into_response(),
    }
}

async fn list_tables(
    State(state): State<Arc<AppState>>,
    Path((catalog_name, schema_name)): Path<(String, String)>,
) -> impl IntoResponse {
    let catalog = state.catalog.read().await;
    match catalog.catalog(&catalog_name) {
        Some(cat) => match cat.table_names(&schema_name) {
            Ok(names) => Json(serde_json::json!({ "tables": names })).into_response(),
            Err(e) => (
                StatusCode::NOT_FOUND,
                Json(serde_json::json!({
                    "error": format!("{e}"),
                    "code": "SCHEMA_NOT_FOUND"
                })),
            )
                .into_response(),
        },
        None => (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({
                "error": format!("catalog '{catalog_name}' not found"),
                "code": "CATALOG_NOT_FOUND"
            })),
        )
            .into_response(),
    }
}

// --- Health ---

async fn health() -> impl IntoResponse {
    Json(serde_json::json!({
        "status": "ok",
        "version": env!("CARGO_PKG_VERSION")
    }))
}

async fn ready(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let catalog = state.catalog.read().await;
    let has_catalogs = !catalog.catalog_names().is_empty();
    let snapshot_id = catalog.snapshot_id.clone();
    drop(catalog);
    let worker_synced = if state.config.coordinator {
        true
    } else {
        state
            .cluster
            .read()
            .await
            .required_catalog_snapshot_id
            .as_deref()
            == Some(snapshot_id.as_str())
    };
    if has_catalogs && worker_synced {
        (
            StatusCode::OK,
            Json(serde_json::json!({
                "ready": true,
                "catalog_snapshot_id": snapshot_id
            })),
        )
            .into_response()
    } else {
        (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(serde_json::json!({
                "ready": false,
                "reason": if has_catalogs { "catalog synchronization pending" } else { "no catalogs loaded" },
                "catalog_snapshot_id": snapshot_id
            })),
        )
            .into_response()
    }
}

// --- Helpers ---

fn encode_arrow_stream(
    schema: &arrow::datatypes::SchemaRef,
    batches: &[arrow::record_batch::RecordBatch],
) -> Result<Vec<u8>, String> {
    let mut bytes =
        crate::transport::BoundedBuffer::new(crate::transport::MAX_PAYLOAD_BYTES as usize);
    {
        // Exchange and result payloads are commonly dominated by repeated
        // integer and string values. LZ4 keeps decoding inexpensive while
        // reducing network transfer and the private receive spool. Arrow IPC
        // readers negotiate the codec from each message, so this remains wire
        // compatible with existing clients.
        let options = arrow::ipc::writer::IpcWriteOptions::default()
            .try_with_compression(Some(arrow::ipc::CompressionType::LZ4_FRAME))
            .map_err(|error| format!("cannot configure Arrow stream: {error}"))?;
        let mut writer = arrow::ipc::writer::StreamWriter::try_new_with_options(
            &mut bytes, schema, options,
        )
        .map_err(|error| format!("cannot create Arrow stream: {error}"))?;
        for batch in batches {
            writer
                .write(batch)
                .map_err(|error| format!("cannot encode Arrow batch: {error}"))?;
        }
        writer
            .finish()
            .map_err(|error| format!("cannot finish Arrow stream: {error}"))?;
    }
    Ok(bytes.into_bytes())
}

#[cfg(test)]
fn decode_arrow_stream(
    bytes: &[u8],
) -> Result<
    (
        arrow::datatypes::SchemaRef,
        Vec<arrow::record_batch::RecordBatch>,
    ),
    String,
> {
    let reader = arrow::ipc::reader::StreamReader::try_new(Cursor::new(bytes), None)
        .map_err(|error| error.to_string())?;
    let schema = reader.schema();
    let batches = reader
        .map(|batch| batch.map_err(|error| error.to_string()))
        .collect::<Result<Vec<_>, _>>()?;
    Ok((schema, batches))
}

fn columns_from_schema(schema: &arrow::datatypes::SchemaRef) -> Vec<ColumnInfo> {
    schema
        .fields()
        .iter()
        .map(|field| ColumnInfo {
            name: field.name().clone(),
            data_type: field.data_type().to_string(),
        })
        .collect()
}

#[derive(Clone, Copy)]
enum MergeOperation {
    Add,
    Min,
    Max,
}

const REMOTE_TASK_TIMEOUT: Duration = Duration::from_secs(120);

struct RemoteTaskFailure {
    message: String,
    retryable: bool,
}

async fn execute_remote_task(
    client: &reqwest::Client,
    worker: &NodeInfo,
    request: &TaskRequest,
    exchange_token: Option<&str>,
) -> Result<
    (
        arrow::datatypes::SchemaRef,
        Vec<arrow::record_batch::RecordBatch>,
        u64,
        usize,
        Option<TaskScanMetrics>,
        Option<TaskExecutionMetrics>,
    ),
    RemoteTaskFailure,
> {
    let (payload, elapsed_us, scan, execution) =
        execute_remote_task_payload(client, worker, request, exchange_token).await?;
    let output_bytes = payload.bytes();
    let (schema, batches) = payload.collect().map_err(|message| RemoteTaskFailure {
        message,
        retryable: false,
    })?;
    Ok((schema, batches, elapsed_us, output_bytes, scan, execution))
}

async fn execute_remote_task_payload(
    client: &reqwest::Client,
    worker: &NodeInfo,
    request: &TaskRequest,
    exchange_token: Option<&str>,
) -> Result<
    (
        crate::transport::ArrowPayload,
        u64,
        Option<TaskScanMetrics>,
        Option<TaskExecutionMetrics>,
    ),
    RemoteTaskFailure,
> {
    let url = format!("{}/v1/task", worker.address.trim_end_matches('/'));
    let mut submission = client.post(url).json(request);
    if let Some(token) = exchange_token {
        submission = submission.bearer_auth(token);
    }
    let response = submission
        .timeout(REMOTE_TASK_TIMEOUT)
        .send()
        .await
        .map_err(|error| RemoteTaskFailure {
            message: format!("worker '{}' is unavailable: {error}", worker.node_id),
            retryable: true,
        })?;
    if !response.status().is_success() {
        let status = response.status();
        let retryable = status.is_server_error()
            || status == StatusCode::REQUEST_TIMEOUT
            || status == StatusCode::TOO_MANY_REQUESTS;
        let mut response = response;
        let message = response
            .chunk()
            .await
            .ok()
            .flatten()
            .map(|chunk| String::from_utf8_lossy(&chunk[..chunk.len().min(8192)]).into_owned())
            .unwrap_or_default();
        return Err(RemoteTaskFailure {
            message: format!(
                "worker '{}' failed task with {status}: {message}",
                worker.node_id
            ),
            retryable,
        });
    }
    let elapsed_us = response
        .headers()
        .get("x-kaveon-task-elapsed-us")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse().ok())
        .unwrap_or_default();
    // Absence is valid for an older worker or a fragment that has no reader telemetry.
    let scan = response
        .headers()
        .get("x-kaveon-task-scan-metrics")
        .and_then(|value| value.to_str().ok())
        .filter(|value| !value.is_empty())
        .and_then(|value| serde_json::from_str(value).ok());
    let execution = response
        .headers()
        .get("x-kaveon-task-execution-metrics")
        .and_then(|value| value.to_str().ok())
        .filter(|value| !value.is_empty())
        .and_then(|value| serde_json::from_str(value).ok());
    let payload = crate::transport::receive(response)
        .await
        .map_err(|message| RemoteTaskFailure {
            retryable: message.starts_with("network receive:"),
            message,
        })?;
    Ok((payload, elapsed_us, scan, execution))
}

async fn cleanup_distributed_query(state: &Arc<AppState>, query_id: &str) {
    let workers = {
        let mut cluster = state.cluster.write().await;
        cluster.remove_stale_workers();
        cluster.workers.values().cloned().collect::<Vec<_>>()
    };
    if let Some(token) = state.config.exchange_token.as_deref() {
        let client = state.internal_http_client.clone();
        let mut cleanups = tokio::task::JoinSet::new();
        for worker in workers {
            let client = client.clone();
            let token = token.to_owned();
            let query_id = query_id.to_owned();
            cleanups.spawn(async move {
                let url = format!(
                    "{}/v1/internal/query/{query_id}/finish",
                    worker.address.trim_end_matches('/')
                );
                let _ = client.post(url).bearer_auth(token).send().await;
            });
        }
        while cleanups.join_next().await.is_some() {}
    }
    if let Some(store) = &state.disk_exchange_store {
        store.finish_query(query_id);
    }
    let _ = state.lifecycle.finish_query(query_id);
}

fn workers_for_catalog_snapshot(
    cluster: &mut crate::cluster::ClusterState,
    required_snapshot_id: &str,
) -> Result<Vec<NodeInfo>, String> {
    cluster.remove_stale_workers();
    let active = cluster.workers.len();
    let compatible = cluster.compatible_workers(required_snapshot_id);
    if active > 0 && compatible.is_empty() {
        return Err(format!(
            "NO_COMPATIBLE_WORKER: no active worker has catalog snapshot {required_snapshot_id}"
        ));
    }
    if active >= 2 && compatible.len() < 2 {
        return Err(format!(
            "INSUFFICIENT_COMPATIBLE_WORKERS: catalog snapshot {required_snapshot_id} is present on {} of {active} active workers",
            compatible.len()
        ));
    }
    Ok(compatible)
}

async fn execute_distributed_fragments(
    state: &Arc<AppState>,
    query_id: &str,
    context: &QueryContext,
    plan: &LogicalPlan,
    catalog_snapshot: &kaveon_core::CatalogManager,
) -> Option<Result<(TaskResponse, Vec<StageTelemetry>, u64), String>> {
    if exact_metadata_count_plan(plan) || !general_distributed_eligible(plan) {
        return None;
    }
    let worker_selection = {
        let mut cluster = state.cluster.write().await;
        workers_for_catalog_snapshot(&mut cluster, &context.catalog_snapshot_id)
    };
    let mut workers = match worker_selection {
        Ok(workers) => workers,
        Err(error) => return Some(Err(error)),
    };
    workers.sort_unstable_by(|left, right| left.node_id.cmp(&right.node_id));
    let token = state.config.exchange_token.clone()?;
    if workers.len() < 2 || token.is_empty() {
        return None;
    }

    let planning_start = Instant::now();
    let graph = crate::planner::build_stage_graph(query_id, plan, workers.len()).ok()?;
    let fragments =
        crate::planner::build_executable_fragments(query_id, plan, catalog_snapshot, workers.len())
            .ok()?;
    let planning_us = elapsed_us(planning_start);
    let mut orchestrator = match CoordinatorOrchestrator::new(graph, fragments, workers.clone()) {
        Ok(orchestrator) => orchestrator,
        Err(error) => return Some(Err(format!("cannot initialize stage execution: {error}"))),
    };
    if state.disk_exchange_store.is_some() {
        orchestrator.set_exchange_store_uri(state.cluster.read().await.this_node.address.clone());
    }
    let cancellation = match state.lifecycle.cancellations.token(query_id) {
        Ok(cancellation) => cancellation,
        Err(error) => return Some(Err(error.to_string())),
    };
    let client = state.internal_http_client.clone();
    let execution_start = Instant::now();
    let mut stage_started = BTreeMap::<StageId, Instant>::new();
    let mut stage_tasks = BTreeMap::<StageId, Vec<TaskTelemetry>>::new();
    let mut task_failures = Vec::new();
    // Exchange deletion is best-effort housekeeping. A consumed exchange cannot
    // be read by a later stage, so overlap its HTTP deletes with the next ready
    // stage instead of leaving every worker idle between stage waves. We still
    // join these jobs before returning to preserve the prior cleanup lifetime.
    let mut exchange_cleanups = tokio::task::JoinSet::new();
    let mut result_schema = None;
    let mut result_batches = Vec::new();
    let mut result_bytes = 0usize;
    let mut result_writer = if context.result_delivery.as_deref() == Some("paged") {
        match state.results.writer() {
            Ok(writer) => Some(writer),
            Err(error) => return Some(Err(error.to_string())),
        }
    } else {
        None
    };

    while !orchestrator.is_terminal() {
        if cancellation.is_cancelled() {
            orchestrator.cancel();
            return Some(Err("query canceled".into()));
        }
        let dispatches = match orchestrator.ready_dispatches() {
            Ok(dispatches) if !dispatches.is_empty() => dispatches,
            Ok(_) => return Some(Err("distributed stage graph made no progress".into())),
            Err(error) => return Some(Err(format!("cannot schedule ready tasks: {error}"))),
        };
        let mut tasks = tokio::task::JoinSet::new();
        for dispatch in dispatches {
            if let Err(error) = orchestrator.start_task(&dispatch.assignment.task_id) {
                return Some(Err(format!("cannot start distributed task: {error}")));
            }
            stage_started
                .entry(dispatch.assignment.task_id.stage_id)
                .or_insert_with(Instant::now);
            let Some(worker) = workers
                .iter()
                .find(|worker| worker.node_id == dispatch.assignment.worker_id)
                .cloned()
            else {
                return Some(Err("task references an unavailable worker".into()));
            };
            let request = task_request_from_dispatch(&dispatch, context);
            let client = client.clone();
            let token = token.clone();
            tasks.spawn(async move {
                let result =
                    execute_remote_task_payload(&client, &worker, &request, Some(&token)).await;
                (dispatch, worker, result)
            });
        }
        while let Some(joined) = tasks.join_next().await {
            let (dispatch, worker, result) = match joined {
                Ok(result) => result,
                Err(error) => {
                    orchestrator.cancel();
                    return Some(Err(format!("distributed task panicked: {error}")));
                }
            };
            let task_id = &dispatch.assignment.task_id;
            match result {
                Ok((mut payload, elapsed_us, scan, execution)) => {
                    let schema = payload.schema();
                    let output_bytes = payload.bytes();
                    let mut output_rows = 0;
                    let mut output_batches = 0;
                    let root = dispatch.exchange_outputs.is_empty();
                    if root {
                        if result_schema
                            .as_ref()
                            .is_some_and(|expected| expected != &schema)
                        {
                            orchestrator.cancel();
                            return Some(Err("root tasks returned incompatible schemas".into()));
                        }
                        result_schema.get_or_insert(schema);
                    }
                    loop {
                        let batch = match payload.next_batch() {
                            Ok(Some(batch)) => batch,
                            Ok(None) => break,
                            Err(error) => return Some(Err(error)),
                        };
                        output_rows += batch.num_rows();
                        output_batches += 1;
                        if root {
                            if let Some(writer) = result_writer.as_mut() {
                                for row in batches_to_json(&[batch]) {
                                    if let Err(error) = writer.push(row) {
                                        return Some(Err(error.to_string()));
                                    }
                                }
                            } else {
                                result_bytes =
                                    result_bytes.saturating_add(batch.get_array_memory_size());
                                if result_bytes > 16 * 1024 * 1024 {
                                    return Some(Err("inline results exceed 16 MiB; request result_delivery=paged".into()));
                                }
                                result_batches.push(batch);
                            }
                        }
                    }
                    stage_tasks
                        .entry(task_id.stage_id)
                        .or_default()
                        .push(TaskTelemetry {
                            task_id: task_id.to_string(),
                            node_id: worker.node_id,
                            partition_index: task_id.partition,
                            elapsed_us,
                            output_rows,
                            output_batches,
                            output_bytes,
                            execution,
                            scan,
                        });
                    if let Err(error) = orchestrator.finish_task(task_id) {
                        return Some(Err(format!("cannot finish distributed task: {error}")));
                    }
                }
                Err(failure) => {
                    eprintln!("distributed task {task_id} failed: {}", failure.message);
                    task_failures.push(failure.message.clone());
                    release_dispatch_outputs(&client, &token, &dispatch).await;
                    if !failure.retryable {
                        orchestrator.cancel();
                        return Some(Err(failure.message));
                    }
                    match orchestrator.fail_task(task_id, &failure.message) {
                        Ok(true) => {}
                        Ok(false) => return Some(Err(task_failures.join("; "))),
                        Err(error) => {
                            return Some(Err(format!(
                                "cannot record distributed task failure: {error}"
                            )));
                        }
                    }
                }
            }
        }
        schedule_completed_exchange_cleanup(
            &client,
            &token,
            &mut orchestrator,
            &mut exchange_cleanups,
        );
    }

    while let Some(cleanup) = exchange_cleanups.join_next().await {
        if cleanup.is_err() {
            // Deletion was already best-effort. Query finalization also removes
            // every worker-side artifact for the query.
        }
    }

    if !orchestrator.is_finished() {
        return Some(Err("distributed query terminated before completion".into()));
    }
    let Some(schema) = result_schema else {
        return Some(Err(
            "distributed query completed without a root result".into()
        ));
    };
    let execution_us = elapsed_us(execution_start);
    let mut stages = stage_tasks
        .into_iter()
        .map(|(stage_id, mut tasks)| {
            tasks.sort_unstable_by_key(|task| task.partition_index);
            StageTelemetry {
                stage_id: stage_id.0,
                state: "FINISHED",
                task_count: tasks.len(),
                completed_tasks: tasks.len(),
                elapsed_us: stage_started
                    .get(&stage_id)
                    .map_or(0, |started| elapsed_us(*started)),
                tasks,
            }
        })
        .collect::<Vec<_>>();
    stages.sort_unstable_by_key(|stage| stage.stage_id);
    let data = if let Some(writer) = result_writer {
        if let Err(error) = state.results.publish(
            query_id,
            context.principal.as_deref().unwrap_or("internal"),
            writer,
        ) {
            return Some(Err(error.to_string()));
        }
        Vec::new()
    } else {
        batches_to_json(&result_batches)
    };
    Some(Ok((
        TaskResponse {
            columns: columns_from_schema(&schema),
            data,
            elapsed_us: execution_us,
        },
        stages,
        planning_us,
    )))
}

async fn release_dispatch_outputs(client: &reqwest::Client, token: &str, dispatch: &TaskDispatch) {
    let locations = dispatch
        .exchange_outputs
        .iter()
        .map(|location| {
            (
                location.worker_uri.clone(),
                crate::exchange::ExchangeIdentity {
                    exchange_id: location.exchange_id.clone(),
                    task_id: location.producer.clone(),
                    output_partition: location.output_partition,
                },
            )
        })
        .collect();
    release_exchange_locations(client, token, locations).await;
}

fn general_distributed_eligible(plan: &LogicalPlan) -> bool {
    match plan {
        LogicalPlan::Aggregate {
            input, aggregates, ..
        } => {
            let supported = aggregates.iter().all(|aggregate| {
                !matches!(
                    aggregate,
                    AggregateExpr::Sum { distinct: true, .. }
                        | AggregateExpr::Avg { distinct: true, .. }
                )
            });
            supported && general_distributed_eligible(input)
        }
        LogicalPlan::Project { input, .. }
        | LogicalPlan::Filter { input, .. }
        | LogicalPlan::Sort { input, .. }
        | LogicalPlan::Limit { input, .. }
        | LogicalPlan::Offset { input, .. }
        | LogicalPlan::Distinct { input, .. }
        | LogicalPlan::Window { input, .. } => general_distributed_eligible(input),
        LogicalPlan::Join { left, right, .. }
        | LogicalPlan::Intersect { left, right }
        | LogicalPlan::Except { left, right } => {
            general_distributed_eligible(left) && general_distributed_eligible(right)
        }
        LogicalPlan::SemiJoin { .. } | LogicalPlan::AntiJoin { .. } => false,
        LogicalPlan::Union { inputs, .. } => inputs.iter().all(general_distributed_eligible),
        LogicalPlan::Scan { .. } => true,
    }
}

fn task_request_from_dispatch(dispatch: &TaskDispatch, context: &QueryContext) -> TaskRequest {
    TaskRequest {
        query_id: dispatch.assignment.task_id.query_id.clone(),
        stage_id: dispatch.assignment.task_id.stage_id.0,
        attempt: dispatch.assignment.task_id.attempt,
        query: String::new(),
        catalog: context.catalog.clone(),
        schema: context.schema.clone(),
        // Executable fragments already carry resolved scan locations and data
        // snapshot versions; they do not consult the worker catalog.
        catalog_snapshot_id: None,
        partition_index: dispatch.assignment.task_id.partition,
        partition_count: dispatch.execution_partition.count,
        fragment: Some(dispatch.fragment.clone()),
        execution_partition: Some(ExecutionPartitionRequest {
            index: dispatch.execution_partition.index,
            count: dispatch.execution_partition.count,
        }),
        exchange_inputs: dispatch
            .exchange_inputs
            .iter()
            .map(|location| ExchangeLocationRequest {
                exchange_id: location.exchange_id.clone(),
                producer: location.producer.clone(),
                output_partition: location.output_partition,
                worker_uri: location.worker_uri.clone(),
            })
            .collect(),
        exchange_outputs: dispatch
            .exchange_outputs
            .iter()
            .map(|location| ExchangeLocationRequest {
                exchange_id: location.exchange_id.clone(),
                producer: location.producer.clone(),
                output_partition: location.output_partition,
                worker_uri: location.worker_uri.clone(),
            })
            .collect(),
    }
}

fn schedule_completed_exchange_cleanup(
    client: &reqwest::Client,
    token: &str,
    orchestrator: &mut CoordinatorOrchestrator,
    cleanups: &mut tokio::task::JoinSet<()>,
) {
    let Ok(intents) = orchestrator.drain_exchange_cleanup() else {
        return;
    };
    let locations: Vec<(String, crate::exchange::ExchangeIdentity)> = intents
        .into_iter()
        .flat_map(|cleanup| {
            cleanup.locations.into_iter().map(move |location| {
                (
                    location.worker_uri,
                    crate::exchange::ExchangeIdentity {
                        exchange_id: cleanup.exchange_id.clone(),
                        task_id: location.producer,
                        output_partition: location.output_partition,
                    },
                )
            })
        })
        .collect();
    if locations.is_empty() {
        return;
    }
    spawn_exchange_cleanup(cleanups, client.clone(), token.to_owned(), locations);
}

fn spawn_exchange_cleanup(
    cleanups: &mut tokio::task::JoinSet<()>,
    client: reqwest::Client,
    token: String,
    locations: Vec<(String, crate::exchange::ExchangeIdentity)>,
) {
    cleanups.spawn(async move {
        release_exchange_locations(&client, &token, locations).await;
    });
}

const MAX_CONCURRENT_EXCHANGE_RELEASES: usize = 16;

async fn run_bounded_exchange_releases<F>(releases: Vec<F>)
where
    F: std::future::Future<Output = ()>,
{
    futures::stream::iter(releases)
        .buffer_unordered(MAX_CONCURRENT_EXCHANGE_RELEASES)
        .for_each(|()| async {})
        .await;
}

async fn release_exchange_locations(
    client: &reqwest::Client,
    token: &str,
    locations: Vec<(String, crate::exchange::ExchangeIdentity)>,
) {
    let releases = locations
        .into_iter()
        .map(|(worker_uri, identity)| async move {
            let _ = crate::exchange::release_exchange(client, &worker_uri, token, &identity).await;
        })
        .collect();
    run_bounded_exchange_releases(releases).await;
}

async fn execute_distributed_top_n(
    state: &Arc<AppState>,
    query_id: &str,
    sql: &str,
    context: &QueryContext,
    plan: &LogicalPlan,
) -> Option<Result<(TaskResponse, StageTelemetry), String>> {
    let (sort_exprs, limit) = top_n_merge_contract(plan)?;
    let worker_selection = {
        let mut cluster = state.cluster.write().await;
        workers_for_catalog_snapshot(&mut cluster, &context.catalog_snapshot_id)
    };
    let mut workers = match worker_selection {
        Ok(workers) => workers,
        Err(error) => return Some(Err(error)),
    };
    workers.sort_unstable_by(|left, right| left.node_id.cmp(&right.node_id));
    if workers.len() < 2 {
        return None;
    }

    let started = Instant::now();
    let partition_count = workers.len();
    let client = state.internal_http_client.clone();
    let exchange_token = state.config.exchange_token.clone();
    let mut tasks = tokio::task::JoinSet::new();
    for partition_index in 0..partition_count {
        let client = client.clone();
        let exchange_token = exchange_token.clone();
        let candidates = crate::scheduler::task_candidates(
            &workers,
            partition_index,
            crate::scheduler::RetryPolicy::default(),
        );
        let query_id = query_id.to_owned();
        let query = sql.to_owned();
        let catalog = context.catalog.clone();
        let schema_name = context.schema.clone();
        let catalog_snapshot_id = context.catalog_snapshot_id.clone();
        tasks.spawn(async move {
            let mut failures = Vec::new();
            for (attempt, worker) in candidates {
                let request = TaskRequest {
                    query_id: query_id.clone(),
                    stage_id: 0,
                    attempt,
                    query: query.clone(),
                    catalog: catalog.clone(),
                    schema: schema_name.clone(),
                    catalog_snapshot_id: Some(catalog_snapshot_id.clone()),
                    partition_index,
                    partition_count,
                    fragment: None,
                    execution_partition: None,
                    exchange_inputs: vec![],
                    exchange_outputs: vec![],
                };
                let task_id = kaveon_core::TaskId {
                    query_id: request.query_id.clone(),
                    stage_id: kaveon_core::StageId(request.stage_id),
                    partition: partition_index,
                    attempt,
                }
                .to_string();
                match execute_remote_task(&client, &worker, &request, exchange_token.as_deref())
                    .await
                {
                    Ok((schema, batches, elapsed_us, output_bytes, scan, execution)) => {
                        let output_rows = batches.iter().map(|batch| batch.num_rows()).sum();
                        let telemetry = TaskTelemetry {
                            task_id,
                            node_id: worker.node_id,
                            partition_index,
                            elapsed_us,
                            output_rows,
                            output_batches: batches.len(),
                            output_bytes,
                            execution,
                            scan,
                        };
                        return Ok((schema, batches, telemetry));
                    }
                    Err(error) => {
                        failures.push(error.message);
                        if !error.retryable {
                            break;
                        }
                    }
                }
            }
            Err(format!(
                "partition {partition_index} exhausted worker attempts: {}",
                failures.join("; ")
            ))
        });
    }

    let mut schema = None;
    let mut partial_batches = Vec::new();
    let mut task_metrics = Vec::with_capacity(partition_count);
    while let Some(result) = tasks.join_next().await {
        match result {
            Ok(Ok((worker_schema, batches, telemetry))) => {
                if schema
                    .as_ref()
                    .is_some_and(|expected| expected != &worker_schema)
                {
                    return Some(Err("workers returned incompatible TopN schemas".into()));
                }
                schema.get_or_insert(worker_schema);
                partial_batches.extend(batches);
                task_metrics.push(telemetry);
            }
            Ok(Err(error)) => return Some(Err(error)),
            Err(error) => return Some(Err(format!("worker task failed: {error}"))),
        }
    }

    let Some(schema) = schema else {
        return Some(Err(
            "distributed TopN completed without a result schema".into()
        ));
    };
    let merged = match merge_top_n(&schema, &partial_batches, &sort_exprs, limit) {
        Ok(merged) => merged,
        Err(error) => return Some(Err(format!("cannot merge distributed TopN: {error}"))),
    };
    let total_elapsed_us = elapsed_us(started);
    task_metrics.sort_unstable_by_key(|task| task.partition_index);
    let batches = merged.into_iter().collect::<Vec<_>>();
    Some(Ok((
        TaskResponse {
            columns: columns_from_schema(&schema),
            data: batches_to_json(&batches),
            elapsed_us: total_elapsed_us,
        },
        StageTelemetry {
            stage_id: 0,
            state: "FINISHED",
            task_count: partition_count,
            completed_tasks: task_metrics.len(),
            elapsed_us: total_elapsed_us,
            tasks: task_metrics,
        },
    )))
}

fn top_n_merge_contract(plan: &LogicalPlan) -> Option<(Vec<SortExpr>, usize)> {
    let LogicalPlan::Limit { input, count } = plan else {
        return None;
    };
    let LogicalPlan::Sort {
        input: sort_input,
        order_by,
    } = input.as_ref()
    else {
        return None;
    };
    if !distributed_scan_input(sort_input) {
        return None;
    }
    Some((
        order_by
            .iter()
            .map(|(expr, ascending)| SortExpr::new(expr.clone(), *ascending))
            .collect(),
        *count,
    ))
}

async fn execute_distributed_aggregate(
    state: &Arc<AppState>,
    query_id: &str,
    sql: &str,
    context: &QueryContext,
    plan: &LogicalPlan,
    memory: &kaveon_core::QueryMemoryPool,
) -> Option<Result<(TaskResponse, StageTelemetry), String>> {
    if exact_metadata_count_plan(plan) {
        return None;
    }
    let (group_count, operations) = aggregate_merge_contract(plan)?;
    let worker_selection = {
        let mut cluster = state.cluster.write().await;
        workers_for_catalog_snapshot(&mut cluster, &context.catalog_snapshot_id)
    };
    let mut workers = match worker_selection {
        Ok(workers) => workers,
        Err(error) => return Some(Err(error)),
    };
    workers.sort_unstable_by(|left, right| left.node_id.cmp(&right.node_id));
    if workers.len() < 2 {
        return None;
    }

    let started = Instant::now();
    let partition_count = workers.len();
    let client = state.internal_http_client.clone();
    let exchange_token = state.config.exchange_token.clone();
    let mut tasks = tokio::task::JoinSet::new();
    for partition_index in 0..partition_count {
        let client = client.clone();
        let exchange_token = exchange_token.clone();
        let candidates = crate::scheduler::task_candidates(
            &workers,
            partition_index,
            crate::scheduler::RetryPolicy::default(),
        );
        let query_id = query_id.to_owned();
        let query = sql.to_owned();
        let catalog = context.catalog.clone();
        let schema_name = context.schema.clone();
        let catalog_snapshot_id = context.catalog_snapshot_id.clone();
        tasks.spawn(async move {
            let mut failures = Vec::new();
            for (attempt, worker) in candidates {
                let request = TaskRequest {
                    query_id: query_id.clone(),
                    stage_id: 0,
                    attempt,
                    query: query.clone(),
                    catalog: catalog.clone(),
                    schema: schema_name.clone(),
                    catalog_snapshot_id: Some(catalog_snapshot_id.clone()),
                    partition_index,
                    partition_count,
                    fragment: None,
                    execution_partition: None,
                    exchange_inputs: vec![],
                    exchange_outputs: vec![],
                };
                let task_id = kaveon_core::TaskId {
                    query_id: request.query_id.clone(),
                    stage_id: kaveon_core::StageId(request.stage_id),
                    partition: partition_index,
                    attempt,
                }
                .to_string();
                match execute_remote_task(&client, &worker, &request, exchange_token.as_deref())
                    .await
                {
                    Ok((schema, batches, elapsed_us, output_bytes, scan, execution)) => {
                        let data = batches_to_json(&batches);
                        let telemetry = TaskTelemetry {
                            task_id,
                            node_id: worker.node_id,
                            partition_index,
                            elapsed_us,
                            output_rows: data.len(),
                            output_batches: batches.len(),
                            output_bytes,
                            execution,
                            scan,
                        };
                        return Ok((
                            TaskResponse {
                                columns: columns_from_schema(&schema),
                                data,
                                elapsed_us,
                            },
                            telemetry,
                        ));
                    }
                    Err(error) => {
                        failures.push(error.message);
                        if !error.retryable {
                            break;
                        }
                    }
                }
            }
            Err(format!(
                "partition {partition_index} exhausted worker attempts: {}",
                failures.join("; ")
            ))
        });
    }

    let mut partials = Vec::with_capacity(partition_count);
    let mut task_metrics = Vec::with_capacity(partition_count);
    while let Some(result) = tasks.join_next().await {
        match result {
            Ok(Ok((response, telemetry))) => {
                partials.push(response);
                task_metrics.push(telemetry);
            }
            Ok(Err(error)) => return Some(Err(error)),
            Err(error) => return Some(Err(format!("worker task failed: {error}"))),
        }
    }
    let total_elapsed_us = elapsed_us(started);
    task_metrics.sort_unstable_by_key(|task| task.partition_index);
    let merged = merge_partial_aggregates(
        partials,
        group_count,
        &operations,
        total_elapsed_us,
        Some(memory),
    );
    Some(merged.map(|result| {
        (
            result,
            StageTelemetry {
                stage_id: 0,
                state: "FINISHED",
                task_count: partition_count,
                completed_tasks: task_metrics.len(),
                elapsed_us: total_elapsed_us,
                tasks: task_metrics,
            },
        )
    }))
}

/// Exact, unfiltered COUNT(*) can be answered from the immutable source
/// snapshot. Keeping it out of distributed execution avoids decoding and
/// exchanging every row merely to add per-partition counters.
fn exact_metadata_count_plan(plan: &LogicalPlan) -> bool {
    let aggregate = match plan {
        LogicalPlan::Project { input, .. } => input.as_ref(),
        _ => plan,
    };
    matches!(
        aggregate,
        LogicalPlan::Aggregate {
            input,
            group_by,
            aggregates,
        } if group_by.is_empty()
            && !aggregates.is_empty()
            && aggregates.iter().all(|aggregate| matches!(
                aggregate,
                AggregateExpr::Count {
                    expr: kaveon_core::Expr::Star,
                    distinct: false,
                }
            ))
            && matches!(input.as_ref(), LogicalPlan::Scan { .. })
    )
}

fn aggregate_merge_contract(plan: &LogicalPlan) -> Option<(usize, Vec<MergeOperation>)> {
    let aggregate = match plan {
        LogicalPlan::Aggregate { .. } => plan,
        LogicalPlan::Project { input, columns }
            if matches!(input.as_ref(), LogicalPlan::Aggregate { .. }) =>
        {
            let LogicalPlan::Aggregate {
                group_by,
                aggregates,
                ..
            } = input.as_ref()
            else {
                return None;
            };
            if columns.len() != group_by.len().saturating_add(aggregates.len()) {
                return None;
            }
            if !projection_preserves_aggregate_order(columns, group_by, aggregates) {
                return None;
            }
            input.as_ref()
        }
        _ => return None,
    };
    let LogicalPlan::Aggregate {
        input,
        group_by,
        aggregates,
    } = aggregate
    else {
        return None;
    };
    if !distributed_scan_input(input) {
        return None;
    }
    let operations = aggregates
        .iter()
        .map(|aggregate| match aggregate {
            AggregateExpr::Count {
                distinct: false, ..
            }
            | AggregateExpr::Sum {
                distinct: false, ..
            } => Some(MergeOperation::Add),
            AggregateExpr::Min(_) => Some(MergeOperation::Min),
            AggregateExpr::Max(_) => Some(MergeOperation::Max),
            AggregateExpr::Avg { .. }
            | AggregateExpr::Count { distinct: true, .. }
            | AggregateExpr::Sum { distinct: true, .. } => None,
        })
        .collect::<Option<Vec<_>>>()?;
    Some((group_by.len(), operations))
}

fn projection_preserves_aggregate_order(
    columns: &[kaveon_core::Expr],
    group_by: &[kaveon_core::Expr],
    aggregates: &[AggregateExpr],
) -> bool {
    let groups_match = columns
        .iter()
        .take(group_by.len())
        .zip(group_by)
        .all(|(projected, grouped)| expression_column(projected) == expression_column(grouped));
    let aggregates_match = columns
        .iter()
        .skip(group_by.len())
        .zip(aggregates)
        .all(|(projected, aggregate)| projected_aggregate_matches(projected, aggregate));
    groups_match && aggregates_match
}

fn expression_column(expr: &kaveon_core::Expr) -> Option<&str> {
    match expr {
        kaveon_core::Expr::Column(name) => Some(name),
        kaveon_core::Expr::Alias { expr, .. } => expression_column(expr),
        _ => None,
    }
}

fn projected_aggregate_matches(expr: &kaveon_core::Expr, aggregate: &AggregateExpr) -> bool {
    let expr = match expr {
        kaveon_core::Expr::Alias { expr, .. } => expr.as_ref(),
        _ => expr,
    };
    let kaveon_core::Expr::Function { name, args } = expr else {
        return false;
    };
    let expected_name = match aggregate {
        AggregateExpr::Count { .. } => "count",
        AggregateExpr::Sum { .. } => "sum",
        AggregateExpr::Avg { .. } => "avg",
        AggregateExpr::Min(_) => "min",
        AggregateExpr::Max(_) => "max",
    };
    if !name.eq_ignore_ascii_case(expected_name) || args.len() != 1 {
        return false;
    }
    let expected_expr = match aggregate {
        AggregateExpr::Count { expr, .. }
        | AggregateExpr::Sum { expr, .. }
        | AggregateExpr::Avg { expr, .. }
        | AggregateExpr::Min(expr)
        | AggregateExpr::Max(expr) => expr,
    };
    match (&args[0], expected_expr) {
        (kaveon_core::Expr::Star, kaveon_core::Expr::Star) => true,
        (left, right) => expression_column(left) == expression_column(right),
    }
}

fn distributed_scan_input(plan: &LogicalPlan) -> bool {
    match plan {
        LogicalPlan::Scan { .. } => true,
        LogicalPlan::Filter { input, .. } | LogicalPlan::Project { input, .. } => {
            distributed_scan_input(input)
        }
        LogicalPlan::Join { .. }
        | LogicalPlan::Aggregate { .. }
        | LogicalPlan::Sort { .. }
        | LogicalPlan::Limit { .. }
        | LogicalPlan::Offset { .. }
        | LogicalPlan::Distinct { .. }
        | LogicalPlan::Window { .. }
        | LogicalPlan::Union { .. }
        | LogicalPlan::Intersect { .. }
        | LogicalPlan::Except { .. } => false,
        LogicalPlan::SemiJoin { .. } | LogicalPlan::AntiJoin { .. } => false,
    }
}

fn merge_partial_aggregates(
    partials: Vec<TaskResponse>,
    group_count: usize,
    operations: &[MergeOperation],
    elapsed_us: u64,
    memory: Option<&kaveon_core::QueryMemoryPool>,
) -> Result<TaskResponse, String> {
    let columns = partials
        .first()
        .map(|partial| partial.columns.clone())
        .unwrap_or_default();
    let expected_columns = group_count.saturating_add(operations.len());
    if columns.len() != expected_columns {
        return Err(format!(
            "partial aggregate returned {} columns; expected {expected_columns}",
            columns.len()
        ));
    }
    let mut groups = std::collections::BTreeMap::<String, Vec<serde_json::Value>>::new();
    let account = memory
        .map(|memory| memory.operator("distributed-aggregate-merge"))
        .transpose()
        .map_err(|error| error.to_string())?;
    let mut reservations = Vec::new();
    for partial in partials {
        if partial.columns != columns {
            return Err("workers returned incompatible aggregate schemas".into());
        }
        for row in partial.data {
            if row.len() != expected_columns {
                return Err("worker returned a malformed aggregate row".into());
            }
            let key = serde_json::to_string(&row[..group_count])
                .map_err(|error| format!("cannot encode aggregate key: {error}"))?;
            match groups.get_mut(&key) {
                Some(existing) => {
                    for (offset, operation) in operations.iter().enumerate() {
                        let index = group_count + offset;
                        existing[index] =
                            merge_value(existing[index].clone(), row[index].clone(), *operation)?;
                    }
                }
                None => {
                    if let Some(account) = &account {
                        let row_bytes = serde_json::to_vec(&row)
                            .map_err(|error| format!("cannot size aggregate row: {error}"))?
                            .len() as u64;
                        reservations.push(
                            account
                                .reserve((key.len() as u64).saturating_add(row_bytes))
                                .map_err(|error| error.to_string())?,
                        );
                    }
                    groups.insert(key, row);
                }
            }
        }
    }
    Ok(TaskResponse {
        columns,
        data: groups.into_values().collect(),
        elapsed_us,
    })
}

fn merge_value(
    left: serde_json::Value,
    right: serde_json::Value,
    operation: MergeOperation,
) -> Result<serde_json::Value, String> {
    if left.is_null() {
        return Ok(right);
    }
    if right.is_null() {
        return Ok(left);
    }
    match operation {
        MergeOperation::Add => match (left.as_i64(), right.as_i64()) {
            (Some(left), Some(right)) => Ok(serde_json::json!(left.saturating_add(right))),
            _ => match (left.as_u64(), right.as_u64()) {
                (Some(left), Some(right)) => Ok(serde_json::json!(left.saturating_add(right))),
                _ => match (left.as_f64(), right.as_f64()) {
                    (Some(left), Some(right)) => Ok(serde_json::json!(left + right)),
                    _ => Err("additive aggregate returned a non-numeric value".into()),
                },
            },
        },
        MergeOperation::Min | MergeOperation::Max => {
            let ordering = compare_json_scalars(&left, &right)?;
            let take_left = matches!(operation, MergeOperation::Min)
                && ordering != std::cmp::Ordering::Greater
                || matches!(operation, MergeOperation::Max) && ordering != std::cmp::Ordering::Less;
            Ok(if take_left { left } else { right })
        }
    }
}

fn compare_json_scalars(
    left: &serde_json::Value,
    right: &serde_json::Value,
) -> Result<std::cmp::Ordering, String> {
    if let (Some(left), Some(right)) = (left.as_f64(), right.as_f64()) {
        return left
            .partial_cmp(&right)
            .ok_or_else(|| "aggregate value is not comparable".into());
    }
    if let (Some(left), Some(right)) = (left.as_str(), right.as_str()) {
        return Ok(left.cmp(right));
    }
    Err("aggregate values have incompatible scalar types".into())
}

fn batches_to_json(batches: &[arrow::record_batch::RecordBatch]) -> Vec<Vec<serde_json::Value>> {
    use arrow::array::{Array, AsArray};
    use arrow::datatypes::*;

    let mut rows = Vec::new();
    for batch in batches {
        let num_cols = batch.num_columns();
        for row in 0..batch.num_rows() {
            let mut cells = Vec::with_capacity(num_cols);
            for col in 0..num_cols {
                let arr = batch.column(col);
                if arr.is_null(row) {
                    cells.push(serde_json::Value::Null);
                    continue;
                }
                let val = match arr.data_type() {
                    DataType::Boolean => {
                        let v = arr
                            .as_any()
                            .downcast_ref::<arrow::array::BooleanArray>()
                            .unwrap()
                            .value(row);
                        serde_json::Value::Bool(v)
                    }
                    DataType::Int8 => serde_json::json!(arr.as_primitive::<Int8Type>().value(row)),
                    DataType::Int16 => {
                        serde_json::json!(arr.as_primitive::<Int16Type>().value(row))
                    }
                    DataType::Int32 => {
                        serde_json::json!(arr.as_primitive::<Int32Type>().value(row))
                    }
                    DataType::Int64 => {
                        serde_json::json!(arr.as_primitive::<Int64Type>().value(row))
                    }
                    DataType::UInt8 => {
                        serde_json::json!(arr.as_primitive::<UInt8Type>().value(row))
                    }
                    DataType::UInt16 => {
                        serde_json::json!(arr.as_primitive::<UInt16Type>().value(row))
                    }
                    DataType::UInt32 => {
                        serde_json::json!(arr.as_primitive::<UInt32Type>().value(row))
                    }
                    DataType::UInt64 => {
                        serde_json::json!(arr.as_primitive::<UInt64Type>().value(row))
                    }
                    DataType::Float32 => {
                        serde_json::json!(arr.as_primitive::<Float32Type>().value(row))
                    }
                    DataType::Float64 => {
                        serde_json::json!(arr.as_primitive::<Float64Type>().value(row))
                    }
                    DataType::Utf8 => {
                        serde_json::Value::String(arr.as_string::<i32>().value(row).to_owned())
                    }
                    DataType::LargeUtf8 => {
                        serde_json::Value::String(arr.as_string::<i64>().value(row).to_owned())
                    }
                    _ => serde_json::Value::String(format!("{:?}", arr.slice(row, 1))),
                };
                cells.push(val);
            }
            rows.push(cells);
        }
    }
    rows
}

fn elapsed_us(start: Instant) -> u64 {
    start.elapsed().as_micros().try_into().unwrap_or(u64::MAX)
}

fn unix_time_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .try_into()
        .unwrap_or(u64::MAX)
}

fn scan_telemetry(metrics: &kaveon_storage::ScanMetrics) -> ScanTelemetry {
    let snapshot = metrics.snapshot();
    ScanTelemetry {
        files_considered: snapshot.files_considered,
        files_opened: snapshot.files_opened,
        row_groups_considered: snapshot.row_groups_considered,
        row_groups_read: snapshot.row_groups_selected,
        row_groups_pruned: snapshot.row_groups_pruned(),
        rows_selected: snapshot.rows_selected,
        rows_emitted: snapshot.rows_emitted,
        batches_emitted: snapshot.batches_emitted,
        compressed_bytes_selected: snapshot.compressed_bytes_selected,
        snapshot_ns: duration_ns(snapshot.snapshot_elapsed),
        footer_ns: duration_ns(snapshot.footer_elapsed),
        read_ns: duration_ns(snapshot.read_elapsed),
        rows_per_second: snapshot.rows_per_second(),
        compressed_bytes_per_second: snapshot.compressed_bytes_per_second(),
    }
}

fn merge_task_scan_metrics<'a>(
    metrics: impl Iterator<Item = &'a kaveon_storage::ScanMetrics>,
) -> TaskScanMetrics {
    metrics.fold(TaskScanMetrics::default(), |mut total, metrics| {
        let snapshot = metrics.snapshot();
        total.files_considered += snapshot.files_considered;
        total.files_opened += snapshot.files_opened;
        total.row_groups_considered += snapshot.row_groups_considered;
        total.row_groups_selected += snapshot.row_groups_selected;
        total.rows_selected += snapshot.rows_selected;
        total.rows_emitted += snapshot.rows_emitted;
        total.compressed_bytes_selected += snapshot.compressed_bytes_selected;
        total.batches_emitted += snapshot.batches_emitted;
        total.snapshot_ns += duration_ns(snapshot.snapshot_elapsed);
        total.footer_ns += duration_ns(snapshot.footer_elapsed);
        total.read_ns += duration_ns(snapshot.read_elapsed);
        total
    })
}

fn distributed_scan_telemetry(stages: &[StageTelemetry]) -> (Vec<ScanTelemetry>, bool) {
    let tasks = stages
        .iter()
        .flat_map(|stage| stage.tasks.iter())
        .collect::<Vec<_>>();
    if tasks.is_empty() || tasks.iter().any(|task| task.scan.is_none()) {
        return (Vec::new(), false);
    }
    let total = tasks
        .into_iter()
        .filter_map(|task| task.scan.as_ref())
        .fold(TaskScanMetrics::default(), |mut total, scan| {
            total.files_considered += scan.files_considered;
            total.files_opened += scan.files_opened;
            total.row_groups_considered += scan.row_groups_considered;
            total.row_groups_selected += scan.row_groups_selected;
            total.rows_selected += scan.rows_selected;
            total.rows_emitted += scan.rows_emitted;
            total.compressed_bytes_selected += scan.compressed_bytes_selected;
            total.batches_emitted += scan.batches_emitted;
            total.snapshot_ns += scan.snapshot_ns;
            total.footer_ns += scan.footer_ns;
            total.read_ns += scan.read_ns;
            total
        });
    let read_elapsed = std::time::Duration::from_nanos(total.read_ns);
    let rows_per_second = if read_elapsed.is_zero() {
        0.0
    } else {
        total.rows_emitted as f64 / read_elapsed.as_secs_f64()
    };
    let compressed_bytes_per_second = if read_elapsed.is_zero() {
        0.0
    } else {
        total.compressed_bytes_selected as f64 / read_elapsed.as_secs_f64()
    };
    (
        vec![ScanTelemetry {
            files_considered: total.files_considered,
            files_opened: total.files_opened,
            row_groups_considered: total.row_groups_considered,
            row_groups_read: total.row_groups_selected,
            row_groups_pruned: total
                .row_groups_considered
                .saturating_sub(total.row_groups_selected),
            rows_selected: total.rows_selected,
            rows_emitted: total.rows_emitted,
            batches_emitted: total.batches_emitted,
            compressed_bytes_selected: total.compressed_bytes_selected,
            snapshot_ns: total.snapshot_ns,
            footer_ns: total.footer_ns,
            read_ns: total.read_ns,
            rows_per_second,
            compressed_bytes_per_second,
        }],
        true,
    )
}

fn duration_ns(duration: std::time::Duration) -> u64 {
    duration.as_nanos().try_into().unwrap_or(u64::MAX)
}

async fn finish_failed_query(
    query_id: &str,
    error: String,
    started: Instant,
    analysis_us: Option<u64>,
    planning_us: Option<u64>,
    logical_plan: Option<kaveon_core::PlanNode>,
) {
    if let Some(record) = QUERY_STORE.write().await.queries.get_mut(query_id) {
        if matches!(record.state, QueryState::Canceled) {
            return;
        }
        record.state = QueryState::Failed;
        record.error = Some(error);
        record.elapsed_ms = started.elapsed().as_millis().try_into().unwrap_or(u64::MAX);
        record.completed_at_ms = unix_time_ms();
        record.timings.analysis_us = analysis_us;
        record.timings.planning_us = planning_us;
        record.plan.logical = logical_plan;
        record.scans.clear();
    }
}

#[cfg(test)]
mod tests {
    #[tokio::test]
    async fn exchange_releases_are_concurrent_and_bounded() {
        let active = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0));
        let peak = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0));
        let releases = (0..(super::MAX_CONCURRENT_EXCHANGE_RELEASES * 2))
            .map(|_| {
                let active = active.clone();
                let peak = peak.clone();
                async move {
                    let current = active.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
                    peak.fetch_max(current, std::sync::atomic::Ordering::SeqCst);
                    tokio::time::sleep(std::time::Duration::from_millis(10)).await;
                    active.fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
                }
            })
            .collect();

        super::run_bounded_exchange_releases(releases).await;

        assert_eq!(active.load(std::sync::atomic::Ordering::SeqCst), 0);
        assert_eq!(
            peak.load(std::sync::atomic::Ordering::SeqCst),
            super::MAX_CONCURRENT_EXCHANGE_RELEASES as u64
        );
    }

    #[tokio::test]
    async fn exchange_cleanup_is_enqueued_without_waiting_for_network_completion() {
        let mut cleanups = tokio::task::JoinSet::new();
        let identity = crate::exchange::ExchangeIdentity {
            exchange_id: kaveon_core::ExchangeId("cleanup-exchange".into()),
            task_id: kaveon_core::TaskId {
                query_id: "cleanup-query".into(),
                stage_id: kaveon_core::StageId(0),
                partition: 0,
                attempt: 0,
            },
            output_partition: 0,
        };

        super::spawn_exchange_cleanup(
            &mut cleanups,
            reqwest::Client::new(),
            "token".into(),
            vec![("http://127.0.0.1:1".into(), identity)],
        );

        // Scheduling returns before the release future is joined, allowing the
        // coordinator to dispatch the next ready stage immediately.
        assert_eq!(cleanups.len(), 1);
        while cleanups.join_next().await.is_some() {}
    }

    #[tokio::test]
    async fn task_response_stream_retains_cache_until_slow_consumer_drops() {
        use futures::StreamExt;
        let metrics = super::TaskExecutionMetrics {
            exchange_input_bytes: 42,
            spill_compactions: 3,
            ..Default::default()
        };
        let cached = std::sync::Arc::new(
            crate::transport::CachedTaskResult::new(
                vec![1; 200_000],
                10,
                None,
                serde_json::to_string(&metrics).ok(),
            )
            .unwrap(),
        );
        let response =
            super::task_outcome_response(crate::lifecycle::TaskOutcome::Success(cached.clone()));
        let observed: super::TaskExecutionMetrics = serde_json::from_str(
            response
                .headers()
                .get("x-kaveon-task-execution-metrics")
                .unwrap()
                .to_str()
                .unwrap(),
        )
        .unwrap();
        assert_eq!(observed.exchange_input_bytes, 42);
        assert_eq!(observed.spill_compactions, 3);
        let mut stream = response.into_body().into_data_stream();
        assert_eq!(std::sync::Arc::strong_count(&cached), 2);
        assert_eq!(stream.next().await.unwrap().unwrap().len(), 64 * 1024);
        assert_eq!(std::sync::Arc::strong_count(&cached), 2);
        drop(stream);
        assert_eq!(std::sync::Arc::strong_count(&cached), 1);
    }

    use super::{
        ColumnInfo, MergeOperation, TaskRequest, TaskResponse, aggregate_merge_contract,
        await_task_memory, capabilities, collect_join_statistics_tables, decode_arrow_stream,
        durable_relation_statistics, encode_arrow_stream, exact_metadata_count_plan,
        exact_source_statistics, execute_analyze, general_distributed_eligible,
        merge_partial_aggregates, mutation_actor, parse_analyze_table, statistics_diagnostics,
        task_request_from_dispatch, top_n_merge_contract, validate_replacement,
    };
    use crate::security::Role;
    use arrow::array::{Int64Array, StringArray};

    #[test]
    fn analyze_parser_accepts_bounded_table_names_only() {
        assert_eq!(
            parse_analyze_table("ANALYZE \"sales\".\"orders\""),
            Some("sales.orders".into())
        );
        assert_eq!(
            parse_analyze_table("analyze lake.sales.orders"),
            Some("lake.sales.orders".into())
        );
        assert_eq!(parse_analyze_table("ANALYZE orders WHERE true"), None);
        assert_eq!(parse_analyze_table("ANALYZE a.b.c.d"), None);
    }

    #[test]
    fn statistics_loader_collects_unique_direct_join_relations() {
        let plan = kaveon_sql::logical_plan::sql_to_logical_plan(
            "SELECT * FROM events e JOIN customers c ON e.customer_id = c.id \
             JOIN customers c2 ON e.customer_id = c2.id",
        )
        .unwrap();
        let mut tables = std::collections::BTreeSet::new();
        collect_join_statistics_tables(&plan, &mut tables);
        assert_eq!(
            tables.into_iter().collect::<Vec<_>>(),
            ["customers".to_owned(), "events".to_owned()]
        );
    }
    use arrow::datatypes::{DataType, Field, Schema};
    use arrow::record_batch::RecordBatch;
    use std::sync::Arc;

    #[tokio::test]
    async fn memory_pressure_queues_tasks_without_consuming_fault_retries() {
        let admission = kaveon_core::MemoryAdmissionController::new(1_024).unwrap();
        let occupied = admission.admit("running", 1_024).unwrap();
        let lifecycle = crate::lifecycle::WorkerLifecycle::<()>::default();
        let cancellation = lifecycle.cancellations.token("waiting-query").unwrap();
        let controller = admission.clone();
        let mut waiter = tokio::spawn(async move {
            await_task_memory(&controller, "waiting-task".into(), 1_024, &cancellation).await
        });

        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(30), &mut waiter)
                .await
                .is_err()
        );
        drop(occupied);
        let admitted = tokio::time::timeout(std::time::Duration::from_secs(1), waiter)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert_eq!(admission.snapshot().current_bytes, 1_024);
        drop(admitted);
        assert_eq!(admission.snapshot().current_bytes, 0);
    }

    #[tokio::test]
    async fn memory_admission_wait_stops_when_query_is_canceled() {
        let admission = kaveon_core::MemoryAdmissionController::new(1_024).unwrap();
        let _occupied = admission.admit("running", 1_024).unwrap();
        let lifecycle = std::sync::Arc::new(crate::lifecycle::WorkerLifecycle::<()>::default());
        let cancellation = lifecycle.cancellations.token("waiting-query").unwrap();
        let controller = admission.clone();
        let waiter = tokio::spawn(async move {
            await_task_memory(&controller, "waiting-task".into(), 1_024, &cancellation).await
        });

        tokio::task::yield_now().await;
        assert!(lifecycle.cancellations.cancel("waiting-query").unwrap());
        let error = tokio::time::timeout(std::time::Duration::from_secs(1), waiter)
            .await
            .unwrap()
            .unwrap()
            .unwrap_err();
        assert!(error.contains("canceled"));
    }

    async fn analyze_test_state() -> (
        Arc<crate::AppState>,
        kaveon_catalog::product_commit::ProductCatalogCommit,
        std::path::PathBuf,
    ) {
        use kaveon_core::CatalogProvider;
        use parquet::arrow::ArrowWriter;
        let directory =
            std::env::temp_dir().join(format!("kaveon-server-analyze-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&directory).unwrap();
        let path = directory.join("orders.parquet");
        let schema = Arc::new(Schema::new(vec![Field::new("id", DataType::Int64, false)]));
        let batch = RecordBatch::try_new(
            schema.clone(),
            vec![Arc::new(Int64Array::from(vec![1, 2, 3]))],
        )
        .unwrap();
        let mut writer =
            ArrowWriter::try_new(std::fs::File::create(&path).unwrap(), schema.clone(), None)
                .unwrap();
        writer.write(&batch).unwrap();
        writer.close().unwrap();
        let storage = kaveon_storage::AdlsConditionalCommit::new(Arc::new(
            object_store::memory::InMemory::new(),
        ));
        let commit = kaveon_catalog::product_commit::ProductCatalogCommit::new(
            storage,
            "product",
            Arc::new(kaveon_catalog::product_metrics::TransactionMetrics::default()),
        )
        .unwrap();
        assert!(matches!(
            commit
                .initialize(
                    kaveon_catalog::product_manifest::CatalogSnapshot::empty("genesis").unwrap()
                )
                .await,
            kaveon_catalog::product_commit::CommitOutcome::Committed(_)
        ));
        let mut manager = kaveon_core::CatalogManager::new("lake", "sales");
        let mut provider = kaveon_core::MemoryCatalog::new(
            "lake",
            kaveon_core::StorageType::Local {
                base_path: directory.clone(),
            },
        )
        .with_schema("sales");
        provider
            .register_table(
                "sales",
                kaveon_core::TableMeta {
                    name: "orders".into(),
                    arrow_schema: schema,
                    location: "orders.parquet".into(),
                    access: kaveon_core::AccessPattern::Optimized,
                    format: kaveon_core::DataFormat::Parquet,
                },
            )
            .unwrap();
        manager.register_catalog(Box::new(provider));
        let mut state = catalog_test_state();
        state.catalog = tokio::sync::RwLock::new(Arc::new(crate::PublishedCatalog {
            manager,
            snapshot_id: "sha256:catalog-one".into(),
        }));
        state.product_transactions =
            crate::transaction_api::TransactionRegistry::enabled(commit.clone());
        (Arc::new(state), commit, directory)
    }

    fn analyze_context() -> super::QueryContext {
        super::QueryContext {
            engine_version: "test".into(),
            environment: "test".into(),
            principal: Some("admin".into()),
            user: Some("admin".into()),
            source: None,
            client: None,
            catalog: "lake".into(),
            schema: "sales".into(),
            time_zone: None,
            client_address: None,
            client_tags: vec![],
            result_delivery: None,
            catalog_snapshot_id: "sha256:catalog-one".into(),
        }
    }

    #[tokio::test]
    async fn analyze_requires_admin_and_publishes_exact_durable_binding() {
        let (state, commit, directory) = analyze_test_state().await;
        let catalog = state.catalog.read().await.clone();
        assert_eq!(
            exact_source_statistics(&catalog, "lake.sales.orders")
                .unwrap()
                .rows,
            3
        );
        let reader = crate::security::Identity {
            principal: "reader".into(),
            display_identity: None,
            role: Role::Reader,
        };
        let denied = execute_analyze(
            &state,
            &reader,
            "denied",
            &analyze_context(),
            "orders".into(),
            std::time::Instant::now(),
        )
        .await;
        assert_eq!(denied.status(), axum::http::StatusCode::FORBIDDEN);
        assert!(
            commit
                .read_current()
                .await
                .unwrap()
                .table_statistics
                .is_empty()
        );
        let admin = crate::security::Identity {
            principal: "admin".into(),
            display_identity: None,
            role: Role::Admin,
        };
        let response = execute_analyze(
            &state,
            &admin,
            "allowed",
            &analyze_context(),
            "orders".into(),
            std::time::Instant::now(),
        )
        .await;
        assert_eq!(response.status(), axum::http::StatusCode::OK);
        let snapshot = commit.read_current().await.unwrap();
        let stats = &snapshot.table_statistics["lake.sales.orders"];
        assert_eq!(stats.row_count, 3);
        assert_eq!(
            snapshot.runtime_table_sources["lake.sales.orders"].source_identity_sha256,
            stats.source_identity_sha256
        );
        assert_eq!(
            durable_relation_statistics(&catalog, &snapshot, "lake.sales.orders")
                .unwrap()
                .rows,
            3
        );
        let diagnostic = statistics_diagnostics(
            axum::extract::State(state.clone()),
            axum::Extension(admin.clone()),
        )
        .await;
        let body = axum::body::to_bytes(diagnostic.into_body(), 64 * 1024)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["statistics"][0]["table"], "lake.sales.orders");
        assert_eq!(json["statistics"][0]["row_count"], 3);
        assert_eq!(json["statistics"][0]["current"], true);
        assert_eq!(
            json["statistics"][0]["catalog_digest_prefix"]
                .as_str()
                .unwrap()
                .len(),
            12
        );
        assert_eq!(
            json["statistics"][0]["source_digest_prefix"]
                .as_str()
                .unwrap()
                .len(),
            12
        );
        let schema = Arc::new(Schema::new(vec![Field::new("id", DataType::Int64, false)]));
        let batch = RecordBatch::try_new(
            schema.clone(),
            vec![Arc::new(Int64Array::from(vec![10, 11, 12, 13]))],
        )
        .unwrap();
        let mut writer = parquet::arrow::ArrowWriter::try_new(
            std::fs::File::create(directory.join("orders.parquet")).unwrap(),
            schema,
            None,
        )
        .unwrap();
        writer.write(&batch).unwrap();
        writer.close().unwrap();
        assert!(durable_relation_statistics(&catalog, &snapshot, "lake.sales.orders").is_none());
        assert_eq!(
            exact_source_statistics(&catalog, "lake.sales.orders")
                .unwrap()
                .rows,
            4
        );
        let stale =
            statistics_diagnostics(axum::extract::State(state.clone()), axum::Extension(admin))
                .await;
        let body = axum::body::to_bytes(stale.into_body(), 64 * 1024)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["statistics"][0]["current"], false);
        let enabled = capabilities(axum::extract::State(state)).await.0;
        assert_eq!(enabled["native_analyze"], true);
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[tokio::test]
    async fn capability_is_false_without_durable_statistics_authority() {
        let state = Arc::new(catalog_test_state());
        assert_eq!(
            capabilities(axum::extract::State(state)).await.0["native_analyze"],
            false
        );
    }

    #[tokio::test]
    async fn statistics_diagnostics_require_admin() {
        let (state, _, directory) = analyze_test_state().await;
        let reader = crate::security::Identity {
            principal: "reader".into(),
            display_identity: None,
            role: Role::Reader,
        };
        let response =
            statistics_diagnostics(axum::extract::State(state), axum::Extension(reader)).await;
        assert_eq!(response.status(), axum::http::StatusCode::FORBIDDEN);
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn statement_lifecycle_guard_releases_capacity_and_cancels_detached_work() {
        let state = std::sync::Arc::new(catalog_test_state());
        for index in 0..1200 {
            let query_id = format!("completed-local-{index}");
            let token = state.lifecycle.cancellations.token(&query_id).unwrap();
            {
                let _guard = super::StatementLifecycleGuard {
                    state: state.clone(),
                    query_id,
                };
                assert!(!token.is_cancelled());
            }
            assert!(token.is_cancelled());
        }
    }

    fn catalog_test_state() -> crate::AppState {
        let config = crate::config::ServerConfig {
            catalog_admin_token: Some("admin-token".into()),
            exchange_token: Some("exchange-token-at-least-32-bytes-long".into()),
            ..crate::config::ServerConfig::default()
        };
        let catalog_store = kaveon_catalog::CatalogStore::open_in_memory().unwrap();
        let snapshot_id = catalog_store.snapshot_identity().unwrap();
        crate::AppState {
            disk_exchange_store: None,
            results: crate::results::ResultStore::default(),
            principal_admission: crate::security::PrincipalAdmission::default(),
            cluster: tokio::sync::RwLock::new(crate::cluster::ClusterState::new(&config)),
            catalog: tokio::sync::RwLock::new(Arc::new(crate::PublishedCatalog {
                manager: kaveon_core::CatalogManager::new("kaveon", "default"),
                snapshot_id,
            })),
            catalog_store,
            exchange_store: crate::exchange::ExchangeStore::default(),
            internal_http_client: reqwest::Client::new(),
            lifecycle: crate::lifecycle::WorkerLifecycle::default(),
            memory_admission: kaveon_core::MemoryAdmissionController::new(
                config.memory_admission_limit_bytes,
            )
            .unwrap(),
            product_transactions: crate::transaction_api::TransactionRegistry::disabled(),
            config,
        }
    }

    #[tokio::test]
    async fn heartbeat_requires_exchange_auth_and_returns_required_catalog_identity() {
        let state = Arc::new(catalog_test_state());
        let mut worker = state.cluster.read().await.this_node.clone();
        worker.role = crate::cluster::NodeRole::Worker;
        worker.node_id = "worker-sync-test".into();

        let unauthorized = super::receive_heartbeat(
            axum::extract::State(state.clone()),
            axum::http::HeaderMap::new(),
            axum::Json(worker.clone()),
        )
        .await;
        assert_eq!(unauthorized.status(), axum::http::StatusCode::UNAUTHORIZED);

        let mut headers = axum::http::HeaderMap::new();
        headers.insert(
            axum::http::header::AUTHORIZATION,
            "Bearer exchange-token-at-least-32-bytes-long"
                .parse()
                .unwrap(),
        );
        let accepted = super::receive_heartbeat(
            axum::extract::State(state.clone()),
            headers,
            axum::Json(worker),
        )
        .await;
        assert_eq!(accepted.status(), axum::http::StatusCode::OK);
        assert!(
            state
                .cluster
                .read()
                .await
                .workers
                .contains_key("worker-sync-test")
        );
    }

    #[tokio::test]
    async fn exchange_prefetch_overlaps_work_with_bounded_deterministic_order() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        let active = Arc::new(AtomicUsize::new(0));
        let peak = Arc::new(AtomicUsize::new(0));
        let results = super::buffered_ordered((0..20).collect(), 4, {
            let active = active.clone();
            let peak = peak.clone();
            move |value| {
                let active = active.clone();
                let peak = peak.clone();
                async move {
                    let now = active.fetch_add(1, Ordering::AcqRel) + 1;
                    peak.fetch_max(now, Ordering::AcqRel);
                    tokio::time::sleep(std::time::Duration::from_millis((5 - value % 5) as u64))
                        .await;
                    active.fetch_sub(1, Ordering::AcqRel);
                    Ok::<_, ()>(value)
                }
            }
        })
        .await;
        assert_eq!(
            results.into_iter().collect::<Result<Vec<_>, _>>(),
            Ok((0..20).collect())
        );
        assert_eq!(peak.load(Ordering::Acquire), 4);
        assert_eq!(active.load(Ordering::Acquire), 0);
    }

    #[test]
    fn catalog_mutations_require_bearer_authorization_and_actor() {
        let state = catalog_test_state();
        let mut headers = axum::http::HeaderMap::new();
        assert_eq!(
            mutation_actor(&state, &headers).unwrap_err().status(),
            axum::http::StatusCode::UNAUTHORIZED
        );

        headers.insert(
            axum::http::header::AUTHORIZATION,
            "Bearer wrong-token".parse().unwrap(),
        );
        headers.insert("x-kaveon-actor", "engineer@example.com".parse().unwrap());
        assert_eq!(
            mutation_actor(&state, &headers).unwrap_err().status(),
            axum::http::StatusCode::UNAUTHORIZED
        );

        headers.insert(
            axum::http::header::AUTHORIZATION,
            "Bearer admin-token".parse().unwrap(),
        );
        assert_eq!(
            mutation_actor(&state, &headers).unwrap(),
            "engineer@example.com"
        );
    }

    #[test]
    fn catalog_metadata_updates_may_preserve_lifecycle() {
        let current = kaveon_core::CatalogRevision::new(2).unwrap();
        let next = current.next().unwrap();
        assert!(
            validate_replacement(
                current,
                kaveon_core::CatalogLifecycle::Active,
                next,
                kaveon_core::CatalogLifecycle::Active,
            )
            .is_ok()
        );
    }

    #[tokio::test]
    async fn distributed_fragment_planning_retains_pinned_catalog_after_publish() {
        use kaveon_core::{
            AccessPattern, CatalogManager, CatalogProvider, DataFormat, FragmentOperator,
            MemoryCatalog, StorageType, TableMeta,
        };

        fn manager(location: &str) -> CatalogManager {
            let mut catalog = MemoryCatalog::new(
                "lake",
                StorageType::Local {
                    base_path: std::path::PathBuf::from("/catalog-root"),
                },
            )
            .with_schema("analytics");
            catalog
                .register_table(
                    "analytics",
                    TableMeta {
                        name: "events".into(),
                        arrow_schema: Arc::new(Schema::new(vec![Field::new(
                            "id",
                            DataType::Int64,
                            false,
                        )])),
                        location: location.into(),
                        access: AccessPattern::Shortcut,
                        format: DataFormat::Parquet,
                    },
                )
                .unwrap();
            let mut manager = CatalogManager::new("lake", "analytics");
            manager.register_catalog(Box::new(catalog));
            manager
        }

        let state = catalog_test_state();
        *state.catalog.write().await = Arc::new(crate::PublishedCatalog {
            manager: manager("snapshot-v1/events.parquet"),
            snapshot_id: "snapshot-v1".into(),
        });
        let pinned = state.catalog.read().await.clone();
        assert_eq!(pinned.snapshot_id, "snapshot-v1");

        // Publish a new catalog head while the query retains its original Arc.
        *state.catalog.write().await = Arc::new(crate::PublishedCatalog {
            manager: manager("snapshot-v2/events.parquet"),
            snapshot_id: "snapshot-v2".into(),
        });

        let mut plan =
            kaveon_sql::logical_plan::sql_to_logical_plan("SELECT id FROM lake.analytics.events")
                .unwrap();
        crate::planner::qualify_tables(&mut plan, "lake", "analytics");
        let fragments =
            crate::planner::build_executable_fragments("query-pinned", &plan, &pinned, 2).unwrap();
        let pinned_sources = fragments
            .values()
            .flat_map(|fragment| fragment.nodes.iter())
            .filter_map(|node| match &node.operator {
                FragmentOperator::Scan(scan) => Some(scan.source_uri.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>();

        assert!(!pinned_sources.is_empty());
        assert!(
            pinned_sources
                .iter()
                .all(|source| source.contains("snapshot-v1/events.parquet"))
        );
        assert!(
            state
                .catalog
                .read()
                .await
                .resolve_table(&kaveon_core::TableReference::parse("lake.analytics.events",))
                .unwrap()
                .table
                .location
                .contains("snapshot-v2/events.parquet")
        );
    }

    #[tokio::test]
    async fn worker_rejects_mismatched_catalog_identity_before_raw_sql_execution() {
        use kaveon_core::{
            AccessPattern, CatalogManager, CatalogProvider, DataFormat, MemoryCatalog, StorageType,
            TableMeta,
        };

        let mut catalog = MemoryCatalog::new(
            "lake",
            StorageType::Local {
                base_path: std::path::PathBuf::from("/catalog-root"),
            },
        )
        .with_schema("analytics");
        catalog
            .register_table(
                "analytics",
                TableMeta {
                    name: "events".into(),
                    arrow_schema: Arc::new(Schema::new(vec![Field::new(
                        "id",
                        DataType::Int64,
                        false,
                    )])),
                    location: "snapshot-v2/events.parquet".into(),
                    access: AccessPattern::Shortcut,
                    format: DataFormat::Parquet,
                },
            )
            .unwrap();
        let mut manager = CatalogManager::new("lake", "analytics");
        manager.register_catalog(Box::new(catalog));
        let state = catalog_test_state();
        let matching = "sha256:durable-catalog-test".to_owned();
        *state.catalog.write().await = Arc::new(crate::PublishedCatalog {
            manager,
            snapshot_id: matching.clone(),
        });

        let request = |identity: &str| TaskRequest {
            query_id: "query-raw".into(),
            stage_id: 0,
            attempt: 0,
            query: "SELECT id FROM lake.analytics.events".into(),
            catalog: "lake".into(),
            schema: "analytics".into(),
            catalog_snapshot_id: Some(identity.into()),
            partition_index: 0,
            partition_count: 1,
            fragment: None,
            execution_partition: None,
            exchange_inputs: vec![],
            exchange_outputs: vec![],
        };

        assert!(
            super::validate_task_catalog_snapshot(&state, &request(&matching))
                .await
                .is_ok()
        );
        let mismatch = super::validate_task_catalog_snapshot(
            &state,
            &request("sha256:coordinator-pinned-an-older-head"),
        )
        .await
        .unwrap_err();
        assert_eq!(mismatch.status(), axum::http::StatusCode::CONFLICT);

        let mut missing = request(&matching);
        missing.catalog_snapshot_id = None;
        let missing = super::validate_task_catalog_snapshot(&state, &missing)
            .await
            .unwrap_err();
        assert_eq!(missing.status(), axum::http::StatusCode::BAD_REQUEST);
    }

    #[test]
    fn catalog_identity_is_canonical_across_registration_order() {
        use kaveon_core::{
            AccessPattern, CatalogManager, CatalogProvider, DataFormat, MemoryCatalog, StorageType,
            TableMeta,
        };

        fn manager(schema_order: &[&str], table_order: &[&str]) -> CatalogManager {
            let mut catalog = MemoryCatalog::new(
                "lake",
                StorageType::AdlsGen2 {
                    account: "account".into(),
                    container: "container".into(),
                    root_path: "root".into(),
                },
            );
            for schema in schema_order {
                catalog = catalog.with_schema(*schema);
            }
            for table in table_order {
                catalog
                    .register_table(
                        "analytics",
                        TableMeta {
                            name: (*table).into(),
                            arrow_schema: Arc::new(Schema::new(vec![Field::new(
                                "id",
                                DataType::Int64,
                                false,
                            )])),
                            location: format!("snapshot/{table}.parquet"),
                            access: AccessPattern::Shortcut,
                            format: DataFormat::Parquet,
                        },
                    )
                    .unwrap();
            }
            let mut manager = CatalogManager::new("lake", "analytics");
            manager.register_catalog(Box::new(catalog));
            manager
        }

        let first = manager(&["analytics", "empty"], &["events", "users"]);
        let reversed = manager(&["empty", "analytics"], &["users", "events"]);
        assert_eq!(
            super::catalog_snapshot_identity(&first, "lake").unwrap(),
            super::catalog_snapshot_identity(&reversed, "lake").unwrap()
        );
    }

    fn columns() -> Vec<ColumnInfo> {
        vec![
            ColumnInfo {
                name: "region".into(),
                data_type: "Utf8".into(),
            },
            ColumnInfo {
                name: "count(*)".into(),
                data_type: "UInt64".into(),
            },
        ]
    }

    #[test]
    fn legacy_task_request_remains_wire_compatible() {
        let request: TaskRequest = serde_json::from_value(serde_json::json!({
            "query_id": "query-1",
            "stage_id": 0,
            "attempt": 0,
            "query": "SELECT 1",
            "catalog": "kaveon",
            "schema": "default",
            "partition_index": 1,
            "partition_count": 2
        }))
        .unwrap();

        assert!(request.fragment.is_none());
        assert!(request.execution_partition.is_none());
        assert!(request.exchange_inputs.is_empty());
        assert!(request.exchange_outputs.is_empty());
    }

    #[test]
    fn fragment_request_preserves_assignment_and_partition_contract() {
        use kaveon_core::{
            EXECUTABLE_FRAGMENT_VERSION, ExecutableFragment, FragmentNodeId, StageId,
            TaskAssignment, TaskId,
        };

        let task_id = TaskId {
            query_id: "query-1".into(),
            stage_id: StageId(3),
            partition: 2,
            attempt: 1,
        };
        let dispatch = crate::orchestrator::TaskDispatch {
            assignment: TaskAssignment {
                task_id: task_id.clone(),
                worker_id: "worker-1".into(),
                splits: vec![],
                input_exchanges: vec![],
                output_exchanges: vec![],
            },
            execution_partition: crate::orchestrator::ExecutionPartition { index: 2, count: 4 },
            fragment: ExecutableFragment {
                version: EXECUTABLE_FRAGMENT_VERSION,
                stage_id: StageId(3),
                root: FragmentNodeId(0),
                nodes: vec![],
            },
            exchange_inputs: vec![],
            exchange_outputs: vec![],
        };
        let context = super::QueryContext {
            engine_version: "test".into(),
            environment: "test".into(),
            principal: None,
            user: None,
            source: None,
            client: None,
            catalog: "kaveon".into(),
            schema: "default".into(),
            time_zone: None,
            client_address: None,
            client_tags: vec![],
            result_delivery: None,
            catalog_snapshot_id: "sha256:test".into(),
        };

        let request = task_request_from_dispatch(&dispatch, &context);
        assert_eq!(request.query_id, task_id.query_id);
        assert_eq!(request.stage_id, 3);
        assert_eq!(request.attempt, 1);
        assert_eq!(request.partition_index, 2);
        assert_eq!(request.partition_count, 4);
        assert!(request.catalog_snapshot_id.is_none());
        assert_eq!(request.execution_partition.unwrap().count, 4);
        assert!(request.fragment.is_some());
    }

    #[test]
    fn submitted_user_cannot_override_authenticated_query_history_user() {
        let request: super::StatementRequest = serde_json::from_value(serde_json::json!({
            "query": "SELECT 1",
            "user": "spoofed-admin"
        }))
        .unwrap();
        let identity = crate::security::Identity {
            principal: "entra:tenant:object".into(),
            display_identity: Some("ada@example.com".into()),
            role: Role::Analyst,
        };
        assert_eq!(request.user.as_deref(), Some("spoofed-admin"));
        assert_eq!(identity.display_name(), "ada@example.com");
        assert_ne!(request.user.as_deref(), Some(identity.display_name()));
    }

    #[test]
    fn general_fragment_path_accepts_supported_plan_families() {
        let scan = kaveon_sql::logical_plan::sql_to_logical_plan(
            "SELECT region FROM orders WHERE total > 10",
        )
        .unwrap();
        let aggregate = kaveon_sql::logical_plan::sql_to_logical_plan(
            "SELECT region, AVG(total) FROM orders GROUP BY region",
        )
        .unwrap();
        let join = kaveon_sql::logical_plan::sql_to_logical_plan(
            "SELECT * FROM orders JOIN customers ON orders.customer_id = customers.id",
        )
        .unwrap();

        assert!(general_distributed_eligible(&scan));
        assert!(general_distributed_eligible(&aggregate));
        assert!(general_distributed_eligible(&join));
    }

    #[test]
    fn exact_unfiltered_counts_bypass_row_decoding_distributed_paths() {
        for sql in [
            "SELECT COUNT(*) FROM events",
            "SELECT COUNT(*) AS rows FROM events",
            "SELECT COUNT(*), COUNT(*) FROM events",
        ] {
            let plan = kaveon_sql::logical_plan::sql_to_logical_plan(sql).unwrap();
            assert!(exact_metadata_count_plan(&plan), "{sql}");
        }
        for sql in [
            "SELECT COUNT(*) FROM events WHERE id > 10",
            "SELECT COUNT(id) FROM events",
            "SELECT id, COUNT(*) FROM events GROUP BY id",
        ] {
            let plan = kaveon_sql::logical_plan::sql_to_logical_plan(sql).unwrap();
            assert!(!exact_metadata_count_plan(&plan), "{sql}");
        }
    }

    #[test]
    fn merges_partial_group_counts() {
        let partials = vec![
            TaskResponse {
                columns: columns(),
                data: vec![vec![serde_json::json!("east"), serde_json::json!(2)]],
                elapsed_us: 1,
            },
            TaskResponse {
                columns: columns(),
                data: vec![
                    vec![serde_json::json!("east"), serde_json::json!(3)],
                    vec![serde_json::json!("west"), serde_json::json!(4)],
                ],
                elapsed_us: 1,
            },
        ];
        let merged =
            merge_partial_aggregates(partials, 1, &[MergeOperation::Add], 2, None).unwrap();
        assert_eq!(
            merged.data,
            vec![
                vec![serde_json::json!("east"), serde_json::json!(5)],
                vec![serde_json::json!("west"), serde_json::json!(4)],
            ]
        );
    }

    #[test]
    fn distributed_aggregate_merge_fails_closed_and_releases_memory() {
        let pool = kaveon_core::QueryMemoryPool::new("aggregate-merge-pressure", 64).unwrap();
        let partials = vec![TaskResponse {
            columns: vec![
                ColumnInfo {
                    name: "region".into(),
                    data_type: "Utf8".into(),
                },
                ColumnInfo {
                    name: "count".into(),
                    data_type: "Int64".into(),
                },
            ],
            data: vec![vec![
                serde_json::json!("a-region-name-large-enough-to-exceed-the-budget"),
                serde_json::json!(1),
            ]],
            elapsed_us: 1,
        }];

        let error =
            match merge_partial_aggregates(partials, 1, &[MergeOperation::Add], 2, Some(&pool)) {
                Ok(_) => panic!("aggregate merge unexpectedly exceeded its memory budget"),
                Err(error) => error,
            };
        assert!(error.contains("cannot reserve"), "{error}");
        assert_eq!(pool.snapshot().current_bytes, 0);
    }

    #[test]
    fn scheduler_filters_workers_by_published_catalog_identity() {
        let config = crate::config::ServerConfig::default();
        let mut cluster = crate::cluster::ClusterState::new(&config);
        let mut worker = cluster.this_node.clone();
        worker.role = crate::cluster::NodeRole::Worker;
        worker.node_id = "matching".into();
        worker.catalog_snapshot_id = Some("snapshot-required".into());
        cluster.register_worker(worker.clone());
        worker.node_id = "stale".into();
        worker.catalog_snapshot_id = Some("snapshot-old".into());
        cluster.register_worker(worker);

        let error =
            super::workers_for_catalog_snapshot(&mut cluster, "snapshot-required").unwrap_err();
        assert!(error.starts_with("INSUFFICIENT_COMPATIBLE_WORKERS:"));

        cluster.workers.remove("stale");
        let compatible =
            super::workers_for_catalog_snapshot(&mut cluster, "snapshot-required").unwrap();
        assert_eq!(compatible.len(), 1);
        assert_eq!(compatible[0].node_id, "matching");
    }

    #[test]
    fn scheduler_reports_when_no_worker_has_required_catalog() {
        let config = crate::config::ServerConfig::default();
        let mut cluster = crate::cluster::ClusterState::new(&config);
        let mut worker = cluster.this_node.clone();
        worker.role = crate::cluster::NodeRole::Worker;
        worker.node_id = "legacy".into();
        worker.catalog_snapshot_id = None;
        cluster.register_worker(worker);

        let error =
            super::workers_for_catalog_snapshot(&mut cluster, "snapshot-required").unwrap_err();
        assert!(error.starts_with("NO_COMPATIBLE_WORKER:"));
    }

    #[test]
    fn aggregates_only_complete_worker_reader_counters() {
        let task = |scan| super::TaskTelemetry {
            task_id: "task".into(),
            node_id: "worker".into(),
            partition_index: 0,
            elapsed_us: 1,
            output_rows: 999,
            output_batches: 1,
            output_bytes: 1,
            execution: None,
            scan,
        };
        let scan = |rows_emitted, rows_selected| super::TaskScanMetrics {
            rows_emitted,
            rows_selected,
            compressed_bytes_selected: 12,
            ..Default::default()
        };
        let stages = vec![super::StageTelemetry {
            stage_id: 0,
            state: "FINISHED",
            task_count: 2,
            completed_tasks: 2,
            elapsed_us: 1,
            tasks: vec![task(Some(scan(40, 60))), task(Some(scan(30, 50)))],
        }];
        let (scans, complete) = super::distributed_scan_telemetry(&stages);
        assert!(complete);
        assert_eq!(scans[0].rows_emitted, 70);
        assert_eq!(scans[0].rows_selected, 110);
        assert_ne!(scans[0].rows_emitted, stages[0].tasks[0].output_rows as u64);
        let incomplete = vec![super::StageTelemetry {
            tasks: vec![task(None)],
            ..stages[0].clone()
        }];
        assert!(!super::distributed_scan_telemetry(&incomplete).1);
    }

    #[test]
    fn distributes_only_when_projection_preserves_merge_layout() {
        let supported = kaveon_sql::logical_plan::sql_to_logical_plan(
            "SELECT region, COUNT(*) FROM orders GROUP BY region",
        )
        .unwrap();
        let reordered = kaveon_sql::logical_plan::sql_to_logical_plan(
            "SELECT COUNT(*), region FROM orders GROUP BY region",
        )
        .unwrap();
        assert!(aggregate_merge_contract(&supported).is_some());
        assert!(aggregate_merge_contract(&reordered).is_none());
    }

    #[test]
    fn distributes_top_n_over_partitionable_scan_inputs() {
        let supported = kaveon_sql::logical_plan::sql_to_logical_plan(
            "SELECT region, total FROM orders WHERE total > 10 ORDER BY total DESC, region ASC LIMIT 5",
        )
        .unwrap();
        let no_limit = kaveon_sql::logical_plan::sql_to_logical_plan(
            "SELECT region, total FROM orders ORDER BY total DESC",
        )
        .unwrap();
        let (ordering, limit) = top_n_merge_contract(&supported).unwrap();
        assert_eq!(ordering.len(), 2);
        assert_eq!(limit, 5);
        assert!(top_n_merge_contract(&no_limit).is_none());
    }

    #[test]
    fn arrow_task_stream_round_trips_schema_and_batches() {
        let schema = Arc::new(Schema::new(vec![
            Field::new("region", DataType::Utf8, false),
            Field::new("total", DataType::Int64, false),
        ]));
        let batch = RecordBatch::try_new(
            schema.clone(),
            vec![
                Arc::new(StringArray::from(vec!["east", "west"])),
                Arc::new(Int64Array::from(vec![2, 3])),
            ],
        )
        .unwrap();
        let bytes = encode_arrow_stream(&schema, std::slice::from_ref(&batch)).unwrap();
        let (decoded_schema, decoded_batches) = decode_arrow_stream(&bytes).unwrap();
        assert_eq!(decoded_schema, schema);
        assert_eq!(decoded_batches, vec![batch]);
    }

    #[test]
    fn arrow_task_stream_compresses_repeated_exchange_values() {
        let schema = Arc::new(Schema::new(vec![Field::new(
            "region",
            DataType::Utf8,
            false,
        )]));
        let batch = RecordBatch::try_new(
            schema.clone(),
            vec![Arc::new(StringArray::from(vec!["east"; 100_000]))],
        )
        .unwrap();

        let bytes = encode_arrow_stream(&schema, std::slice::from_ref(&batch)).unwrap();

        assert!(bytes.len() < batch.get_array_memory_size());
        let (_, decoded) = decode_arrow_stream(&bytes).unwrap();
        assert_eq!(decoded, vec![batch]);
    }

    #[test]
    fn arrow_task_stream_preserves_empty_result_schema() {
        let schema = Arc::new(Schema::new(vec![Field::new(
            "region",
            DataType::Utf8,
            false,
        )]));
        let bytes = encode_arrow_stream(&schema, &[]).unwrap();
        let (decoded_schema, decoded_batches) = decode_arrow_stream(&bytes).unwrap();
        assert_eq!(decoded_schema, schema);
        assert!(decoded_batches.is_empty());
    }
}
