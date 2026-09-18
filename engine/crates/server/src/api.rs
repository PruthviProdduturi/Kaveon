use crate::AppState;
use crate::cluster::{NodeInfo, NodeRole};
use crate::lifecycle::{CancellationToken, TaskClaim, TaskOutcome, TaskOwner};
use crate::security::Identity;
use crate::settings::QuerySettings;
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
use kaveon_sql::logical_plan::sql_to_logical_plan_for_binder;
use kaveon_sql::logical_plan::{AggregateExpr, LogicalPlan};
use kaveon_sql::parser::{
    NativeTransactionalStatement, adapt_product_dml, parse_native_transactional,
};
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
use crate::planner::SourcePins;

struct QueryStore {
    queries: HashMap<String, QueryRecord>,
}

/// Where the query ran, and why, when it did not run on the workers.
#[derive(Clone, Serialize, PartialEq, Eq, Debug)]
struct ExecutionPlacement {
    /// `pending`, `distributed`, `coordinator` or `cache`.
    mode: &'static str,
    /// The distributed path taken (`fragments`, `aggregate`, `top_n`), the
    /// reason the coordinator ran it instead, or `hit` for a cached result.
    #[serde(skip_serializing_if = "Option::is_none")]
    detail: Option<String>,
}

impl ExecutionPlacement {
    fn pending() -> Self {
        Self {
            mode: "pending",
            detail: None,
        }
    }
    fn distributed(path: &str) -> Self {
        Self {
            mode: "distributed",
            detail: Some(path.to_owned()),
        }
    }
    fn coordinator(reason: Option<String>) -> Self {
        Self {
            mode: "coordinator",
            detail: Some(reason.unwrap_or_else(|| "shape has no distributed plan".to_owned())),
        }
    }
    /// Served from the coordinator's result cache: no worker work.
    fn cache() -> Self {
        Self {
            mode: "cache",
            detail: Some("hit".to_owned()),
        }
    }
}

#[derive(Clone, Serialize)]
struct QueryRecord {
    rows_are_preview: bool,
    scan_metrics_complete: bool,
    execution: ExecutionPlacement,
    /// What the statement set for itself; absent when it set nothing.
    #[serde(skip_serializing_if = "QuerySettings::is_default")]
    settings: QuerySettings,
    /// For a cache hit, the query whose result was served and what that
    /// query took to produce it.
    #[serde(skip_serializing_if = "Option::is_none")]
    cached_from: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    cached_elapsed_ms: Option<u64>,
    /// How long the statement waited for memory admission before it ran;
    /// zero when it was admitted on arrival. Not part of `elapsed_ms`,
    /// which starts at admission.
    admission_wait_ms: u64,
    /// Where a paged statement's first page is served, from the moment it
    /// runs: pages stream while the statement executes. Absent for inline
    /// delivery.
    #[serde(skip_serializing_if = "Option::is_none")]
    next_uri: Option<String>,
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
    compute_wall_us: u64,
    compute_queue_us: u64,
    admission_wait_us: u64,
    exchange_input_payloads: u64,
    exchange_input_bytes: u64,
    exchange_fetch_us: u64,
    exchange_decode_batches: u64,
    exchange_decode_bytes: u64,
    exchange_decode_us: u64,
    exchange_output_copies: u64,
    exchange_output_bytes: u64,
    exchange_hash_us: u64,
    exchange_copy_us: u64,
    exchange_copy_allocations: u64,
    exchange_copied_bytes: u64,
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
    spill_write_us: u64,
    spill_read_us: u64,
}

#[derive(Default)]
struct ExchangeDecodeMetrics {
    batches: AtomicU64,
    bytes: AtomicU64,
    elapsed_us: AtomicU64,
}

/// Counters emitted by a worker's storage readers, never derived from query output.
#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(default)]
struct TaskScanMetrics {
    files_considered: u64,
    files_opened: u64,
    decoded_batch_cache_hits: u64,
    decoded_batch_cache_misses: u64,
    decoded_batch_cache_evictions: u64,
    decoded_batch_cache_singleflight_waits: u64,
    row_groups_considered: u64,
    row_groups_selected: u64,
    rows_selected: u64,
    rows_emitted: u64,
    compressed_bytes_selected: u64,
    compressed_bytes_read: u64,
    row_filter_rows_examined: u64,
    row_filter_rows_admitted: u64,
    batches_emitted: u64,
    snapshot_ns: u64,
    footer_ns: u64,
    read_ns: u64,
    lanes: u64,
    lane_rows_min: u64,
    lane_rows_max: u64,
    lane_read_ns_min: u64,
    lane_read_ns_max: u64,
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
    #[serde(skip_serializing)]
    settings: QuerySettings,
}

#[derive(Clone, Serialize)]
struct ScanTelemetry {
    files_considered: u64,
    files_opened: u64,
    decoded_batch_cache_hits: u64,
    decoded_batch_cache_misses: u64,
    decoded_batch_cache_evictions: u64,
    decoded_batch_cache_singleflight_waits: u64,
    row_groups_considered: u64,
    row_groups_read: u64,
    row_groups_pruned: u64,
    rows_selected: u64,
    rows_emitted: u64,
    batches_emitted: u64,
    compressed_bytes_selected: u64,
    /// Compressed bytes the decoder read, against what the selected row
    /// groups hold: the difference is what late materialisation and the
    /// offset index left unread.
    compressed_bytes_read: u64,
    /// Rows a decoder-side row filter examined and admitted to the rest of
    /// the projection.
    row_filter_rows_examined: u64,
    row_filter_rows_admitted: u64,
    snapshot_ns: u64,
    footer_ns: u64,
    read_ns: u64,
    rows_per_second: f64,
    compressed_bytes_per_second: f64,
    /// Decoder lanes across every task, and the lightest and heaviest
    /// lane anywhere: the spread is the variance a scan carries.
    lanes: u64,
    lane_rows_min: u64,
    lane_rows_max: u64,
    lane_read_ns_min: u64,
    lane_read_ns_max: u64,
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
    /// Waiting for memory admission on the coordinator.
    Queued,
    Running,
    Finished,
    Failed,
    Canceled,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct ColumnInfo {
    pub(crate) name: String,
    #[serde(rename = "type")]
    pub(crate) data_type: String,
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
        .route("/v1/cache", delete(clear_result_cache))
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
        .route("/v1/whoami", get(whoami))
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
    /// Per-request settings; see `crate::settings`.
    #[serde(default)]
    settings: Option<serde_json::Map<String, serde_json::Value>>,
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
    /// The statement's settings: the task is admitted with the statement's
    /// memory limit and runs at its parallelism.
    #[serde(default)]
    settings: QuerySettings,
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
    // Finishing a query the coordinator gave up on must stop its tasks
    // here too; otherwise they keep the worker busy for nobody.
    if let Err(error) = state.lifecycle.cancellations.cancel(&query_id) {
        return lifecycle_error_response(error.to_string());
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
        req.settings.query_memory_limit_bytes(&state.config),
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
    if let Some(threads) = req.settings.local_parallelism
        && let Err(error) =
            kaveon_exec::local_parallel::set_query_parallelism(admitted.pool(), threads)
    {
        let message = error.to_string();
        let _ = owner.complete(TaskOutcome::Failed(Arc::from(message.clone())));
        return task_failure_response(StatusCode::BAD_REQUEST, &message);
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
    let mut plan = match sql_to_logical_plan_for_binder(req.query.trim().trim_end_matches(';')) {
        Ok(plan) => plan,
        Err(error) => {
            let message = error.to_string();
            let _ = owner.complete(TaskOutcome::Failed(Arc::from(message.clone())));
            return task_failure_response(StatusCode::BAD_REQUEST, &message);
        }
    };
    crate::planner::qualify_tables(&mut plan, &req.catalog, &req.schema);
    let plan = match kaveon_optim::binder::bind(plan, &state.catalog.read().await.manager) {
        Ok(plan) => plan,
        Err(error) => {
            let message = error.to_string();
            let _ = owner.complete(TaskOutcome::Failed(Arc::from(message.clone())));
            return task_failure_response(StatusCode::BAD_REQUEST, &message);
        }
    };
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
/// fault retries before any running task released memory. The task waits in
/// the worker's admission queue, in arrival order, until a running task
/// releases its budget or the query is cancelled; the coordinator's task
/// timeout bounds the wait. Only a full queue refuses.
async fn await_task_memory(
    admission: &MemoryAdmissionController,
    task_id: String,
    limit_bytes: u64,
    cancellation: &CancellationToken,
) -> Result<AdmittedQueryMemory, String> {
    if cancellation.is_cancelled() {
        return Err("query canceled while waiting for memory admission".into());
    }
    let wait = admission
        .admit_queued(task_id, limit_bytes)
        .map_err(|error| error.to_string())?;
    tokio::select! {
        admitted = wait => Ok(admitted),
        () = cancellation.cancelled() => {
            Err("query canceled while waiting for memory admission".into())
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

/// Chunks a lane may hold between the producer and its uploader: with
/// 4 MiB chunks, 32 MiB of back-pressure per destination.
const OUTPUT_LANE_CHUNKS: usize = 8;

/// One exchange output partition on its way to one destination.
struct OutputLane {
    identity: crate::exchange::ExchangeIdentity,
    sender: Option<tokio::sync::mpsc::Sender<crate::exchange::ExchangeChunk>>,
    upload: tokio::task::JoinHandle<crate::exchange::ExchangeResult<u64>>,
}

/// The executing thread's side of the lanes: a streaming IPC writer per
/// lane, created when the output schema is first seen, closed at the end.
struct StreamingOutputs {
    senders: Vec<(
        crate::exchange::ExchangeIdentity,
        tokio::sync::mpsc::Sender<crate::exchange::ExchangeChunk>,
    )>,
    writers: Vec<Option<crate::exchange::StreamingOutput>>,
    schema: Option<arrow::datatypes::SchemaRef>,
    encode_us: u64,
    lanes: usize,
}

struct FinishedOutputs {
    encode_us: u64,
    lanes: usize,
}

impl StreamingOutputs {
    fn new(
        senders: Vec<(
            crate::exchange::ExchangeIdentity,
            tokio::sync::mpsc::Sender<crate::exchange::ExchangeChunk>,
        )>,
    ) -> Self {
        let lanes = senders.len();
        Self {
            writers: (0..lanes).map(|_| None).collect(),
            senders,
            schema: None,
            encode_us: 0,
            lanes,
        }
    }

    fn open(&mut self, schema: &arrow::datatypes::SchemaRef) -> kaveon_core::Result<()> {
        if self.schema.is_some() {
            return Ok(());
        }
        for (lane, (identity, sender)) in self.senders.iter().enumerate() {
            self.writers[lane] = Some(
                crate::exchange::StreamingOutput::new(
                    identity.clone(),
                    schema,
                    sender.clone(),
                    crate::exchange::ExchangeLimits::default(),
                )
                .map_err(|error| kaveon_core::KaveonError::Execution(error.to_string()))?,
            );
        }
        self.schema = Some(Arc::clone(schema));
        Ok(())
    }

    fn write(
        &mut self,
        partition: usize,
        batch: &arrow::record_batch::RecordBatch,
    ) -> kaveon_core::Result<()> {
        let started = Instant::now();
        self.open(&batch.schema())?;
        for (lane, (identity, _)) in self.senders.iter().enumerate() {
            if identity.output_partition != partition {
                continue;
            }
            if let Some(writer) = self.writers[lane].as_mut() {
                writer.write(batch).map_err(|error| {
                    kaveon_core::KaveonError::Execution(format!(
                        "cannot stream exchange '{}': {error}",
                        identity.exchange_id.0
                    ))
                })?;
            }
        }
        self.encode_us = self.encode_us.saturating_add(elapsed_us(started));
        Ok(())
    }

    /// Close every lane. A lane that saw no batch still sends the output's
    /// schema and end marker, so the consumer finds a complete, empty
    /// stream of the right shape.
    fn finish(
        mut self,
        schema: &arrow::datatypes::SchemaRef,
    ) -> kaveon_core::Result<FinishedOutputs> {
        let started = Instant::now();
        self.open(schema)?;
        for (lane, writer) in self.writers.into_iter().enumerate() {
            if let Some(writer) = writer {
                writer.finish().map_err(|error| {
                    kaveon_core::KaveonError::Execution(format!(
                        "cannot finish exchange '{}': {error}",
                        self.senders[lane].0.exchange_id.0
                    ))
                })?;
            }
        }
        Ok(FinishedOutputs {
            encode_us: self.encode_us.saturating_add(elapsed_us(started)),
            lanes: self.lanes,
        })
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

/// A spooled exchange payload read one IPC batch at a time. The spool is a
/// file behind a buffered reader, so what the task holds in memory is the
/// batch it last returned, not the payload: that batch is what is reserved,
/// released when the next one replaces it. (Reserving the payload's size
/// charged three producers' spools at once against the budget of a task
/// that never held them — the final stage of a 100 M-group aggregate was
/// refused for memory it did not use.)
///
/// What is reserved is what the batch's rows occupy: an IPC batch is one
/// message body that every column's buffers point into, and its Arrow
/// memory size counts that body once per buffer — four times over for the
/// two-column grouped-state batch.
///
/// A batch whose reservation the budget refuses is kept: the refusal is
/// reported, and the next call offers the same batch again, its
/// reservation tried first — the IPC reader has moved on, so nothing else
/// could bring the batch back. That is what lets the pump ask the merge
/// threads for memory and try again (`local_parallel::ThreadSource`).
struct DiskExchangeInput {
    schema: arrow::datatypes::SchemaRef,
    payloads: std::collections::VecDeque<crate::transport::ArrowPayload>,
    memory: kaveon_core::OperatorMemoryAccount,
    /// The batch handed out last, held until the next call, when read as
    /// a `BatchOperator`; a thread source hands the reservation over with
    /// the batch instead.
    decoded: Option<kaveon_core::MemoryReservation>,
    /// A decoded batch whose reservation was refused, with its size,
    /// offered again on the next call.
    pending: Option<(arrow::record_batch::RecordBatch, u64)>,
    metrics: Arc<ExchangeDecodeMetrics>,
}
impl DiskExchangeInput {
    fn new(
        schema: arrow::datatypes::SchemaRef,
        payloads: std::collections::VecDeque<crate::transport::ArrowPayload>,
        memory: kaveon_core::OperatorMemoryAccount,
        metrics: Arc<ExchangeDecodeMetrics>,
    ) -> Self {
        Self {
            schema,
            payloads,
            memory,
            decoded: None,
            pending: None,
            metrics,
        }
    }

    /// The next batch with the reservation holding it: the pending batch
    /// first, then the spool's next.
    fn decode_next(
        &mut self,
    ) -> kaveon_core::Result<
        Option<(
            arrow::record_batch::RecordBatch,
            Option<kaveon_core::MemoryReservation>,
        )>,
    > {
        self.memory.check_cancelled()?;
        let (batch, bytes) = match self.pending.take() {
            Some(pending) => pending,
            None => {
                let Some(batch) = self.decode()? else {
                    return Ok(None);
                };
                let bytes = kaveon_exec::local_parallel::occupied_bytes(&batch)?;
                (batch, bytes)
            }
        };
        let reservation = if bytes > 0 {
            match self.memory.reserve(bytes) {
                Ok(reservation) => Some(reservation),
                Err(error) => {
                    self.pending = Some((batch, bytes));
                    return Err(error);
                }
            }
        } else {
            None
        };
        Ok(Some((batch, reservation)))
    }

    /// The spool's next batch, counted once.
    fn decode(&mut self) -> kaveon_core::Result<Option<arrow::record_batch::RecordBatch>> {
        while let Some(payload) = self.payloads.front_mut() {
            let decode_started = Instant::now();
            if let Some(batch) = payload
                .next_batch()
                .map_err(kaveon_core::KaveonError::Execution)?
            {
                self.metrics
                    .elapsed_us
                    .fetch_add(elapsed_us(decode_started), Ordering::AcqRel);
                self.metrics.batches.fetch_add(1, Ordering::AcqRel);
                self.metrics.bytes.fetch_add(
                    kaveon_exec::local_parallel::occupied_bytes(&batch)?,
                    Ordering::AcqRel,
                );
                return Ok(Some(batch));
            }
            self.payloads.pop_front();
        }
        Ok(None)
    }
}
impl kaveon_core::BatchOperator for DiskExchangeInput {
    fn schema(&self) -> &arrow::datatypes::SchemaRef {
        &self.schema
    }
    fn next_batch(&mut self) -> kaveon_core::Result<Option<arrow::record_batch::RecordBatch>> {
        self.decoded = None;
        let Some((batch, reservation)) = self.decode_next()? else {
            return Ok(None);
        };
        self.decoded = reservation;
        Ok(Some(batch))
    }
}
impl kaveon_exec::local_parallel::ThreadSource for DiskExchangeInput {
    fn schema(&self) -> &arrow::datatypes::SchemaRef {
        &self.schema
    }
    fn next_batch(
        &mut self,
    ) -> kaveon_core::Result<Option<kaveon_exec::local_parallel::ReservedBatch>> {
        Ok(self
            .decode_next()?
            .map(|(batch, memory)| kaveon_exec::local_parallel::ReservedBatch { batch, memory }))
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
        Ok(Box::new(DiskExchangeInput::new(
            schema,
            payloads,
            self.memory.clone(),
            Arc::clone(&self.decode_metrics),
        )))
    }

    /// One source per producer payload, each decoding its spool on the
    /// thread that reads it.
    fn open_each(
        &self,
        exchange_id: &ExchangeId,
    ) -> kaveon_core::Result<Option<kaveon_exec::local_parallel::Sources>> {
        let inputs = self.inputs.get(exchange_id).ok_or_else(|| {
            kaveon_core::KaveonError::Execution(format!("missing exchange {}", exchange_id.0))
        })?;
        let schema = inputs
            .first()
            .ok_or_else(|| {
                kaveon_core::KaveonError::Execution("empty exchange payload set".into())
            })?
            .schema();
        let openers = inputs
            .iter()
            .map(|payload| {
                let payload = payload
                    .fork()
                    .map_err(kaveon_core::KaveonError::Execution)?;
                let schema = payload.schema();
                let memory = self.memory.clone();
                let metrics = Arc::clone(&self.decode_metrics);
                Ok(Box::new(move || {
                    Ok(Box::new(DiskExchangeInput::new(
                        schema,
                        std::collections::VecDeque::from([payload]),
                        memory,
                        metrics,
                    ))
                        as Box<dyn kaveon_exec::local_parallel::ThreadSource>)
                })
                    as kaveon_exec::local_parallel::SourceOpener)
            })
            .collect::<kaveon_core::Result<Vec<_>>>()?;
        Ok(Some(kaveon_exec::local_parallel::Sources::Threads {
            schema,
            openers,
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
    // Exchange output streams to its destinations while the fragment runs:
    // one lane (a bounded chunk channel and an uploader) per output
    // partition and destination, so the task never holds its whole output.
    let mut lanes: Vec<OutputLane> = Vec::new();
    for location in &req.exchange_outputs {
        if location.producer.query_id != req.query_id
            || location.producer.stage_id != StageId(req.stage_id)
            || location.producer.partition != requested_partition_index(req)
            || location.producer.attempt != req.attempt
        {
            return Err(format!(
                "exchange '{}' destination declares a producer that does not match this task",
                location.exchange_id.0
            ));
        }
        let (sender, receiver) = tokio::sync::mpsc::channel(OUTPUT_LANE_CHUNKS);
        let upload = tokio::spawn(crate::exchange::upload_chunk_stream(
            client.clone(),
            location.worker_uri.clone(),
            token.to_owned(),
            receiver,
            crate::exchange::ExchangeLimits::default(),
        ));
        lanes.push(OutputLane {
            identity: crate::exchange::ExchangeIdentity {
                exchange_id: location.exchange_id.clone(),
                task_id: location.producer.clone(),
                output_partition: location.output_partition,
            },
            sender: Some(sender),
            upload,
        });
    }
    let lane_senders = lanes
        .iter_mut()
        .map(|lane| {
            (
                lane.identity.clone(),
                lane.sender.take().expect("lane sender"),
            )
        })
        .collect::<Vec<_>>();
    let execution_state = Arc::clone(state);
    let execution_fragment = fragment.clone();
    let execution_memory = memory.clone();
    let worker_decode_metrics = Arc::clone(&decode_metrics);
    let compute_started = Instant::now();
    let execution = tokio::task::spawn_blocking(move || {
        let queue_us = elapsed_us(compute_started);
        let wall_started = Instant::now();
        let cpu_started = thread_cpu_us();
        let catalog = execution_state.catalog.blocking_read();
        let mut outputs = StreamingOutputs::new(lane_senders);
        let mut sink = |partition: usize, batch: &arrow::record_batch::RecordBatch| {
            outputs.write(partition, batch)
        };
        let result = crate::fragment_exec::execute_fragment_streaming(
            &execution_fragment,
            &catalog,
            &PrefetchedExchangeInputs {
                inputs,
                memory: input_account,
                decode_metrics: worker_decode_metrics,
            },
            partition,
            Some(&execution_memory),
            &mut sink,
        )
        .map_err(|error| error.to_string());
        let outputs = match &result {
            Ok(execution) => outputs
                .finish(&execution.result_schema)
                .map_err(|error| error.to_string()),
            Err(error) => Err(error.clone()),
        };
        let cpu_us =
            cpu_started.and_then(|started| thread_cpu_us().map(|end| end.saturating_sub(started)));
        (result, outputs, cpu_us, queue_us, elapsed_us(wall_started))
    })
    .await
    .map_err(|error| format!("fragment execution task failed: {error}"))?;
    let (execution, outputs, compute_cpu_us, compute_queue_us, compute_wall_us) = execution;
    // The uploaders finish once their channels close; a failed upload is a
    // failed task whatever the fragment returned.
    let upload_started = Instant::now();
    let mut uploaded = 0u64;
    for lane in lanes {
        let bytes = lane
            .upload
            .await
            .map_err(|error| format!("exchange uploader failed: {error}"))?
            .map_err(|error| {
                format!(
                    "cannot upload exchange '{}': {error}",
                    lane.identity.exchange_id.0
                )
            })?;
        uploaded = uploaded.saturating_add(bytes);
    }
    let execution = execution?;
    let outputs = outputs?;
    metrics.exchange_encode_us = outputs.encode_us;
    metrics.exchange_output_copies = outputs.lanes as u64;
    metrics.exchange_output_bytes = uploaded;
    metrics.exchange_upload_us = elapsed_us(upload_started);
    metrics.compute_cpu_us = compute_cpu_us;
    metrics.compute_queue_us = compute_queue_us;
    metrics.compute_wall_us = compute_wall_us;
    metrics.exchange_decode_batches = decode_metrics.batches.load(Ordering::Acquire);
    metrics.exchange_decode_bytes = decode_metrics.bytes.load(Ordering::Acquire);
    metrics.exchange_decode_us = decode_metrics.elapsed_us.load(Ordering::Acquire);
    metrics.exchange_hash_us = execution.hash_partition_metrics.hash_us;
    metrics.exchange_copy_us = execution.hash_partition_metrics.copy_us;
    metrics.exchange_copy_allocations = execution.hash_partition_metrics.copy_allocations;
    metrics.exchange_copied_bytes = execution.hash_partition_metrics.copied_bytes;
    for (exchange_id, output) in &execution.exchange_outputs {
        for output_partition in 0..output.partitions.len() {
            if !req.exchange_outputs.iter().any(|location| {
                &location.exchange_id == exchange_id
                    && location.output_partition == output_partition
            }) {
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
        metrics.spill_write_us = after.write_us.saturating_sub(before.write_us);
        metrics.spill_read_us = after.read_us.saturating_sub(before.read_us);
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

/// The refusal of a statement that could not be admitted: on arrival, from
/// a full queue, or after its wait expired. `admission_wait_ms` is how long
/// it waited before the refusal.
fn admission_rejected_response(error: String, admission_wait_ms: u64) -> Response {
    (
        StatusCode::TOO_MANY_REQUESTS,
        Json(serde_json::json!({
            "error": error,
            "code": "MEMORY_ADMISSION_REJECTED",
            "admission_wait_ms": admission_wait_ms
        })),
    )
        .into_response()
}

/// The record's `next_uri` for a paged statement: its first page, served
/// while the statement runs. `None` for inline delivery.
fn paged_next_uri(query_id: &str, context: &QueryContext) -> Option<String> {
    (context.result_delivery.as_deref() == Some("paged"))
        .then(|| format!("/v1/query/{query_id}/results/0"))
}

/// A statement's record before it has produced anything: queued for
/// admission, or admitted and running.
fn pending_query_record(
    query_id: &str,
    sql: &str,
    settings: &QuerySettings,
    submitted_at_ms: u64,
    context: &QueryContext,
    state: QueryState,
    admission_wait_ms: u64,
) -> QueryRecord {
    QueryRecord {
        rows_are_preview: true,
        scan_metrics_complete: false,
        execution: ExecutionPlacement::pending(),
        settings: settings.clone(),
        cached_from: None,
        cached_elapsed_ms: None,
        admission_wait_ms,
        next_uri: matches!(state, QueryState::Running)
            .then(|| paged_next_uri(query_id, context))
            .flatten(),
        id: query_id.to_owned(),
        sql: sql.to_owned(),
        state,
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
    }
}

async fn submit_statement(
    State(state): State<Arc<AppState>>,
    Extension(identity): Extension<Identity>,
    Json(req): Json<StatementRequest>,
) -> impl IntoResponse {
    run_statement(state, identity, req, Uuid::new_v4().to_string()).await
}

/// Runs one statement under `query_id` exactly as `POST /v1/statement`
/// does — settings, admission, the record, cancellation, planning and
/// execution — and answers with the response the client would get. The
/// statement path calls it for the statements it runs on behalf of another
/// (`ANALYZE … WITH (distinct = true)`), so those go through the same
/// machinery as a client statement under an id their caller knows.
async fn run_statement(
    state: Arc<AppState>,
    identity: Identity,
    req: StatementRequest,
    query_id: String,
) -> Response {
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
    // Settings first: a refused setting is a 400 before any permit is held.
    let (settings, sql, time_zone) = match request_settings(&req, &state.config) {
        Ok(parsed) => parsed,
        Err(error) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({
                    "error": error.to_string(),
                    "code": "INVALID_SETTING"
                })),
            )
                .into_response();
        }
    };
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
    if let Some((status, body)) =
        transaction_api_guidance(&sql, state.product_transactions.catalog().is_some())
    {
        return (status, Json(body)).into_response();
    }
    let submitted_at_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64;
    // `SHOW STATS FOR` and `DESCRIBE DETAIL` are the statistics statements;
    // they need the session context a catalog statement does not, and the
    // catalog parser must not read `DESCRIBE DETAIL t` as `DESCRIBE detail`.
    let statistics_statement = parse_statistics_statement(&sql);
    // A catalog statement is recognised before the session context is
    // checked: `CREATE CATALOG` on an empty coordinator, or `CREATE SCHEMA`
    // in a catalog with no schema yet, has no valid context to validate.
    let catalog_statement = if statistics_statement.is_some() {
        None
    } else {
        match kaveon_sql::ddl::parse_catalog_statement(&sql) {
            Ok(statement) => statement,
            Err(error) => {
                return (
                    StatusCode::BAD_REQUEST,
                    Json(serde_json::json!({
                        "error": format!("SQL parse error: {error}"),
                        "code": "SYNTAX_ERROR"
                    })),
                )
                    .into_response();
            }
        }
    };
    // Pin one immutable catalog manager for validation, optimization and
    // physical planning. Publishing a newer manager swaps the outer Arc and
    // cannot change the definitions observed by this query.
    let catalog_snapshot = state.catalog.read().await.clone();
    let requested_catalog = req
        .catalog
        .as_deref()
        .unwrap_or_else(|| catalog_snapshot.default_catalog());
    if catalog_statement.is_none() && catalog_snapshot.catalog(requested_catalog).is_none() {
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
        let selected_catalog = catalog.catalog(catalog_name);
        if catalog_statement.is_none() && selected_catalog.is_none() {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({
                    "error": format!("catalog '{catalog_name}' not found"),
                    "code": "CATALOG_NOT_FOUND"
                })),
            )
                .into_response();
        }
        if catalog_statement.is_none()
            && !selected_catalog.is_some_and(|selected| {
                selected
                    .schema_names()
                    .iter()
                    .any(|name| name == schema_name)
            })
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
            time_zone,
            client_address: None,
            client_tags: req.client_tags,
            result_delivery: req.result_delivery,
            catalog_snapshot_id,
            settings: settings.clone(),
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

    // Memory admission: on arrival when the budget fits, else queued until
    // it does, the statement asked not to wait, the wait expired, or the
    // statement was cancelled. A queued statement is in the history as
    // QUEUED so that it can be seen and cancelled by ID.
    let admission_started = Instant::now();
    let admission_wait = settings.admission_wait(&state.config);
    let query_limit_bytes = settings.query_memory_limit_bytes(&state.config);
    let query_memory = if admission_wait.is_zero() {
        match state
            .memory_admission
            .admit(query_id.clone(), query_limit_bytes)
        {
            Ok(memory) => memory,
            Err(error) => return admission_rejected_response(error.to_string(), 0),
        }
    } else {
        let mut wait = match state
            .memory_admission
            .admit_queued(query_id.clone(), query_limit_bytes)
        {
            Ok(wait) => wait,
            Err(error) => return admission_rejected_response(error.to_string(), 0),
        };
        if !wait.admitted_immediately() {
            QUERY_STORE.write().await.queries.insert(
                query_id.clone(),
                pending_query_record(
                    &query_id,
                    &sql,
                    &settings,
                    submitted_at_ms,
                    &context,
                    QueryState::Queued,
                    0,
                ),
            );
        }
        tokio::select! {
            memory = &mut wait => memory,
            () = cancellation.cancelled() => {
                // `cancel_query` has already marked the record.
                drop(wait);
                return canceled_task_response();
            }
            () = tokio::time::sleep(admission_wait) => {
                match wait.expire() {
                    Some(memory) => memory,
                    None => {
                        let waited_ms = elapsed_ms(admission_started);
                        let stats = state.memory_admission.stats();
                        let message = format!(
                            "memory admission wait of {} s expired: {} of {} bytes admitted, {} statements waiting",
                            admission_wait.as_secs(),
                            stats.admitted_bytes,
                            stats.limit_bytes,
                            stats.queue_depth
                        );
                        if let Some(record) = QUERY_STORE.write().await.queries.get_mut(&query_id)
                            && matches!(record.state, QueryState::Queued)
                        {
                            record.state = QueryState::Failed;
                            record.error = Some(message.clone());
                            record.admission_wait_ms = waited_ms;
                            record.completed_at_ms = unix_time_ms();
                        }
                        return admission_rejected_response(message, waited_ms);
                    }
                }
            }
        }
    };
    let admission_wait_ms = elapsed_ms(admission_started);
    let start = Instant::now();
    let memory_cancellation = cancellation.clone();
    if let Err(error) = query_memory
        .pool()
        .set_cancellation_probe(move || memory_cancellation.is_cancelled())
    {
        return lifecycle_error_response(error.to_string());
    }
    if let Some(threads) = settings.local_parallelism
        && let Err(error) =
            kaveon_exec::local_parallel::set_query_parallelism(query_memory.pool(), threads)
    {
        return lifecycle_error_response(error.to_string());
    }

    let mut result_writer = {
        // A cancellation that landed while the statement was queued keeps
        // its record; the same lock `cancel_query` takes, so neither side
        // overwrites the other.
        let mut store = QUERY_STORE.write().await;
        if cancellation.is_cancelled()
            || store
                .queries
                .get(&query_id)
                .is_some_and(|record| matches!(record.state, QueryState::Canceled))
        {
            drop(store);
            return canceled_task_response();
        }
        // A paged statement's pages are registered under the same lock that
        // makes its record RUNNING with `next_uri`: a client following the
        // link is told to wait (202) until the first page lands, never told
        // the result is unknown, and the pages stream while the statement
        // runs. Every early return below drops the writer, which turns the
        // entry into a `410 Gone` tombstone.
        let result_writer = if paged {
            match state.results.begin(&query_id, &identity.principal) {
                Ok(writer) => Some(writer),
                Err(error) => {
                    let mut record = pending_query_record(
                        &query_id,
                        &sql,
                        &settings,
                        submitted_at_ms,
                        &context,
                        QueryState::Failed,
                        admission_wait_ms,
                    );
                    record.error = Some(error.to_string());
                    record.completed_at_ms = unix_time_ms();
                    store.queries.insert(query_id.clone(), record);
                    drop(store);
                    return task_failure_response(
                        StatusCode::INSUFFICIENT_STORAGE,
                        "result disk quota or write failure",
                    );
                }
            }
        } else {
            None
        };
        store.queries.insert(
            query_id.clone(),
            pending_query_record(
                &query_id,
                &sql,
                &settings,
                submitted_at_ms,
                &context,
                QueryState::Running,
                admission_wait_ms,
            ),
        );
        result_writer
    };

    // Catalog and ANALYZE statements answer inline whatever the delivery;
    // their finished record drops `next_uri` and the writer with it.
    if let Some(statement) = catalog_statement {
        return execute_catalog(&state, &identity, &query_id, &context, statement, start).await;
    }
    if let Some(statement) = parse_analyze_statement(&sql) {
        // ANALYZE runs no operator of its own, and the statements it runs
        // for its distinct counts are admitted in their own right — memory,
        // principal and resource group — so a single-slot coordinator does
        // not wait on itself.
        drop(query_memory);
        drop(_group_permit);
        drop(_principal_permit);
        return execute_analyze(&state, &identity, &query_id, &context, statement, start).await;
    }
    if let Some(statement) = statistics_statement {
        return execute_statistics_statement(&state, &query_id, &context, statement, start).await;
    }

    let analysis_start = Instant::now();
    let mut plan = match sql_to_logical_plan_for_binder(&sql) {
        Ok(p) => p,
        Err(e) => {
            let message = format!("SQL parse error: {e}");
            finish_failed_query(&query_id, message.clone(), start, None, None, None).await;
            let mut body = serde_json::json!({
                "error": message,
                "code": "SYNTAX_ERROR"
            });
            if let Some((line, column)) = parse_error_position(&message) {
                body["position"] = serde_json::json!({ "line": line, "column": column });
            }
            return (StatusCode::BAD_REQUEST, Json(body)).into_response();
        }
    };
    crate::planner::qualify_tables(&mut plan, &context.catalog, &context.schema);
    let plan = match kaveon_optim::binder::bind(plan, &catalog_snapshot) {
        Ok(plan) => plan,
        Err(error) => {
            let message = format!("SQL analysis error: {error}");
            finish_failed_query(&query_id, message.clone(), start, None, None, None).await;
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({
                    "error": message,
                    "code": "ANALYSIS_ERROR"
                })),
            )
                .into_response();
        }
    };
    let analysis_us = elapsed_us(analysis_start);
    let logical_plan = crate::planner::logical_plan_tree(&plan);
    let plan = kaveon_optim::rules::push_filter_down(plan);
    let plan = kaveon_optim::rules::push_projection_down(plan);
    let (plan, planning_source_pins) =
        optimize_with_durable_statistics(&state, plan, &catalog_snapshot).await;
    let optimized_plan = crate::planner::optimized_plan_tree(&plan);
    let physical_plan = crate::planner::physical_plan_tree(&plan);
    if let Some(record) = QUERY_STORE.write().await.queries.get_mut(&query_id) {
        record.timings.analysis_us = Some(analysis_us);
        record.plan.logical = Some(logical_plan.clone());
        record.plan.optimized = Some(optimized_plan.clone());
        record.plan.physical = Some(physical_plan.clone());
    }

    // The result cache: a complete result of this statement under this
    // catalog snapshot, these pinned versions and this time zone is served
    // without worker work. Bypassed by `settings.result_cache = false`.
    let cache_key = (settings.result_cache_enabled() && state.result_cache.enabled()).then(|| {
        crate::result_cache::ResultCacheKey::new(
            &sql,
            &context.catalog,
            &context.schema,
            &context.catalog_snapshot_id,
            &planning_source_pins.delta_versions,
            context.time_zone.as_deref(),
        )
    });
    if let Some(hit) = cache_key
        .as_ref()
        .and_then(|key| state.result_cache.get(key))
    {
        let mut data = (*hit.rows).clone();
        let next_uri = if paged {
            match spool_rows(&state, &query_id, result_writer.take(), &mut data) {
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
        let record = QueryRecord {
            rows_are_preview: true,
            scan_metrics_complete: true,
            execution: ExecutionPlacement::cache(),
            settings: settings.clone(),
            cached_from: Some(hit.query_id.clone()),
            cached_elapsed_ms: Some(hit.elapsed_ms),
            admission_wait_ms,
            next_uri: paged_next_uri(&query_id, &context),
            id: query_id.clone(),
            sql,
            state: QueryState::Finished,
            columns: hit.columns.clone(),
            rows: history_preview(&data),
            error: None,
            elapsed_ms: elapsed,
            submitted_at_ms,
            completed_at_ms: unix_time_ms(),
            timings: QueryTimings {
                analysis_us: Some(analysis_us),
                planning_us: None,
                execution_us: None,
                result_serialization_us: None,
            },
            plan: QueryPlan {
                logical: Some(logical_plan),
                optimized: Some(optimized_plan),
                physical: Some(physical_plan),
            },
            scans: vec![],
            stages: vec![],
            context,
        };
        if !commit_query_record(record).await {
            state.results.remove(&query_id);
            return canceled_task_response();
        }
        return Json(StatementResponse {
            next_uri,
            id: query_id,
            state: QueryState::Finished,
            columns: Some(hit.columns.clone()),
            data: Some(data),
            error: None,
            elapsed_ms: elapsed,
        })
        .into_response();
    }

    // Why the coordinator ran it, when it did: surfaced on the record so a
    // downgrade is never silent.
    let mut placement_reason: Option<String> = None;
    if let Some(distributed) = execute_distributed_fragments(
        &state,
        &query_id,
        &context,
        &plan,
        &catalog_snapshot,
        &planning_source_pins,
        DistributedSink {
            placement_reason: &mut placement_reason,
            result_writer: &mut result_writer,
        },
    )
    .await
    {
        match distributed {
            Ok((result, stages, planning_us)) => {
                let mut result = result;
                keep_result(&state, &cache_key, &result, start, &query_id, paged);
                let next_uri = if paged {
                    match spool_rows(&state, &query_id, result_writer.take(), &mut result.data) {
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
                    execution: ExecutionPlacement::distributed("fragments"),
                    settings: settings.clone(),
                    cached_from: None,
                    cached_elapsed_ms: None,
                    admission_wait_ms,
                    next_uri: paged_next_uri(&query_id, &context),
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
                keep_result(&state, &cache_key, &result, start, &query_id, paged);
                let next_uri = if paged {
                    match spool_rows(&state, &query_id, result_writer.take(), &mut result.data) {
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
                    execution: ExecutionPlacement::distributed("aggregate"),
                    settings: settings.clone(),
                    cached_from: None,
                    cached_elapsed_ms: None,
                    admission_wait_ms,
                    next_uri: paged_next_uri(&query_id, &context),
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
                keep_result(&state, &cache_key, &result, start, &query_id, paged);
                let next_uri = if paged {
                    match spool_rows(&state, &query_id, result_writer.take(), &mut result.data) {
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
                    execution: ExecutionPlacement::distributed("top_n"),
                    settings: settings.clone(),
                    cached_from: None,
                    cached_elapsed_ms: None,
                    admission_wait_ms,
                    next_uri: paged_next_uri(&query_id, &context),
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

    let local_catalog_snapshot = Arc::clone(&catalog_snapshot);
    let local_query_id = query_id.clone();
    // Build non-Send operators inside the blocking task. Retain admission until
    // both execution and result publication complete, even if the HTTP future drops.
    let local_execution = tokio::task::spawn_blocking(move || {
        let mut local_columns = Vec::new();
        let planned_execution = {
            let planning_start = Instant::now();
            crate::planner::plan_query_with_pins(
                &plan,
                &local_catalog_snapshot,
                query_memory.pool(),
                &planning_source_pins,
            )
            .map(|planned| {
                let planning_us = elapsed_us(planning_start);
                let scan_handles = planned.scan_metrics;
                let mut operator = planned.operator;
                local_columns = column_infos(operator.schema());
                if result_writer.is_some()
                    && let Some(record) = QUERY_STORE
                        .blocking_write()
                        .queries
                        .get_mut(&local_query_id)
                    && matches!(record.state, QueryState::Running)
                {
                    record.columns = local_columns.clone();
                }
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
                execution: ExecutionPlacement::coordinator(placement_reason.clone()),
                settings: settings.clone(),
                cached_from: None,
                cached_elapsed_ms: None,
                admission_wait_ms,
                next_uri: paged_next_uri(&query_id, &context),
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
                data_type: presented_type(f.data_type()),
            })
            .collect()
    } else {
        vec![]
    };

    let serialization_start = Instant::now();
    let rows = batches_to_json(&batches);
    if let Some(key) = &cache_key
        && !paged
    {
        state.result_cache.insert(
            key.clone(),
            &columns,
            &rows,
            start.elapsed().as_millis() as u64,
            &query_id,
        );
    }
    let next_uri = if let Some(writer) = result_writer {
        if state.results.publish(&query_id, writer).is_err() {
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
        execution: ExecutionPlacement::coordinator(placement_reason.clone()),
        settings: settings.clone(),
        cached_from: None,
        cached_elapsed_ms: None,
        admission_wait_ms,
        next_uri: paged_next_uri(&query_id, &context),
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

/// The record's columns while it still runs, so a paged reader has a header
/// for the pages it can already read. A record past RUNNING is left alone.
async fn publish_columns(query_id: &str, columns: &[ColumnInfo]) {
    if let Some(record) = QUERY_STORE.write().await.queries.get_mut(query_id)
        && matches!(record.state, QueryState::Running)
    {
        record.columns = columns.to_vec();
    }
}

fn column_infos(schema: &arrow::datatypes::Schema) -> Vec<ColumnInfo> {
    schema
        .fields()
        .iter()
        .map(|field| ColumnInfo {
            name: field.name().clone(),
            data_type: presented_type(field.data_type()),
        })
        .collect()
}

/// Keeps a finished distributed result in the cache, when the statement
/// allowed it. Elapsed is measured at this point: what it took to produce
/// the rows, before any paging. A paged statement's rows went to the page
/// store, not `result.data`, so it is never kept — the coordinator-local
/// path makes the same choice.
fn keep_result(
    state: &AppState,
    cache_key: &Option<crate::result_cache::ResultCacheKey>,
    result: &TaskResponse,
    start: Instant,
    query_id: &str,
    paged: bool,
) {
    if paged {
        return;
    }
    if let Some(key) = cache_key {
        state.result_cache.insert(
            key.clone(),
            &result.columns,
            &result.data,
            start.elapsed().as_millis() as u64,
            query_id,
        );
    }
}

/// `DELETE /v1/cache`: an administrator drops every cached result.
async fn clear_result_cache(
    State(state): State<Arc<AppState>>,
    Extension(identity): Extension<Identity>,
) -> Response {
    if identity.role != crate::security::Role::Admin {
        return (
            StatusCode::FORBIDDEN,
            Json(serde_json::json!({"error": "clearing the result cache requires admin role", "code": "FORBIDDEN"})),
        )
            .into_response();
    }
    let (entries, bytes) = state.result_cache.clear();
    Json(serde_json::json!({
        "cleared_entries": entries,
        "cleared_bytes": bytes,
        "result_cache": state.result_cache.stats(),
    }))
    .into_response()
}

/// The statement's settings, its SQL with any `SET SESSION` prefix removed,
/// and its time zone (the request's field, or the prefix's assignment; both
/// must agree when both are given).
fn request_settings(
    req: &StatementRequest,
    config: &crate::config::ServerConfig,
) -> Result<(QuerySettings, String, Option<String>), crate::settings::SettingsError> {
    let prefix = crate::settings::split_session_prefix(&req.query)?;
    let mut settings = req.settings.clone().unwrap_or_default();
    let session_time_zone = crate::settings::merge_session_prefix(&mut settings, &prefix)?;
    let time_zone = match (&req.time_zone, session_time_zone) {
        (Some(field), Some(session)) if *field != session => {
            return Err(crate::settings::SettingsError(
                "time_zone is given twice with different values".into(),
            ));
        }
        (field, session) => session.or_else(|| field.clone()),
    };
    let settings = QuerySettings::from_request(&settings, config)?;
    let sql = prefix.statement.trim().trim_end_matches(';').to_owned();
    Ok((settings, sql, time_zone))
}

/// Which columns `ANALYZE` counts distinct values for: none (the metadata
/// profile only), every column, or the columns named.
#[derive(Clone, Debug, PartialEq, Eq)]
enum DistinctColumns {
    None,
    All,
    Named(Vec<String>),
}

/// `ANALYZE [catalog.][schema.]table [WITH (distinct = true | columns =
/// ARRAY['a', 'b'])]`, parsed.
#[derive(Clone, Debug, PartialEq, Eq)]
struct AnalyzeStatement {
    table: String,
    distinct: DistinctColumns,
}

/// `None` when the statement is not an `ANALYZE`; `Err` with the reason for
/// an `ANALYZE` whose table name or `WITH` properties are malformed.
fn parse_analyze_statement(sql: &str) -> Option<Result<AnalyzeStatement, String>> {
    let sql = sql.trim();
    let rest = sql
        .get(..7)
        .filter(|word| word.eq_ignore_ascii_case("ANALYZE"))
        .and_then(|_| sql.get(7..))
        .filter(|rest| rest.is_empty() || rest.starts_with(|c: char| c.is_ascii_whitespace()))?;
    Some(parse_analyze_body(rest.trim()))
}

fn parse_analyze_body(body: &str) -> Result<AnalyzeStatement, String> {
    let name_end = body
        .find(|c: char| c.is_ascii_whitespace() || c == '(')
        .unwrap_or(body.len());
    let (name, tail) = body.split_at(name_end);
    let table = bounded_table_name(name).ok_or_else(|| {
        "ANALYZE takes [catalog.][schema.]table of plain or double-quoted identifier parts"
            .to_owned()
    })?;
    let tail = tail.trim();
    if tail.is_empty() {
        return Ok(AnalyzeStatement {
            table,
            distinct: DistinctColumns::None,
        });
    }
    let properties = tail
        .get(..4)
        .filter(|word| word.eq_ignore_ascii_case("WITH"))
        .and_then(|_| tail.get(4..))
        .map(str::trim_start)
        .filter(|rest| rest.starts_with('('))
        .and_then(|rest| rest.strip_prefix('('))
        .and_then(|rest| rest.trim_end().strip_suffix(')'))
        .ok_or_else(|| {
            "ANALYZE accepts WITH (distinct = true) or WITH (columns = ARRAY['a', 'b']) after the table name".to_owned()
        })?;
    let mut distinct = None;
    let mut columns = None;
    for entry in split_property_entries(properties)? {
        let (key, value) = entry
            .split_once('=')
            .map(|(key, value)| (key.trim(), value.trim()))
            .ok_or_else(|| format!("ANALYZE property '{}' needs key = value", entry.trim()))?;
        if key.eq_ignore_ascii_case("distinct") {
            if distinct.is_some() {
                return Err("ANALYZE property distinct is given twice".into());
            }
            distinct = Some(match value {
                v if v.eq_ignore_ascii_case("true") => true,
                v if v.eq_ignore_ascii_case("false") => false,
                other => {
                    return Err(format!(
                        "ANALYZE property distinct must be true or false, not {other}"
                    ));
                }
            });
        } else if key.eq_ignore_ascii_case("columns") {
            if columns.is_some() {
                return Err("ANALYZE property columns is given twice".into());
            }
            columns = Some(parse_column_array(value)?);
        } else {
            return Err(format!(
                "unknown ANALYZE property '{key}'; the properties are distinct and columns"
            ));
        }
    }
    let distinct = match (distinct, columns) {
        (Some(_), Some(_)) => {
            return Err("ANALYZE takes distinct or columns, not both".into());
        }
        (Some(true), None) => DistinctColumns::All,
        (Some(false), None) | (None, None) => DistinctColumns::None,
        (None, Some(columns)) => DistinctColumns::Named(columns),
    };
    Ok(AnalyzeStatement { table, distinct })
}

/// The comma-separated `key = value` entries of a property list, commas
/// inside quotes and brackets kept.
fn split_property_entries(properties: &str) -> Result<Vec<&str>, String> {
    let mut entries = Vec::new();
    let mut start = 0;
    let mut depth = 0usize;
    let mut quoted = false;
    for (index, c) in properties.char_indices() {
        match c {
            '\'' => quoted = !quoted,
            '[' if !quoted => depth += 1,
            ']' if !quoted => {
                depth = depth
                    .checked_sub(1)
                    .ok_or_else(|| "unbalanced ']' in ANALYZE properties".to_owned())?;
            }
            ',' if !quoted && depth == 0 => {
                entries.push(&properties[start..index]);
                start = index + 1;
            }
            _ => {}
        }
    }
    if quoted {
        return Err("unterminated string in ANALYZE properties".into());
    }
    if depth != 0 {
        return Err("unbalanced '[' in ANALYZE properties".into());
    }
    entries.push(&properties[start..]);
    if entries.iter().any(|entry| entry.trim().is_empty()) {
        return Err("empty entry in ANALYZE properties".into());
    }
    Ok(entries)
}

/// `ARRAY['a', 'b']`: at least one single-quoted column name (`''` for a
/// quote), none repeated.
fn parse_column_array(value: &str) -> Result<Vec<String>, String> {
    let malformed = || {
        "ANALYZE property columns must be ARRAY['a', 'b'] of single-quoted column names".to_owned()
    };
    let items = value
        .get(..5)
        .filter(|word| word.eq_ignore_ascii_case("ARRAY"))
        .and_then(|_| value.get(5..))
        .map(str::trim_start)
        .and_then(|rest| rest.strip_prefix('['))
        .and_then(|rest| rest.strip_suffix(']'))
        .ok_or_else(malformed)?;
    let mut columns: Vec<String> = Vec::new();
    let mut rest = items.trim();
    if rest.is_empty() {
        return Err("ANALYZE property columns names no column".into());
    }
    loop {
        let unquoted = rest.strip_prefix('\'').ok_or_else(malformed)?;
        let mut name = String::new();
        let mut chars = unquoted.char_indices().peekable();
        let mut closed = None;
        while let Some((index, c)) = chars.next() {
            if c != '\'' {
                name.push(c);
            } else if chars.peek().is_some_and(|(_, next)| *next == '\'') {
                name.push('\'');
                chars.next();
            } else {
                closed = Some(index + 1);
                break;
            }
        }
        let after = closed.ok_or_else(malformed)?;
        if name.is_empty() {
            return Err("ANALYZE property columns names an empty column".into());
        }
        if columns.contains(&name) {
            return Err(format!("ANALYZE property columns names '{name}' twice"));
        }
        columns.push(name);
        rest = unquoted[after..].trim_start();
        match rest.strip_prefix(',') {
            Some(next) => rest = next.trim_start(),
            None if rest.is_empty() => return Ok(columns),
            None => return Err(malformed()),
        }
    }
}

/// `[catalog.][schema.]table` of plain or double-quoted identifier parts,
/// the form `ANALYZE` and the statistics statements accept; `None` for
/// anything else.
fn bounded_table_name(rest: &str) -> Option<String> {
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

/// The statements that read a table's statistics.
#[derive(Clone, Debug, PartialEq, Eq)]
enum StatisticsStatement {
    /// `SHOW STATS FOR [catalog.][schema.]table`
    ShowStats(String),
    /// `DESCRIBE DETAIL [catalog.][schema.]table`
    DescribeDetail(String),
}

fn parse_statistics_statement(sql: &str) -> Option<StatisticsStatement> {
    let words = sql.split_whitespace().collect::<Vec<_>>();
    let keyword = |index: usize, expected: &str| {
        words
            .get(index)
            .is_some_and(|word| word.eq_ignore_ascii_case(expected))
    };
    if words.len() == 4 && keyword(0, "SHOW") && keyword(1, "STATS") && keyword(2, "FOR") {
        return bounded_table_name(words[3]).map(StatisticsStatement::ShowStats);
    }
    if words.len() == 3 && (keyword(0, "DESCRIBE") || keyword(0, "DESC")) && keyword(1, "DETAIL") {
        return bounded_table_name(words[2]).map(StatisticsStatement::DescribeDetail);
    }
    None
}

/// The session-qualified `catalog.schema.table` of a bounded table name.
fn qualify_table(context: &QueryContext, table: &str) -> String {
    match table.split('.').count() {
        1 => format!("{}.{}.{}", context.catalog, context.schema, table),
        2 => format!("{}.{}", context.catalog, table),
        _ => table.to_owned(),
    }
}

/// The version of the statistics document `ANALYZE` writes.
const STATISTICS_DOCUMENT_VERSION: u64 = 2;

/// The statistics document for a profiled source: what `ANALYZE` stores at
/// `statistics/<operation>.json` and `SHOW STATS FOR` / `DESCRIBE DETAIL`
/// read back.
fn statistics_document(
    qualified: &str,
    catalog_snapshot_sha256: &str,
    location: &str,
    profile: &kaveon_storage::SourceProfile,
    distinct: &BTreeMap<String, u64>,
) -> serde_json::Value {
    let columns = profile
        .columns
        .iter()
        .map(|column| {
            serde_json::json!({
                "name": column.name,
                "type": kaveon_sql::ddl::sql_type_name(&column.data_type),
                "nulls": column.nulls,
                "min": column.min,
                "max": column.max,
                "compressed_bytes": column.compressed_bytes,
                "distinct": distinct.get(&column.name),
            })
        })
        .collect::<Vec<_>>();
    serde_json::json!({
        "version": STATISTICS_DOCUMENT_VERSION,
        "table": qualified,
        "analyzed_at_ms": unix_time_ms(),
        "catalog_snapshot_sha256": catalog_snapshot_sha256,
        "source_identity_sha256": profile.statistics.identity_sha256,
        "format": format_name(profile.format),
        "location": location,
        "delta_version": profile.statistics.delta_version,
        "row_count": profile.statistics.row_count,
        "file_count": profile.file_count,
        "row_group_count": profile.row_group_count,
        "compressed_bytes": profile.compressed_bytes,
        "uncompressed_bytes": profile.uncompressed_bytes,
        "last_modified_ms": profile.last_modified_ms,
        "partition_columns": profile.partition_columns,
        "columns": columns,
    })
}

fn format_name(format: kaveon_core::DataFormat) -> &'static str {
    match format {
        kaveon_core::DataFormat::Parquet => "parquet",
        kaveon_core::DataFormat::Delta => "delta",
        kaveon_core::DataFormat::Iceberg => "iceberg",
    }
}

/// Milliseconds since the epoch as ISO 8601 UTC text, for the timestamp
/// columns of `DESCRIBE DETAIL`.
fn iso_utc_ms(value: Option<i64>) -> serde_json::Value {
    value
        .map(|value| {
            kaveon_storage::StatValue::Timestamp {
                value,
                unit: arrow::datatypes::TimeUnit::Millisecond,
                utc: true,
            }
            .to_json()
        })
        .unwrap_or(serde_json::Value::Null)
}

/// A statistics bound as `SHOW STATS FOR` presents it: text.
fn bound_text(value: &serde_json::Value) -> serde_json::Value {
    match value {
        serde_json::Value::Null => serde_json::Value::Null,
        serde_json::Value::String(text) => serde_json::Value::String(text.clone()),
        other => serde_json::Value::String(other.to_string()),
    }
}

fn varchar(name: &str) -> ColumnInfo {
    ColumnInfo {
        name: name.into(),
        data_type: "VARCHAR".into(),
    }
}

fn bigint(name: &str) -> ColumnInfo {
    ColumnInfo {
        name: name.into(),
        data_type: "BIGINT".into(),
    }
}

/// A timestamp column whose values are ISO 8601 UTC text (see
/// [`iso_utc_ms`]).
fn timestamp(name: &str) -> ColumnInfo {
    ColumnInfo {
        name: name.into(),
        data_type: "TIMESTAMP".into(),
    }
}

/// `SHOW STATS FOR` over a stored statistics document: Trino's columns, one
/// row per column and a summary row whose `column_name` is null and which
/// carries the table's `row_count` and total `data_size`; `analyzed_at` is
/// the same on every row. A version 1 document (row count and column names
/// only) yields name-only rows and no `analyzed_at`.
fn show_stats_result(
    document: &serde_json::Value,
) -> (Vec<ColumnInfo>, Vec<Vec<serde_json::Value>>) {
    let columns = vec![
        varchar("column_name"),
        varchar("data_type"),
        bigint("data_size"),
        ColumnInfo {
            name: "nulls_fraction".into(),
            data_type: "DOUBLE".into(),
        },
        bigint("distinct_values_count"),
        varchar("low_value"),
        varchar("high_value"),
        bigint("row_count"),
        timestamp("analyzed_at"),
    ];
    let row_count = document["row_count"].as_u64();
    let analyzed_at = iso_utc_ms(document["analyzed_at_ms"].as_i64());
    let mut rows = document["columns"]
        .as_array()
        .map(|columns| {
            columns
                .iter()
                .map(|column| {
                    let (name, column) = match column {
                        serde_json::Value::String(name) => (name.clone(), None),
                        other => (
                            other["name"].as_str().unwrap_or_default().to_owned(),
                            Some(other),
                        ),
                    };
                    let nulls = column.and_then(|column| column["nulls"].as_u64());
                    let nulls_fraction = match (nulls, row_count) {
                        (Some(nulls), Some(rows)) if rows > 0 => {
                            serde_json::json!(nulls as f64 / rows as f64)
                        }
                        _ => serde_json::Value::Null,
                    };
                    let field = |key: &str| {
                        column
                            .map(|column| column[key].clone())
                            .unwrap_or(serde_json::Value::Null)
                    };
                    vec![
                        serde_json::json!(name),
                        field("type"),
                        field("compressed_bytes"),
                        nulls_fraction,
                        field("distinct"),
                        bound_text(&field("min")),
                        bound_text(&field("max")),
                        serde_json::Value::Null,
                        analyzed_at.clone(),
                    ]
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    rows.push(vec![
        serde_json::Value::Null,
        serde_json::Value::Null,
        document["compressed_bytes"].clone(),
        serde_json::Value::Null,
        serde_json::Value::Null,
        serde_json::Value::Null,
        serde_json::Value::Null,
        serde_json::json!(row_count),
        analyzed_at,
    ]);
    (columns, rows)
}

/// `DESCRIBE DETAIL`: the table-level facts, from the stored document when
/// the table was analyzed and from a fresh metadata read otherwise.
fn describe_detail_result(
    format: kaveon_core::DataFormat,
    location: &str,
    document: Option<&serde_json::Value>,
    fresh: Option<&kaveon_storage::SourceProfile>,
) -> (Vec<ColumnInfo>, Vec<Vec<serde_json::Value>>) {
    let columns = vec![
        varchar("format"),
        varchar("location"),
        timestamp("created_at"),
        timestamp("last_modified"),
        bigint("num_files"),
        bigint("size_in_bytes"),
        bigint("row_count"),
        bigint("delta_version"),
        varchar("partition_columns"),
        timestamp("analyzed_at"),
        varchar("catalog_snapshot"),
    ];
    let row = match (document, fresh) {
        (Some(document), _) => vec![
            serde_json::json!(format_name(format)),
            serde_json::json!(location),
            serde_json::Value::Null,
            iso_utc_ms(document["last_modified_ms"].as_i64()),
            document["file_count"].clone(),
            document["compressed_bytes"].clone(),
            document["row_count"].clone(),
            document["delta_version"].clone(),
            document["partition_columns"]
                .as_array()
                .map(|names| {
                    serde_json::json!(
                        names
                            .iter()
                            .filter_map(serde_json::Value::as_str)
                            .collect::<Vec<_>>()
                            .join(",")
                    )
                })
                .unwrap_or(serde_json::Value::Null),
            iso_utc_ms(document["analyzed_at_ms"].as_i64()),
            document["catalog_snapshot_sha256"].clone(),
        ],
        (None, Some(profile)) => vec![
            serde_json::json!(format_name(format)),
            serde_json::json!(location),
            serde_json::Value::Null,
            iso_utc_ms(profile.last_modified_ms),
            serde_json::json!(profile.file_count),
            serde_json::json!(profile.compressed_bytes),
            serde_json::Value::Null,
            serde_json::json!(profile.statistics.delta_version),
            serde_json::json!(profile.partition_columns.join(",")),
            serde_json::Value::Null,
            serde_json::Value::Null,
        ],
        (None, None) => Vec::new(),
    };
    (columns, vec![row])
}

/// Runs `SHOW STATS FOR` or `DESCRIBE DETAIL` for any statement-capable
/// role and answers inline like `ANALYZE`.
async fn execute_statistics_statement(
    state: &Arc<AppState>,
    query_id: &str,
    context: &QueryContext,
    statement: StatisticsStatement,
    started: Instant,
) -> Response {
    let (table, show_stats) = match statement {
        StatisticsStatement::ShowStats(table) => (table, true),
        StatisticsStatement::DescribeDetail(table) => (table, false),
    };
    let qualified = qualify_table(context, &table);
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
    let location = resolved.full_path();
    let format = resolved.table.format;
    let stored = match state.product_transactions.catalog() {
        Some(commit) => match stored_statistics_document(&commit, &qualified).await {
            Ok(document) => document,
            Err((status, code, message)) => {
                return analyze_failure(query_id, started, status, code, message).await;
            }
        },
        None if show_stats => {
            return analyze_failure(
                query_id,
                started,
                StatusCode::SERVICE_UNAVAILABLE,
                "STATISTICS_DISABLED",
                "durable statistics are disabled".into(),
            )
            .await;
        }
        None => None,
    };
    let (columns, rows) = if show_stats {
        let Some(document) = stored else {
            return analyze_failure(
                query_id,
                started,
                StatusCode::BAD_REQUEST,
                "STATISTICS_UNAVAILABLE",
                format!("no statistics for {qualified}; run ANALYZE {qualified}"),
            )
            .await;
        };
        show_stats_result(&document)
    } else {
        let fresh = if stored.is_none() {
            let read = tokio::task::spawn_blocking({
                let location = location.clone();
                move || kaveon_storage::profile_source(&location, format)
            })
            .await;
            match read {
                Ok(Ok(profile)) => Some(profile),
                Ok(Err(error)) => {
                    return analyze_failure(
                        query_id,
                        started,
                        StatusCode::BAD_REQUEST,
                        "DESCRIBE_FAILED",
                        error.to_string(),
                    )
                    .await;
                }
                Err(_) => {
                    return analyze_failure(
                        query_id,
                        started,
                        StatusCode::INTERNAL_SERVER_ERROR,
                        "DESCRIBE_FAILED",
                        "metadata read did not complete".into(),
                    )
                    .await;
                }
            }
        } else {
            None
        };
        describe_detail_result(format, &location, stored.as_ref(), fresh.as_ref())
    };
    finish_inline_statement(query_id, started, columns, rows).await
}

/// The table's stored statistics document, parsed; `None` when the table
/// was never analyzed.
async fn stored_statistics_document(
    commit: &kaveon_catalog::product_commit::ProductCatalogCommit,
    qualified: &str,
) -> Result<Option<serde_json::Value>, (StatusCode, &'static str, String)> {
    let snapshot = commit.read_current().await.map_err(|_| {
        (
            StatusCode::SERVICE_UNAVAILABLE,
            "CATALOG_UNAVAILABLE",
            "cannot read product catalog head".to_owned(),
        )
    })?;
    statistics_document_in(commit, &snapshot, qualified).await
}

/// The table's statistics document under `snapshot`, parsed; `None` when
/// the snapshot holds none.
async fn statistics_document_in(
    commit: &kaveon_catalog::product_commit::ProductCatalogCommit,
    snapshot: &kaveon_catalog::product_manifest::CatalogSnapshot,
    qualified: &str,
) -> Result<Option<serde_json::Value>, (StatusCode, &'static str, String)> {
    let Some(stored) = snapshot.table_statistics.get(qualified) else {
        return Ok(None);
    };
    let unreadable = || {
        (
            StatusCode::SERVICE_UNAVAILABLE,
            "STATISTICS_INVALID",
            format!("stored statistics for {qualified} are not readable"),
        )
    };
    let bytes = commit
        .fetch_immutable_file(&stored.document)
        .await
        .map_err(|_| unreadable())?;
    serde_json::from_slice::<serde_json::Value>(&bytes)
        .map(Some)
        .map_err(|_| unreadable())
}

/// Finishes a statement that answered on the coordinator: the record and
/// the response carry the same columns and rows.
async fn finish_inline_statement(
    query_id: &str,
    started: Instant,
    columns: Vec<ColumnInfo>,
    rows: Vec<Vec<serde_json::Value>>,
) -> Response {
    let elapsed = started.elapsed().as_millis().try_into().unwrap_or(u64::MAX);
    if let Some(record) = QUERY_STORE.write().await.queries.get_mut(query_id) {
        record.state = QueryState::Finished;
        record.next_uri = None;
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

/// `ANALYZE`: the metadata profile, then — for `WITH (distinct = true)` or
/// `WITH (columns = ARRAY[…])` — one `SELECT COUNT(DISTINCT "column")`
/// statement per selected column through [`run_statement`], sequentially,
/// cancelled with this statement; then the source identity is read again
/// and one document is committed. A column not counted by this statement
/// keeps the count of the previous document when the source identity is
/// unchanged, else it is null.
async fn execute_analyze(
    state: &Arc<AppState>,
    identity: &Identity,
    query_id: &str,
    context: &QueryContext,
    statement: Result<AnalyzeStatement, String>,
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
    let statement = match statement {
        Ok(statement) => statement,
        Err(message) => {
            return analyze_failure(
                query_id,
                started,
                StatusCode::BAD_REQUEST,
                "SYNTAX_ERROR",
                message,
            )
            .await;
        }
    };
    let qualified = qualify_table(context, &statement.table);
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
    let location = resolved.full_path();
    let first = match kaveon_storage::profile_source(&location, resolved.table.format) {
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
    let selected = match &statement.distinct {
        DistinctColumns::None => Vec::new(),
        DistinctColumns::All => first
            .columns
            .iter()
            .map(|column| column.name.clone())
            .collect(),
        DistinctColumns::Named(names) => {
            if let Some(unknown) = names
                .iter()
                .find(|name| !first.columns.iter().any(|column| column.name == **name))
            {
                return analyze_failure(
                    query_id,
                    started,
                    StatusCode::BAD_REQUEST,
                    "ANALYSIS_ERROR",
                    format!("column '{unknown}' does not exist in {qualified}"),
                )
                .await;
            }
            names.clone()
        }
    };
    let cancellation = match state.lifecycle.cancellations.token(query_id) {
        Ok(token) => token,
        Err(error) => {
            return analyze_failure(
                query_id,
                started,
                StatusCode::SERVICE_UNAVAILABLE,
                "ANALYZE_FAILED",
                error.to_string(),
            )
            .await;
        }
    };
    let mut measured = BTreeMap::new();
    for column in &selected {
        match count_distinct_values(
            state,
            identity,
            query_id,
            context,
            &qualified,
            column,
            &cancellation,
        )
        .await
        {
            Ok(count) => {
                measured.insert(column.clone(), count);
            }
            Err(SubStatementError::Canceled) => return canceled_task_response(),
            Err(SubStatementError::Failed {
                status,
                code,
                message,
            }) => {
                return analyze_failure(query_id, started, status, &code, message).await;
            }
        }
    }
    let second = match kaveon_storage::profile_source(&location, resolved.table.format) {
        Ok(value) if value.statistics.identity_sha256 == first.statistics.identity_sha256 => value,
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
    // The counts the previous document holds for the same source identity
    // stay; a column counted now takes the new count.
    let mut distinct = match statistics_document_in(&commit, &current, &qualified).await {
        Ok(previous) => previous
            .filter(|previous| {
                previous["source_identity_sha256"] == second.statistics.identity_sha256
            })
            .map(|previous| preserved_distinct_counts(&previous))
            .unwrap_or_default(),
        Err((status, code, message)) => {
            return analyze_failure(query_id, started, status, code, message).await;
        }
    };
    distinct.extend(measured);
    let document = serde_json::to_vec(&statistics_document(
        &qualified,
        &catalog_snapshot_sha256,
        &location,
        &second,
        &distinct,
    ))
    .unwrap();
    let row_count = second.statistics.row_count;
    let source_identity_sha256 = second.statistics.identity_sha256;
    let document_sha = format!("{:x}", Sha256::digest(&document));
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
                    source_identity_sha256: source_identity_sha256.clone(),
                },
            },
            CatalogChange::PutStatistics {
                table: qualified.clone(),
                statistics: TableStatisticsRef {
                    catalog_snapshot_sha256,
                    source_identity_sha256,
                    document: ImmutableFileRef {
                        path: path.clone(),
                        sha256: document_sha,
                    },
                    row_count,
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
        varchar("table"),
        bigint("row_count"),
        bigint("distinct_columns"),
    ];
    let rows = vec![vec![
        serde_json::json!(qualified),
        serde_json::json!(row_count),
        serde_json::json!(selected.len()),
    ]];
    if let Some(record) = QUERY_STORE.write().await.queries.get_mut(query_id) {
        record.state = QueryState::Finished;
        record.next_uri = None;
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

/// A catalog statement (`CREATE`/`DROP`/`ALTER` on the durable catalog,
/// `SHOW`/`DESCRIBE` over the published snapshot) runs on the coordinator
/// and leaves a query record like any statement.
async fn execute_catalog(
    state: &Arc<AppState>,
    identity: &Identity,
    query_id: &str,
    context: &QueryContext,
    statement: kaveon_sql::ddl::CatalogStatement,
    started: Instant,
) -> Response {
    let result = crate::catalog_ddl::execute_catalog_statement(
        state,
        identity,
        &context.catalog,
        &context.schema,
        statement,
    )
    .await;
    let result = match result {
        Ok(result) => result,
        Err(error) => {
            finish_failed_query(query_id, error.message.clone(), started, None, None, None).await;
            return (
                error.status,
                Json(serde_json::json!({
                    "id": query_id,
                    "error": error.message,
                    "code": error.code
                })),
            )
                .into_response();
        }
    };
    let elapsed = started.elapsed().as_millis().try_into().unwrap_or(u64::MAX);
    if let Some(record) = QUERY_STORE.write().await.queries.get_mut(query_id) {
        record.state = QueryState::Finished;
        record.next_uri = None;
        record.columns = result.columns.clone();
        record.rows = result.rows.clone();
        record.elapsed_ms = elapsed;
        record.completed_at_ms = unix_time_ms();
    }
    Json(StatementResponse {
        next_uri: None,
        id: query_id.into(),
        state: QueryState::Finished,
        columns: Some(result.columns),
        data: Some(result.rows),
        error: None,
        elapsed_ms: elapsed,
    })
    .into_response()
}

/// The distinct counts of a stored document: column name to count, for
/// the columns that carry one.
fn preserved_distinct_counts(document: &serde_json::Value) -> BTreeMap<String, u64> {
    document["columns"]
        .as_array()
        .map(|columns| {
            columns
                .iter()
                .filter_map(|column| {
                    Some((
                        column["name"].as_str()?.to_owned(),
                        column["distinct"].as_u64()?,
                    ))
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Why a statement `ANALYZE` ran on its behalf did not answer with a value.
enum SubStatementError {
    /// The `ANALYZE` statement was cancelled; the sub-statement with it.
    Canceled,
    /// The sub-statement failed: its status, code and message, the column
    /// named.
    Failed {
        status: StatusCode,
        code: String,
        message: String,
    },
}

/// A double-quoted SQL identifier.
fn quote_identifier(name: &str) -> String {
    format!("\"{}\"", name.replace('"', "\"\""))
}

/// `SELECT COUNT(DISTINCT "column") FROM catalog.schema.table` as a
/// statement of its own through [`run_statement`] — admitted, recorded,
/// planned and executed as a client statement would be, tagged
/// `analyze:<parent id>`, the result cache off — cancelled when `parent`
/// is. The exact count of the column's non-null distinct values.
async fn count_distinct_values(
    state: &Arc<AppState>,
    identity: &Identity,
    parent_id: &str,
    context: &QueryContext,
    qualified: &str,
    column: &str,
    parent: &CancellationToken,
) -> Result<u64, SubStatementError> {
    let failed = |status: StatusCode, code: &str, message: String| SubStatementError::Failed {
        status,
        code: code.to_owned(),
        message: format!("distinct count of column '{column}' failed: {message}"),
    };
    // The table name is bounded to identifier characters (see
    // `bounded_table_name`); the column is whatever the source calls it.
    let query = format!(
        "SELECT COUNT(DISTINCT {}) FROM {qualified}",
        quote_identifier(column)
    );
    let mut settings = match serde_json::to_value(&context.settings) {
        Ok(serde_json::Value::Object(map)) => map,
        _ => serde_json::Map::new(),
    };
    settings.insert("result_cache".into(), serde_json::Value::Bool(false));
    let mut client_tags = context.client_tags.clone();
    client_tags.push(format!("analyze:{parent_id}"));
    let request = StatementRequest {
        query,
        catalog: Some(context.catalog.clone()),
        schema: Some(context.schema.clone()),
        source: context.source.clone(),
        client: context.client.clone(),
        user: None,
        time_zone: context.time_zone.clone(),
        client_tags,
        result_delivery: None,
        settings: Some(settings),
    };
    let child_id = Uuid::new_v4().to_string();
    // The child's token exists before it starts, so a cancellation of the
    // parent that lands first is seen at the child's first check.
    state
        .lifecycle
        .cancellations
        .token(&child_id)
        .map_err(|error| {
            failed(
                StatusCode::SERVICE_UNAVAILABLE,
                "ANALYZE_FAILED",
                error.to_string(),
            )
        })?;
    let mut child = Box::pin(run_statement(
        Arc::clone(state),
        identity.clone(),
        request,
        child_id.clone(),
    ));
    let response = tokio::select! {
        response = &mut child => response,
        () = parent.cancelled() => {
            // The token first — it is what a child that has no record yet
            // checks — then the client's cancellation of the child's record
            // and its worker tasks.
            let _ = state.lifecycle.cancellations.cancel(&child_id);
            let _ = cancel_query(
                State(Arc::clone(state)),
                Extension(identity.clone()),
                Path(child_id.clone()),
            )
            .await;
            let response = child.await;
            // A child that left the admission queue on the token alone
            // still has a queued record: it was cancelled.
            if let Some(record) = QUERY_STORE.write().await.queries.get_mut(&child_id)
                && matches!(record.state, QueryState::Queued | QueryState::Running)
            {
                record.state = QueryState::Canceled;
                record.error = Some(format!("canceled with ANALYZE {parent_id}"));
                record.completed_at_ms = unix_time_ms();
            }
            response
        }
    };
    if parent.is_cancelled() {
        return Err(SubStatementError::Canceled);
    }
    let status = response.status();
    let body = axum::body::to_bytes(response.into_body(), 1 << 20)
        .await
        .map_err(|error| {
            failed(
                StatusCode::INTERNAL_SERVER_ERROR,
                "ANALYZE_FAILED",
                error.to_string(),
            )
        })?;
    let body = serde_json::from_slice::<serde_json::Value>(&body).map_err(|error| {
        failed(
            StatusCode::INTERNAL_SERVER_ERROR,
            "ANALYZE_FAILED",
            error.to_string(),
        )
    })?;
    if status != StatusCode::OK {
        let code = body["code"].as_str().unwrap_or("ANALYZE_FAILED");
        let message = body["error"]
            .as_str()
            .map(str::to_owned)
            .unwrap_or_else(|| format!("HTTP {status}"));
        return Err(failed(status, code, message));
    }
    let value = &body["data"][0][0];
    value
        .as_u64()
        .or_else(|| value.as_str().and_then(|text| text.parse().ok()))
        .ok_or_else(|| {
            failed(
                StatusCode::INTERNAL_SERVER_ERROR,
                "ANALYZE_FAILED",
                format!("the count came back as {value}"),
            )
        })
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
        Json(serde_json::json!({"id":query_id,"error":message,"code":code})),
    )
        .into_response()
}

#[derive(Debug, Serialize)]
struct TransactionCapabilities {
    enabled: bool,
    supported_statements: [&'static str; 6],
    single_statement_per_request: bool,
    parameter_binding: bool,
    multi_row_insert: bool,
    returning: bool,
    savepoints: bool,
    explicit_isolation_modes: bool,
    arbitrary_table_dml: bool,
}

fn transaction_api_guidance(
    sql: &str,
    transaction_api_enabled: bool,
) -> Option<(StatusCode, serde_json::Value)> {
    let parsed = parse_native_transactional(sql).ok()?;
    let supported = match parsed {
        NativeTransactionalStatement::Begin
        | NativeTransactionalStatement::Commit
        | NativeTransactionalStatement::Rollback => true,
        NativeTransactionalStatement::Dml(dml) => adapt_product_dml(&dml).is_ok(),
    };
    if !supported {
        return None;
    }
    if transaction_api_enabled {
        Some((
            StatusCode::BAD_REQUEST,
            serde_json::json!({
                "error": "transaction statements must use the authenticated transaction API",
                "code": "TRANSACTION_API_REQUIRED",
                "transaction_endpoint": "/v1/transaction/sql",
                "capabilities_endpoint": "/v1/capabilities"
            }),
        ))
    } else {
        Some((
            StatusCode::SERVICE_UNAVAILABLE,
            serde_json::json!({
                "error": "the transaction API is not configured",
                "code": "TRANSACTION_API_UNAVAILABLE",
                "transaction_endpoint": "/v1/transaction/sql",
                "capabilities_endpoint": "/v1/capabilities"
            }),
        ))
    }
}

#[derive(Debug, Serialize)]
struct EngineCapabilities {
    native_analyze: bool,
    transactions: TransactionCapabilities,
}

#[derive(Serialize)]
struct WhoamiResponse<'a> {
    principal: &'a str,
    display: Option<&'a str>,
    role: &'static str,
    auth: crate::security::AuthSource,
}

/// The identity the security layer attached to this request. Clients use it
/// for their session header; nothing here grants or changes access.
async fn whoami(
    Extension(identity): Extension<Identity>,
    source: Option<Extension<crate::security::AuthSource>>,
) -> Json<serde_json::Value> {
    let role = match identity.role {
        crate::security::Role::Reader => "reader",
        crate::security::Role::Analyst => "analyst",
        crate::security::Role::Admin => "admin",
    };
    let auth = source.map_or(crate::security::AuthSource::Static, |Extension(source)| {
        source
    });
    Json(
        serde_json::to_value(WhoamiResponse {
            principal: &identity.principal,
            display: identity.display_identity.as_deref(),
            role,
            auth,
        })
        .expect("whoami serializes"),
    )
}

async fn capabilities(State(state): State<Arc<AppState>>) -> Json<EngineCapabilities> {
    let transactions_enabled = state.product_transactions.catalog().is_some();
    Json(EngineCapabilities {
        native_analyze: state.config.coordinator && transactions_enabled,
        transactions: TransactionCapabilities {
            enabled: transactions_enabled,
            supported_statements: [
                "BEGIN",
                "INSERT product record",
                "UPDATE product record",
                "DELETE product record",
                "COMMIT",
                "ROLLBACK",
            ],
            single_statement_per_request: true,
            parameter_binding: false,
            multi_row_insert: false,
            returning: false,
            savepoints: false,
            explicit_isolation_modes: false,
            arbitrary_table_dml: false,
        },
    })
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
) -> (LogicalPlan, SourcePins) {
    let mut tables = std::collections::BTreeSet::new();
    collect_join_statistics_tables(&plan, &mut tables);
    if tables.is_empty() {
        return (plan, SourcePins::default());
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
            let current = kaveon_storage::analyze_source(&location, format).ok();
            (table, qualified, location, current)
        });
    }
    let catalog_digest = format!("{:x}", Sha256::digest(catalog.snapshot_id.as_bytes()));
    let mut cache = HashMap::new();
    let mut pins = SourcePins::default();
    while let Some(loaded) = loads.join_next().await {
        let Ok((table, qualified, location, current)) = loaded else {
            continue;
        };
        if let Some(version) = current.as_ref().and_then(|value| value.delta_version) {
            pins.delta_versions.insert(location.clone(), version);
        }
        if let Some(listing) = current
            .as_ref()
            .and_then(|value| value.parquet_listing.clone())
        {
            pins.parquet_directories.insert(location, listing);
        }
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
    (
        kaveon_optim::statistics::optimize_with_statistics(plan, &mut |table| {
            cache.get(table).cloned().flatten()
        }),
        pins,
    )
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
        .filter(|record| !matches!(record.state, QueryState::Queued | QueryState::Running))
        .map(|record| (record.submitted_at_ms, record.id.clone()))
        .collect();
    terminal.sort_unstable();
    let remove = terminal.len().saturating_sub(QUERY_HISTORY_LIMIT - 1);
    for (_, id) in terminal.into_iter().take(remove) {
        store.queries.remove(&id);
    }
}

/// Pages the rows a path collected in memory through the statement's writer
/// and completes the result. Without a writer the path streamed the rows
/// itself and already published; the result must then be registered.
fn spool_rows(
    state: &AppState,
    id: &str,
    writer: Option<crate::results::ResultWriter>,
    rows: &mut Vec<Vec<serde_json::Value>>,
) -> std::io::Result<String> {
    let uri = format!("/v1/query/{id}/results/0");
    let Some(mut writer) = writer else {
        return if state.results.contains(id) {
            Ok(uri)
        } else {
            Err(std::io::Error::other("paged result was not published"))
        };
    };
    for row in rows.drain(..) {
        writer.push(row)?;
    }
    state.results.publish(id, writer)?;
    Ok(uri)
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
        Ok(crate::results::ResultPage::Ready(value)) => Json(value).into_response(),
        Ok(crate::results::ResultPage::Pending(value)) => (
            StatusCode::ACCEPTED,
            [(axum::http::header::RETRY_AFTER, "1")],
            Json(value),
        )
            .into_response(),
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
    let was_running = matches!(record.state, QueryState::Queued | QueryState::Running);
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

    let mut coordinator = nodes
        .iter()
        .find(|n| n.role == NodeRole::Coordinator)
        .cloned()
        .unwrap_or_else(|| cluster.this_node.clone());
    if state.config.coordinator {
        coordinator.result_cache = Some(state.result_cache.stats());
        coordinator.admission = Some(state.memory_admission.stats());
    }

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
    let mut node = cluster.this_node.clone();
    if state.config.coordinator {
        node.result_cache = Some(state.result_cache.stats());
    }
    node.admission = Some(state.memory_admission.stats());
    Json(node)
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

/// Rebuild the planning snapshot from the durable definitions and publish
/// it; every catalog mutation ends here, whichever surface made it.
pub(crate) async fn publish_catalog_snapshot(state: &AppState) -> anyhow::Result<()> {
    let snapshot = crate::config::catalog_manager_snapshot(&state.catalog_store)
        .map_err(|error| anyhow::anyhow!("catalog snapshot failed: {error}"))?;
    let snapshot_id = state
        .catalog_store
        .snapshot_identity()
        .map_err(|error| anyhow::anyhow!("catalog identity failed: {error}"))?;
    *state.catalog.write().await = Arc::new(crate::PublishedCatalog {
        manager: snapshot,
        snapshot_id,
    });
    // A new snapshot identity already misses every key; dropping the
    // entries bounds staleness and frees the budget at once.
    state.result_cache.clear();
    Ok(())
}

pub(crate) async fn refresh_catalog_snapshot(state: &AppState) -> Result<(), Box<Response>> {
    publish_catalog_snapshot(state).await.map_err(|error| {
        Box::new(
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({ "error": error.to_string() })),
            )
                .into_response(),
        )
    })
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
        let mut writer =
            arrow::ipc::writer::StreamWriter::try_new_with_options(&mut bytes, schema, options)
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
            data_type: presented_type(field.data_type()),
        })
        .collect()
}

#[derive(Clone, Copy)]
enum MergeOperation {
    Add,
    Min,
    Max,
}

/// How long one task may run before the coordinator gives it up. Clients
/// bound their own waits (and cancel on the way out); this is the ceiling for
/// a stage over the full table, not an interactive budget.
const REMOTE_TASK_TIMEOUT: Duration = Duration::from_secs(600);

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
            message: if error.is_timeout() {
                format!(
                    "worker '{}' did not finish the task within {}s",
                    worker.node_id,
                    REMOTE_TASK_TIMEOUT.as_secs()
                )
            } else {
                format!("worker '{}' is unavailable: {error}", worker.node_id)
            },
            // A task that ran out of time would run out of time again, and
            // the first attempt is still running until the query is finished.
            retryable: !error.is_timeout(),
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

/// What a distributed run writes back besides its result: why the
/// coordinator ran the statement instead, and the paged writer its root
/// tasks stream into (`None` for inline delivery; taken once published).
struct DistributedSink<'a> {
    placement_reason: &'a mut Option<String>,
    result_writer: &'a mut Option<crate::results::ResultWriter>,
}

async fn execute_distributed_fragments(
    state: &Arc<AppState>,
    query_id: &str,
    context: &QueryContext,
    plan: &LogicalPlan,
    catalog_snapshot: &kaveon_core::CatalogManager,
    pins: &SourcePins,
    sink: DistributedSink<'_>,
) -> Option<Result<(TaskResponse, Vec<StageTelemetry>, u64), String>> {
    let DistributedSink {
        placement_reason,
        result_writer,
    } = sink;
    if exact_metadata_count_plan(plan) {
        *placement_reason = Some("exact count answered from table metadata".to_owned());
        return None;
    }
    if !general_distributed_eligible(plan) {
        *placement_reason = Some("shape has no distributed plan".to_owned());
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
    let Some(token) = state
        .config
        .exchange_token
        .clone()
        .filter(|token| !token.is_empty())
    else {
        *placement_reason = Some("no exchange token is configured".to_owned());
        return None;
    };
    if workers.len() < 2 {
        *placement_reason = Some(format!(
            "{} compatible worker(s); distributed execution needs two",
            workers.len()
        ));
        return None;
    }

    let planning_start = Instant::now();
    // A shape the stage planner cannot express runs on the coordinator
    // instead; that downgrade is worth a line in the log.
    let graph = match crate::planner::build_stage_graph(query_id, plan, workers.len()) {
        Ok(graph) => graph,
        Err(error) => {
            eprintln!("query {query_id} runs on the coordinator: stage graph: {error}");
            *placement_reason = Some(format!("stage graph: {error}"));
            return None;
        }
    };
    let fragments = match crate::planner::build_executable_fragments_with_pins(
        query_id,
        plan,
        catalog_snapshot,
        workers.len(),
        pins,
    ) {
        Ok(fragments) => fragments,
        Err(error) => {
            eprintln!("query {query_id} runs on the coordinator: fragments: {error}");
            *placement_reason = Some(format!("fragments: {error}"));
            return None;
        }
    };
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
                        if result_schema.is_none() && result_writer.is_some() {
                            // A paged reader can render page 0 the moment it
                            // lands: give the running record its columns now.
                            publish_columns(query_id, &column_infos(&schema)).await;
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
                    let task = TaskTelemetry {
                        task_id: task_id.to_string(),
                        node_id: worker.node_id,
                        partition_index: task_id.partition,
                        elapsed_us,
                        output_rows,
                        output_batches,
                        output_bytes,
                        execution,
                        scan,
                    };
                    publish_task_completion(
                        query_id,
                        task_id.stage_id.0,
                        dispatch.execution_partition.count,
                        stage_started
                            .get(&task_id.stage_id)
                            .map_or(0, |started| self::elapsed_us(*started)),
                        task.clone(),
                    )
                    .await;
                    stage_tasks.entry(task_id.stage_id).or_default().push(task);
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
    let mut stages = Vec::with_capacity(stage_tasks.len());
    for (stage_id, tasks) in stage_tasks {
        let task_count = tasks.len();
        let stage_elapsed_us = stage_started
            .get(&stage_id)
            .map_or(0, |started| elapsed_us(*started));
        for task in tasks {
            record_stage_task(&mut stages, stage_id.0, task_count, stage_elapsed_us, task);
        }
    }
    let data = if let Some(writer) = result_writer.take() {
        if let Err(error) = state.results.publish(query_id, writer) {
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
        LogicalPlan::SemiJoin { left, right, .. } | LogicalPlan::AntiJoin { left, right, .. } => {
            general_distributed_eligible(left) && general_distributed_eligible(right)
        }
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
        settings: context.settings.clone(),
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
        let settings = context.settings.clone();
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
                    settings: settings.clone(),
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
                publish_task_completion(
                    query_id,
                    0,
                    partition_count,
                    elapsed_us(started),
                    telemetry.clone(),
                )
                .await;
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
        let settings = context.settings.clone();
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
                    settings: settings.clone(),
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
                publish_task_completion(
                    query_id,
                    0,
                    partition_count,
                    elapsed_us(started),
                    telemetry.clone(),
                )
                .await;
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

/// The type a client sees: a dictionary-encoded column is its value type.
fn presented_type(data_type: &arrow::datatypes::DataType) -> String {
    match data_type {
        arrow::datatypes::DataType::Dictionary(_, values) => values.to_string(),
        other => other.to_string(),
    }
}

fn batches_to_json(batches: &[arrow::record_batch::RecordBatch]) -> Vec<Vec<serde_json::Value>> {
    use arrow::array::{Array, AsArray};
    use arrow::datatypes::*;

    let mut rows = Vec::new();
    for batch in batches {
        let num_cols = batch.num_columns();
        // A dictionary-encoded column is presented as its values; the
        // encoding is the file's business, not the client's.
        let columns: Vec<arrow::array::ArrayRef> = batch
            .columns()
            .iter()
            .map(|column| match column.data_type() {
                DataType::Dictionary(_, values) => {
                    arrow::compute::cast(column, values).unwrap_or_else(|_| column.clone())
                }
                _ => column.clone(),
            })
            .collect();
        for row in 0..batch.num_rows() {
            let mut cells = Vec::with_capacity(num_cols);
            for arr in columns.iter().take(num_cols) {
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

fn elapsed_ms(start: Instant) -> u64 {
    start.elapsed().as_millis().try_into().unwrap_or(u64::MAX)
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
        decoded_batch_cache_hits: snapshot.decoded_batch_cache_hits,
        decoded_batch_cache_misses: snapshot.decoded_batch_cache_misses,
        decoded_batch_cache_evictions: snapshot.decoded_batch_cache_evictions,
        decoded_batch_cache_singleflight_waits: snapshot.decoded_batch_cache_singleflight_waits,
        row_groups_considered: snapshot.row_groups_considered,
        row_groups_read: snapshot.row_groups_selected,
        row_groups_pruned: snapshot.row_groups_pruned(),
        rows_selected: snapshot.rows_selected,
        rows_emitted: snapshot.rows_emitted,
        batches_emitted: snapshot.batches_emitted,
        compressed_bytes_selected: snapshot.compressed_bytes_selected,
        compressed_bytes_read: snapshot.compressed_bytes_read,
        row_filter_rows_examined: snapshot.row_filter_rows_examined,
        row_filter_rows_admitted: snapshot.row_filter_rows_admitted,
        snapshot_ns: duration_ns(snapshot.snapshot_elapsed),
        footer_ns: duration_ns(snapshot.footer_elapsed),
        read_ns: duration_ns(snapshot.read_elapsed),
        rows_per_second: snapshot.rows_per_second(),
        compressed_bytes_per_second: snapshot.compressed_bytes_per_second(),
        lanes: snapshot.lanes,
        lane_rows_min: snapshot.lane_rows_min,
        lane_rows_max: snapshot.lane_rows_max,
        lane_read_ns_min: duration_ns(snapshot.lane_elapsed_min),
        lane_read_ns_max: duration_ns(snapshot.lane_elapsed_max),
    }
}

/// Lane spreads combine as the lightest and heaviest lane anywhere.
fn merge_lanes(
    total: &mut TaskScanMetrics,
    lanes: u64,
    rows_min: u64,
    rows_max: u64,
    ns_min: u64,
    ns_max: u64,
) {
    if lanes == 0 {
        return;
    }
    if total.lanes == 0 {
        total.lane_rows_min = rows_min;
        total.lane_read_ns_min = ns_min;
    } else {
        total.lane_rows_min = total.lane_rows_min.min(rows_min);
        total.lane_read_ns_min = total.lane_read_ns_min.min(ns_min);
    }
    total.lanes += lanes;
    total.lane_rows_max = total.lane_rows_max.max(rows_max);
    total.lane_read_ns_max = total.lane_read_ns_max.max(ns_max);
}

fn merge_task_scan_metrics<'a>(
    metrics: impl Iterator<Item = &'a kaveon_storage::ScanMetrics>,
) -> TaskScanMetrics {
    metrics.fold(TaskScanMetrics::default(), |mut total, metrics| {
        let snapshot = metrics.snapshot();
        total.files_considered += snapshot.files_considered;
        total.files_opened += snapshot.files_opened;
        total.decoded_batch_cache_hits += snapshot.decoded_batch_cache_hits;
        total.decoded_batch_cache_misses += snapshot.decoded_batch_cache_misses;
        total.decoded_batch_cache_evictions += snapshot.decoded_batch_cache_evictions;
        total.decoded_batch_cache_singleflight_waits +=
            snapshot.decoded_batch_cache_singleflight_waits;
        total.row_groups_considered += snapshot.row_groups_considered;
        total.row_groups_selected += snapshot.row_groups_selected;
        total.rows_selected += snapshot.rows_selected;
        total.rows_emitted += snapshot.rows_emitted;
        total.compressed_bytes_selected += snapshot.compressed_bytes_selected;
        total.compressed_bytes_read += snapshot.compressed_bytes_read;
        total.row_filter_rows_examined += snapshot.row_filter_rows_examined;
        total.row_filter_rows_admitted += snapshot.row_filter_rows_admitted;
        total.batches_emitted += snapshot.batches_emitted;
        total.snapshot_ns += duration_ns(snapshot.snapshot_elapsed);
        total.footer_ns += duration_ns(snapshot.footer_elapsed);
        total.read_ns += duration_ns(snapshot.read_elapsed);
        merge_lanes(
            &mut total,
            snapshot.lanes,
            snapshot.lane_rows_min,
            snapshot.lane_rows_max,
            duration_ns(snapshot.lane_elapsed_min),
            duration_ns(snapshot.lane_elapsed_max),
        );
        total
    })
}

/// Folds one finished task into the stage list, kept ordered by stage: the
/// stage is created on first sight, its tasks stay in partition order and
/// its counters are refreshed. The final record and the live record while
/// the statement runs both go through this, so the numbers a client polls
/// are the ones it reads once the statement finishes.
fn record_stage_task(
    stages: &mut Vec<StageTelemetry>,
    stage_id: u32,
    task_count: usize,
    elapsed_us: u64,
    task: TaskTelemetry,
) {
    let index = match stages.binary_search_by_key(&stage_id, |stage| stage.stage_id) {
        Ok(index) => index,
        Err(index) => {
            stages.insert(
                index,
                StageTelemetry {
                    stage_id,
                    state: "RUNNING",
                    task_count,
                    completed_tasks: 0,
                    elapsed_us,
                    tasks: Vec::new(),
                },
            );
            index
        }
    };
    let stage = &mut stages[index];
    stage.tasks.push(task);
    stage
        .tasks
        .sort_unstable_by_key(|task| task.partition_index);
    stage.completed_tasks = stage.tasks.len();
    stage.task_count = task_count;
    stage.elapsed_us = elapsed_us;
    stage.state = if stage.completed_tasks >= stage.task_count {
        "FINISHED"
    } else {
        "RUNNING"
    };
}

/// A finished task lands on the record while the statement runs: its
/// stage's counters, its telemetry, and the scan totals over every task
/// finished so far by the aggregation the final record uses.
/// `scan_metrics_complete` stays false until the statement finishes.
fn merge_task_into_record(
    record: &mut QueryRecord,
    stage_id: u32,
    task_count: usize,
    stage_elapsed_us: u64,
    task: TaskTelemetry,
) {
    record_stage_task(
        &mut record.stages,
        stage_id,
        task_count,
        stage_elapsed_us,
        task,
    );
    record.scans = distributed_scan_telemetry(&record.stages).0;
}

/// A record that is no longer running (canceled, failed, or already
/// committed) is left alone.
async fn publish_task_completion(
    query_id: &str,
    stage_id: u32,
    task_count: usize,
    stage_elapsed_us: u64,
    task: TaskTelemetry,
) {
    if let Some(record) = QUERY_STORE.write().await.queries.get_mut(query_id)
        && matches!(record.state, QueryState::Running)
    {
        merge_task_into_record(record, stage_id, task_count, stage_elapsed_us, task);
    }
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
            total.decoded_batch_cache_hits += scan.decoded_batch_cache_hits;
            total.decoded_batch_cache_misses += scan.decoded_batch_cache_misses;
            total.decoded_batch_cache_evictions += scan.decoded_batch_cache_evictions;
            total.decoded_batch_cache_singleflight_waits +=
                scan.decoded_batch_cache_singleflight_waits;
            total.row_groups_considered += scan.row_groups_considered;
            total.row_groups_selected += scan.row_groups_selected;
            total.rows_selected += scan.rows_selected;
            total.rows_emitted += scan.rows_emitted;
            total.compressed_bytes_selected += scan.compressed_bytes_selected;
            total.compressed_bytes_read += scan.compressed_bytes_read;
            total.row_filter_rows_examined += scan.row_filter_rows_examined;
            total.row_filter_rows_admitted += scan.row_filter_rows_admitted;
            total.batches_emitted += scan.batches_emitted;
            total.snapshot_ns += scan.snapshot_ns;
            total.footer_ns += scan.footer_ns;
            total.read_ns += scan.read_ns;
            merge_lanes(
                &mut total,
                scan.lanes,
                scan.lane_rows_min,
                scan.lane_rows_max,
                scan.lane_read_ns_min,
                scan.lane_read_ns_max,
            );
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
            decoded_batch_cache_hits: total.decoded_batch_cache_hits,
            decoded_batch_cache_misses: total.decoded_batch_cache_misses,
            decoded_batch_cache_evictions: total.decoded_batch_cache_evictions,
            decoded_batch_cache_singleflight_waits: total.decoded_batch_cache_singleflight_waits,
            row_groups_considered: total.row_groups_considered,
            row_groups_read: total.row_groups_selected,
            row_groups_pruned: total
                .row_groups_considered
                .saturating_sub(total.row_groups_selected),
            rows_selected: total.rows_selected,
            rows_emitted: total.rows_emitted,
            batches_emitted: total.batches_emitted,
            compressed_bytes_selected: total.compressed_bytes_selected,
            compressed_bytes_read: total.compressed_bytes_read,
            row_filter_rows_examined: total.row_filter_rows_examined,
            row_filter_rows_admitted: total.row_filter_rows_admitted,
            snapshot_ns: total.snapshot_ns,
            footer_ns: total.footer_ns,
            read_ns: total.read_ns,
            rows_per_second,
            compressed_bytes_per_second,
            lanes: total.lanes,
            lane_rows_min: total.lane_rows_min,
            lane_rows_max: total.lane_rows_max,
            lane_read_ns_min: total.lane_read_ns_min,
            lane_read_ns_max: total.lane_read_ns_max,
        }],
        true,
    )
}

fn duration_ns(duration: std::time::Duration) -> u64 {
    duration.as_nanos().try_into().unwrap_or(u64::MAX)
}

/// The one-based line and column a parser error names. `sqlparser` ends
/// its messages with ` at Line: N, Column: M` when the failing token has a
/// location; the error reaches the API as text, so the position is read
/// back from the message. A message without one yields `None`.
fn parse_error_position(message: &str) -> Option<(u64, u64)> {
    let (_, location) = message.rsplit_once(" at Line: ")?;
    let (line, column) = location.split_once(", Column: ")?;
    Some((line.parse().ok()?, column.parse().ok()?))
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
        record.stages.clear();
    }
}

/// A coordinator state over an empty in-memory catalog store, for the
/// catalog API and catalog statement tests.
#[cfg(test)]
pub(crate) fn catalog_test_state() -> crate::AppState {
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
        result_cache: crate::result_cache::ResultCache::new(
            1 << 20,
            std::time::Duration::from_secs(60),
        ),
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
        .unwrap()
        .with_queue_limit(config.memory_admission_queue),
        product_transactions: crate::transaction_api::TransactionRegistry::disabled(),
        config,
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_spooled_exchange_input_reserves_the_batch_it_holds_not_the_spool() {
        use arrow::array::Int64Array;
        use arrow::datatypes::{DataType, Field, Schema};
        use arrow::record_batch::RecordBatch;
        use kaveon_core::BatchOperator;
        // Four batches of 128 KiB each in one spool: a budget that holds one
        // batch and a half must read the whole payload, because only the
        // batch handed out is in memory.
        let schema = Arc::new(Schema::new(vec![Field::new("v", DataType::Int64, false)]));
        let mut bytes = Vec::new();
        {
            let mut writer =
                arrow::ipc::writer::StreamWriter::try_new(&mut bytes, &schema).unwrap();
            for round in 0..4i64 {
                let values = (0..16_384).map(|i| i + round).collect::<Vec<_>>();
                let batch = RecordBatch::try_new(
                    Arc::clone(&schema),
                    vec![Arc::new(Int64Array::from(values))],
                )
                .unwrap();
                writer.write(&batch).unwrap();
            }
            writer.finish().unwrap();
        }
        let payload = crate::transport::ArrowPayload::from_ipc_bytes(&bytes).unwrap();
        let batch_bytes = 16_384 * 8;
        assert!(payload.bytes() > 3 * batch_bytes);
        let pool = kaveon_core::QueryMemoryPool::new("spooled-input", (batch_bytes * 3 / 2) as u64)
            .unwrap();
        let mut input = super::DiskExchangeInput::new(
            payload.schema(),
            std::collections::VecDeque::from([payload]),
            pool.operator("prefetched-exchanges").unwrap(),
            Arc::new(super::ExchangeDecodeMetrics::default()),
        );
        let mut rows = 0;
        while let Some(batch) = input.next_batch().unwrap() {
            rows += batch.num_rows();
            let held = pool.snapshot().current_bytes;
            assert!(
                held >= batch_bytes as u64 && held <= (batch_bytes * 3 / 2) as u64,
                "reserved {held} for a {batch_bytes}-byte batch"
            );
        }
        assert_eq!(rows, 4 * 16_384);
        drop(input);
        assert_eq!(pool.snapshot().current_bytes, 0);
    }

    /// A spool of three batches, the first a quarter the size of the other
    /// two: the payload, the bytes of a large batch, of the small one, and
    /// each batch's first value.
    fn three_batch_spool() -> (crate::transport::ArrowPayload, u64, u64, Vec<i64>) {
        use arrow::array::Int64Array;
        use arrow::datatypes::{DataType, Field, Schema};
        use arrow::record_batch::RecordBatch;
        let schema = Arc::new(Schema::new(vec![Field::new("v", DataType::Int64, false)]));
        let mut bytes = Vec::new();
        let mut firsts = Vec::new();
        {
            let mut writer =
                arrow::ipc::writer::StreamWriter::try_new(&mut bytes, &schema).unwrap();
            for round in 0..3i64 {
                let rows = if round == 0 { 4_096 } else { 16_384 };
                let values = (0..rows).map(|i| i + round * 100_000).collect::<Vec<_>>();
                firsts.push(values[0]);
                let batch = RecordBatch::try_new(
                    Arc::clone(&schema),
                    vec![Arc::new(Int64Array::from(values))],
                )
                .unwrap();
                writer.write(&batch).unwrap();
            }
            writer.finish().unwrap();
        }
        (
            crate::transport::ArrowPayload::from_ipc_bytes(&bytes).unwrap(),
            16_384 * 8,
            4_096 * 8,
            firsts,
        )
    }

    fn first_value(batch: &arrow::record_batch::RecordBatch) -> i64 {
        batch
            .column(0)
            .as_any()
            .downcast_ref::<arrow::array::Int64Array>()
            .unwrap()
            .value(0)
    }

    /// A batch whose reservation the budget refuses is offered again on
    /// the next call — once: the IPC reader has moved past it, and the
    /// batch is neither lost with the error nor handed out twice. Read as
    /// a `BatchOperator`, which holds each batch until the next call.
    #[test]
    fn a_spooled_exchange_input_offers_a_refused_batch_again_exactly_once() {
        use kaveon_core::BatchOperator;
        let (payload, batch_bytes, first_bytes, firsts) = three_batch_spool();
        let budget = batch_bytes * 2;
        let pool = kaveon_core::QueryMemoryPool::new("re-offered", budget).unwrap();
        let metrics = Arc::new(super::ExchangeDecodeMetrics::default());
        let mut input = super::DiskExchangeInput::new(
            payload.schema(),
            std::collections::VecDeque::from([payload]),
            pool.operator("prefetched-exchanges").unwrap(),
            Arc::clone(&metrics),
        );
        let first = input.next_batch().unwrap().unwrap();
        assert_eq!(first_value(&first), firsts[0]);
        assert_eq!(pool.snapshot().current_bytes, first_bytes);
        // Something else takes all that is left beside the small first
        // batch: once that is released, what is free is a quarter of the
        // next batch, decoded and refused.
        let ballast = pool
            .operator("ballast")
            .unwrap()
            .reserve(budget - first_bytes)
            .unwrap();
        let refused = input.next_batch().unwrap_err();
        assert!(
            matches!(&refused, kaveon_core::KaveonError::MemoryLimit(message)
                if message.contains("operator 'prefetched-exchanges' cannot reserve")),
            "{refused}"
        );
        assert_eq!(
            metrics.batches.load(std::sync::atomic::Ordering::Acquire),
            2,
            "decoded once"
        );
        // Refused again while the ballast holds; the batch is still kept.
        let refused = input.next_batch().unwrap_err();
        assert!(matches!(refused, kaveon_core::KaveonError::MemoryLimit(_)));
        assert_eq!(
            metrics.batches.load(std::sync::atomic::Ordering::Acquire),
            2
        );
        drop(ballast);
        // The kept batch, then the third, then the end: three in all.
        let second = input.next_batch().unwrap().unwrap();
        assert_eq!(first_value(&second), firsts[1]);
        assert_eq!(pool.snapshot().current_bytes, batch_bytes);
        let third = input.next_batch().unwrap().unwrap();
        assert_eq!(first_value(&third), firsts[2]);
        assert!(input.next_batch().unwrap().is_none());
        assert_eq!(
            metrics.batches.load(std::sync::atomic::Ordering::Acquire),
            3
        );
        drop(input);
        assert_eq!(pool.snapshot().current_bytes, 0);
    }

    /// The same input read as a thread source hands each batch over with
    /// the reservation holding it — the one charge for the batch while
    /// it is in flight — and keeps a refused batch the same way.
    #[test]
    fn a_spooled_exchange_input_hands_its_reservation_over_with_the_batch() {
        use kaveon_exec::local_parallel::ThreadSource;
        let (payload, batch_bytes, first_bytes, firsts) = three_batch_spool();
        let pool = kaveon_core::QueryMemoryPool::new("handed-over", batch_bytes * 2).unwrap();
        let mut input = super::DiskExchangeInput::new(
            payload.schema(),
            std::collections::VecDeque::from([payload]),
            pool.operator("prefetched-exchanges").unwrap(),
            Arc::new(super::ExchangeDecodeMetrics::default()),
        );
        let first = ThreadSource::next_batch(&mut input).unwrap().unwrap();
        assert_eq!(first_value(&first.batch), firsts[0]);
        assert_eq!(first.memory.as_ref().unwrap().bytes(), first_bytes);
        // The source holds nothing of its own: the reservation is the
        // batch's holder's, and the two held together leave the third
        // refused until one of them is dropped.
        let second = ThreadSource::next_batch(&mut input).unwrap().unwrap();
        assert_eq!(first_value(&second.batch), firsts[1]);
        assert_eq!(pool.snapshot().current_bytes, first_bytes + batch_bytes);
        let refused = ThreadSource::next_batch(&mut input).unwrap_err();
        assert!(matches!(refused, kaveon_core::KaveonError::MemoryLimit(_)));
        drop(first);
        assert_eq!(pool.snapshot().current_bytes, batch_bytes);
        let third = ThreadSource::next_batch(&mut input).unwrap().unwrap();
        assert_eq!(first_value(&third.batch), firsts[2]);
        assert!(ThreadSource::next_batch(&mut input).unwrap().is_none());
        drop(second);
        drop(third);
        drop(input);
        assert_eq!(pool.snapshot().current_bytes, 0);
    }

    fn statement_request(query: &str, settings: serde_json::Value) -> super::StatementRequest {
        super::StatementRequest {
            query: query.into(),
            catalog: None,
            schema: None,
            source: None,
            client: None,
            user: None,
            time_zone: None,
            client_tags: vec![],
            result_delivery: None,
            settings: settings.as_object().cloned(),
        }
    }

    #[test]
    fn request_settings_fold_the_object_and_the_set_session_prefix() {
        let config = crate::config::ServerConfig {
            query_memory_limit_bytes: 1 << 30,
            ..crate::config::ServerConfig::default()
        };
        let request = statement_request(
            "SET SESSION result_cache = false; SET SESSION time_zone = 'UTC'; SELECT 1;",
            serde_json::json!({"query_memory_limit_bytes": 1 << 20}),
        );
        let (settings, sql, time_zone) = super::request_settings(&request, &config).unwrap();
        assert_eq!(sql, "SELECT 1");
        assert_eq!(time_zone.as_deref(), Some("UTC"));
        assert_eq!(settings.result_cache, Some(false));
        assert_eq!(settings.query_memory_limit_bytes, Some(1 << 20));
        assert_eq!(settings.query_memory_limit_bytes(&config), 1 << 20);

        // The record serialises the settings only when the statement set some.
        let plain = statement_request("SELECT 1", serde_json::Value::Null);
        let (settings, sql, time_zone) = super::request_settings(&plain, &config).unwrap();
        assert!(settings.is_default());
        assert_eq!(sql, "SELECT 1");
        assert!(time_zone.is_none());

        let unknown = statement_request("SELECT 1", serde_json::json!({"spill_bytes": 1}));
        let error = super::request_settings(&unknown, &config).unwrap_err();
        assert_eq!(error.0, "unknown setting 'spill_bytes'");

        let raised = statement_request(
            "SELECT 1",
            serde_json::json!({"query_memory_limit_bytes": (1u64 << 30) + 1}),
        );
        assert!(super::request_settings(&raised, &config).is_err());

        let mut conflicting = statement_request(
            "SET SESSION time_zone = 'UTC'; SELECT 1",
            serde_json::Value::Null,
        );
        conflicting.time_zone = Some("Europe/Dublin".into());
        let error = super::request_settings(&conflicting, &config).unwrap_err();
        assert!(error.0.contains("time_zone"), "{error}");

        let alone = statement_request("SET SESSION result_cache = false", serde_json::Value::Null);
        let error = super::request_settings(&alone, &config).unwrap_err();
        assert!(error.0.contains("stateless"), "{error}");
    }

    #[test]
    fn task_requests_carry_the_statement_settings_and_lower_the_worker_limit() {
        let config = crate::config::ServerConfig {
            query_memory_limit_bytes: 1 << 30,
            ..crate::config::ServerConfig::default()
        };
        let settings = crate::settings::QuerySettings {
            query_memory_limit_bytes: Some(1 << 20),
            local_parallelism: Some(1),
            result_cache: None,
            admission_wait_seconds: None,
        };
        let request = super::TaskRequest {
            query_id: "query-settings".into(),
            stage_id: 0,
            attempt: 0,
            query: String::new(),
            catalog: "kaveon".into(),
            schema: "default".into(),
            catalog_snapshot_id: None,
            partition_index: 0,
            partition_count: 1,
            fragment: None,
            execution_partition: None,
            exchange_inputs: vec![],
            exchange_outputs: vec![],
            settings,
        };
        let wire = serde_json::to_value(&request).unwrap();
        assert_eq!(
            wire["settings"],
            serde_json::json!({"query_memory_limit_bytes": 1 << 20, "local_parallelism": 1})
        );
        let decoded: super::TaskRequest = serde_json::from_value(wire).unwrap();
        assert_eq!(decoded.settings.query_memory_limit_bytes(&config), 1 << 20);
        // An older coordinator sends no settings: the worker's own limit stands.
        let legacy: super::TaskRequest = serde_json::from_value(serde_json::json!({
            "query_id": "q", "stage_id": 0, "attempt": 0
        }))
        .unwrap();
        assert!(legacy.settings.is_default());
        assert_eq!(legacy.settings.query_memory_limit_bytes(&config), 1 << 30);
        // A statement cannot raise the worker's limit through the task.
        let raised = crate::settings::QuerySettings {
            query_memory_limit_bytes: Some(1 << 40),
            ..crate::settings::QuerySettings::default()
        };
        assert_eq!(raised.query_memory_limit_bytes(&config), 1 << 30);
    }

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
        AnalyzeStatement, ColumnInfo, DistinctColumns, MergeOperation, StatisticsStatement,
        TaskRequest, TaskResponse, aggregate_merge_contract, await_task_memory, capabilities,
        catalog_test_state, collect_join_statistics_tables, decode_arrow_stream,
        durable_relation_statistics, encode_arrow_stream, exact_metadata_count_plan,
        exact_source_statistics, execute_analyze, general_distributed_eligible, iso_utc_ms,
        merge_partial_aggregates, mutation_actor, parse_analyze_statement,
        parse_statistics_statement, statistics_diagnostics, task_request_from_dispatch,
        top_n_merge_contract, transaction_api_guidance, validate_replacement,
    };
    use crate::security::Role;
    use arrow::array::{Int64Array, StringArray};
    use axum::http::StatusCode;
    use sha2::{Digest, Sha256};

    fn analyze(sql: &str) -> Result<AnalyzeStatement, String> {
        parse_analyze_statement(sql).expect("an ANALYZE statement")
    }

    fn analyze_of(table: &str, distinct: DistinctColumns) -> Result<AnalyzeStatement, String> {
        Ok(AnalyzeStatement {
            table: table.into(),
            distinct,
        })
    }

    #[test]
    fn analyze_parser_accepts_bounded_table_names_only() {
        assert_eq!(
            analyze("ANALYZE \"sales\".\"orders\""),
            analyze_of("sales.orders", DistinctColumns::None)
        );
        assert_eq!(
            analyze("analyze lake.sales.orders"),
            analyze_of("lake.sales.orders", DistinctColumns::None)
        );
        assert_eq!(parse_analyze_statement("ANALYZER orders"), None);
        assert_eq!(parse_analyze_statement("SELECT 1"), None);
        assert!(analyze("ANALYZE orders WHERE true").is_err());
        assert!(analyze("ANALYZE a.b.c.d").is_err());
        assert!(analyze("ANALYZE ").is_err());
    }

    #[test]
    fn analyze_parser_reads_the_with_properties() {
        let named = |names: &[&str]| {
            DistinctColumns::Named(names.iter().map(|n| (*n).to_owned()).collect())
        };
        assert_eq!(
            analyze("ANALYZE orders WITH (distinct = true)"),
            analyze_of("orders", DistinctColumns::All)
        );
        assert_eq!(
            analyze("analyze lake.sales.orders with(DISTINCT=TRUE)"),
            analyze_of("lake.sales.orders", DistinctColumns::All)
        );
        assert_eq!(
            analyze("ANALYZE orders WITH (distinct = false)"),
            analyze_of("orders", DistinctColumns::None)
        );
        assert_eq!(
            analyze("ANALYZE \"sales\".\"orders\" WITH (columns = ARRAY['a', 'b'])"),
            analyze_of("sales.orders", named(&["a", "b"]))
        );
        assert_eq!(
            analyze("ANALYZE orders WITH ( Columns = array[ 'Region Name' , 'it''s' ] )"),
            analyze_of("orders", named(&["Region Name", "it's"]))
        );
        assert_eq!(
            analyze("ANALYZE orders\n  WITH (\n    columns = ARRAY['a,b']\n  )"),
            analyze_of("orders", named(&["a,b"]))
        );
        let error = |sql: &str| analyze(sql).unwrap_err();
        assert_eq!(
            error("ANALYZE orders WITH (distinct = true, columns = ARRAY['a'])"),
            "ANALYZE takes distinct or columns, not both"
        );
        assert_eq!(
            error("ANALYZE orders WITH (distinct = false, columns = ARRAY['a'])"),
            "ANALYZE takes distinct or columns, not both"
        );
        assert_eq!(
            error("ANALYZE orders WITH (distinct = yes)"),
            "ANALYZE property distinct must be true or false, not yes"
        );
        assert_eq!(
            error("ANALYZE orders WITH (sample = 1)"),
            "unknown ANALYZE property 'sample'; the properties are distinct and columns"
        );
        assert_eq!(
            error("ANALYZE orders WITH (columns = ARRAY[])"),
            "ANALYZE property columns names no column"
        );
        assert_eq!(
            error("ANALYZE orders WITH (columns = ARRAY['a', 'a'])"),
            "ANALYZE property columns names 'a' twice"
        );
        assert_eq!(
            error("ANALYZE orders WITH (distinct = true, distinct = true)"),
            "ANALYZE property distinct is given twice"
        );
        assert!(error("ANALYZE orders WITH (columns = ARRAY[a])").contains("single-quoted"));
        assert!(error("ANALYZE orders WITH (columns = ARRAY['a')").contains("unbalanced"));
        assert!(error("ANALYZE orders WITH (columns = ARRAY['a'] extra)").contains("ARRAY"));
        assert!(error("ANALYZE orders WITH (columns = 'a')").contains("ARRAY"));
        assert!(error("ANALYZE orders WITH (distinct = true").contains("WITH"));
        assert!(error("ANALYZE orders USING (distinct = true)").contains("WITH"));
        assert!(error("ANALYZE orders WITH ()").contains("empty entry"));
        assert!(error("ANALYZE orders WITH (distinct = true,)").contains("empty entry"));
        assert!(error("ANALYZE orders WITH (columns = ARRAY['a)").contains("unterminated"));
        assert!(error("ANALYZE orders WITH (distinct)").contains("key = value"));
    }

    #[test]
    fn statistics_statement_parser_accepts_bounded_table_names_only() {
        assert_eq!(
            parse_statistics_statement("SHOW STATS FOR lake.sales.orders"),
            Some(StatisticsStatement::ShowStats("lake.sales.orders".into()))
        );
        assert_eq!(
            parse_statistics_statement("show stats for \"sales\".\"orders\""),
            Some(StatisticsStatement::ShowStats("sales.orders".into()))
        );
        assert_eq!(
            parse_statistics_statement("DESCRIBE DETAIL orders"),
            Some(StatisticsStatement::DescribeDetail("orders".into()))
        );
        assert_eq!(
            parse_statistics_statement("desc detail lake.sales.orders"),
            Some(StatisticsStatement::DescribeDetail(
                "lake.sales.orders".into()
            ))
        );
        assert_eq!(
            parse_statistics_statement("SHOW STATS FOR (SELECT 1)"),
            None
        );
        assert_eq!(parse_statistics_statement("SHOW STATS orders"), None);
        assert_eq!(parse_statistics_statement("DESCRIBE orders"), None);
        assert_eq!(parse_statistics_statement("DESCRIBE DETAIL a.b.c.d"), None);
        assert_eq!(
            parse_statistics_statement("SHOW STATS FOR orders WHERE x"),
            None
        );
    }

    #[tokio::test]
    async fn statistics_statements_read_the_published_document() {
        let (state, commit, directory) = analyze_test_state().await;
        let admin = crate::security::Identity {
            principal: "admin".into(),
            display_identity: None,
            role: Role::Admin,
        };
        let analyst = crate::security::Identity {
            principal: "analyst".into(),
            display_identity: None,
            role: Role::Analyst,
        };
        let file_bytes = std::fs::metadata(directory.join("orders.parquet"))
            .unwrap()
            .len();

        // Before ANALYZE: no statistics to show, but the detail answers from
        // a fresh metadata read.
        let (status, body) = submit(
            &state,
            &analyst,
            "SHOW STATS FOR orders",
            serde_json::Value::Null,
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
        assert_eq!(body["code"], "STATISTICS_UNAVAILABLE");
        assert_eq!(
            body["error"],
            "no statistics for lake.sales.orders; run ANALYZE lake.sales.orders"
        );
        let failed = record(body["id"].as_str().unwrap(), &analyst).await;
        assert_eq!(failed["state"], "FAILED");

        let (status, body) = submit(
            &state,
            &analyst,
            "DESCRIBE DETAIL orders",
            serde_json::Value::Null,
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        let names = body["columns"]
            .as_array()
            .unwrap()
            .iter()
            .map(|column| column["name"].as_str().unwrap().to_owned())
            .collect::<Vec<_>>();
        assert_eq!(
            names,
            [
                "format",
                "location",
                "created_at",
                "last_modified",
                "num_files",
                "size_in_bytes",
                "row_count",
                "delta_version",
                "partition_columns",
                "analyzed_at",
                "catalog_snapshot"
            ]
        );
        let row = &body["data"][0];
        assert_eq!(row[0], "parquet");
        assert!(row[1].as_str().unwrap().ends_with("orders.parquet"));
        assert!(row[2].is_null());
        assert!(row[3].as_str().unwrap().ends_with('Z'));
        assert_eq!(row[4], 1);
        assert_eq!(row[5], file_bytes);
        assert!(row[6].is_null());
        assert!(row[7].is_null());
        assert_eq!(row[8], "");
        assert!(row[9].is_null());
        assert!(row[10].is_null());
        assert_eq!(body["data"].as_array().unwrap().len(), 1);

        // ANALYZE keeps its result and writes the version 2 document.
        let (status, body) =
            submit(&state, &analyst, "ANALYZE orders", serde_json::Value::Null).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
        let (status, body) =
            submit(&state, &admin, "ANALYZE orders", serde_json::Value::Null).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(
            body["data"],
            serde_json::json!([["lake.sales.orders", 3, 0]])
        );
        assert_eq!(body["columns"][2]["name"], "distinct_columns");
        let snapshot = commit.read_current().await.unwrap();
        let stored = &snapshot.table_statistics["lake.sales.orders"];
        assert_eq!(stored.row_count, 3);
        assert!(stored.document.path.starts_with("statistics/"));
        let document: serde_json::Value =
            serde_json::from_slice(&commit.fetch_immutable_file(&stored.document).await.unwrap())
                .unwrap();
        assert_eq!(document["version"], 2);
        assert_eq!(document["table"], "lake.sales.orders");
        assert!(document["analyzed_at_ms"].as_i64().unwrap() > 0);
        assert_eq!(
            document["catalog_snapshot_sha256"],
            format!("{:x}", Sha256::digest(b"sha256:catalog-one"))
        );
        assert_eq!(
            document["source_identity_sha256"],
            stored.source_identity_sha256
        );
        assert_eq!(document["format"], "parquet");
        assert!(
            document["location"]
                .as_str()
                .unwrap()
                .ends_with("orders.parquet")
        );
        assert!(document["delta_version"].is_null());
        assert_eq!(document["row_count"], 3);
        assert_eq!(document["file_count"], 1);
        assert_eq!(document["row_group_count"], 1);
        assert_eq!(document["compressed_bytes"], file_bytes);
        assert!(document["uncompressed_bytes"].as_u64().unwrap() > 0);
        assert!(document["last_modified_ms"].as_i64().unwrap() > 0);
        assert_eq!(document["partition_columns"], serde_json::json!([]));
        let column = &document["columns"][0];
        assert_eq!(column["name"], "id");
        assert_eq!(column["type"], "bigint");
        assert_eq!(column["nulls"], 0);
        assert_eq!(column["min"], 1);
        assert_eq!(column["max"], 3);
        assert!(column["compressed_bytes"].as_u64().unwrap() > 0);
        assert!(column["distinct"].is_null());
        assert_eq!(document["columns"].as_array().unwrap().len(), 1);

        // SHOW STATS FOR: one row per column and the summary row.
        let (status, body) = submit(
            &state,
            &analyst,
            "SHOW STATS FOR lake.sales.orders",
            serde_json::Value::Null,
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        let columns = body["columns"]
            .as_array()
            .unwrap()
            .iter()
            .map(|column| {
                (
                    column["name"].as_str().unwrap().to_owned(),
                    column["type"].as_str().unwrap().to_owned(),
                )
            })
            .collect::<Vec<_>>();
        assert_eq!(
            columns,
            [
                ("column_name".to_owned(), "VARCHAR".to_owned()),
                ("data_type".into(), "VARCHAR".into()),
                ("data_size".into(), "BIGINT".into()),
                ("nulls_fraction".into(), "DOUBLE".into()),
                ("distinct_values_count".into(), "BIGINT".into()),
                ("low_value".into(), "VARCHAR".into()),
                ("high_value".into(), "VARCHAR".into()),
                ("row_count".into(), "BIGINT".into()),
                ("analyzed_at".into(), "TIMESTAMP".into()),
            ]
        );
        let analyzed_at = iso_utc_ms(document["analyzed_at_ms"].as_i64());
        assert!(analyzed_at.as_str().unwrap().ends_with('Z'));
        assert_eq!(
            body["data"],
            serde_json::json!([
                [
                    "id",
                    "bigint",
                    column["compressed_bytes"],
                    0.0,
                    null,
                    "1",
                    "3",
                    null,
                    analyzed_at
                ],
                [
                    null,
                    null,
                    file_bytes,
                    null,
                    null,
                    null,
                    null,
                    3,
                    analyzed_at
                ]
            ])
        );
        let finished = record(body["id"].as_str().unwrap(), &analyst).await;
        assert_eq!(finished["state"], "FINISHED");
        assert_eq!(finished["columns"][8]["name"], "analyzed_at");
        assert_eq!(finished["rows"], body["data"]);

        // DESCRIBE DETAIL after ANALYZE answers from the document.
        let (status, body) = submit(
            &state,
            &analyst,
            "DESCRIBE DETAIL lake.sales.orders",
            serde_json::Value::Null,
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        let row = &body["data"][0];
        assert_eq!(row[0], "parquet");
        assert_eq!(row[4], 1);
        assert_eq!(row[5], file_bytes);
        assert_eq!(row[6], 3);
        assert!(row[7].is_null());
        assert_eq!(row[8], "");
        assert_eq!(row[9], analyzed_at);
        assert_eq!(row[10], document["catalog_snapshot_sha256"]);

        let (status, body) = submit(
            &state,
            &analyst,
            "SHOW STATS FOR lake.sales.missing",
            serde_json::Value::Null,
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
        assert_eq!(body["code"], "TABLE_NOT_FOUND");
        std::fs::remove_dir_all(directory).unwrap();
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
        let admission = kaveon_core::MemoryAdmissionController::new(1_024)
            .unwrap()
            .with_queue_limit(4);
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
        let admission = kaveon_core::MemoryAdmissionController::new(1_024)
            .unwrap()
            .with_queue_limit(4);
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

    #[tokio::test]
    async fn a_worker_task_is_refused_only_by_a_full_queue() {
        let admission = kaveon_core::MemoryAdmissionController::new(1_024)
            .unwrap()
            .with_queue_limit(1);
        let _occupied = admission.admit("running", 1_024).unwrap();
        let lifecycle = crate::lifecycle::WorkerLifecycle::<()>::default();
        let cancellation = lifecycle.cancellations.token("waiting-query").unwrap();
        let controller = admission.clone();
        let waiting = cancellation.clone();
        let _first = tokio::spawn(async move {
            await_task_memory(&controller, "first".into(), 1_024, &waiting).await
        });
        tokio::task::yield_now().await;
        let error = await_task_memory(&admission, "second".into(), 1_024, &cancellation)
            .await
            .unwrap_err();
        assert!(error.contains("queue is full"), "{error}");
        assert_eq!(admission.stats().rejected, 1);
    }

    /// One statement's budget fills the coordinator; the next arrivals
    /// queue, run once it is released, and record the wait.
    #[tokio::test]
    async fn a_statement_waits_for_admission_then_runs_and_records_the_wait() {
        let (state, _commit, directory) = admission_test_state(2).await;
        let occupied = state
            .memory_admission
            .admit("occupying", state.config.query_memory_limit_bytes)
            .unwrap();
        let analyst = crate::security::Identity {
            principal: "analyst".into(),
            display_identity: None,
            role: Role::Analyst,
        };
        let sql = "SELECT id FROM orders WHERE id > 1 ORDER BY id";
        let state = Arc::new(state);
        let submitting = {
            let state = state.clone();
            let analyst = analyst.clone();
            tokio::spawn(async move {
                submit(
                    &state,
                    &analyst,
                    sql,
                    serde_json::json!({"result_cache": false}),
                )
                .await
            })
        };
        // Queued: visible, not yet running, and not immediately refused.
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        assert!(!submitting.is_finished());
        let queued = {
            let store = super::QUERY_STORE.read().await;
            store
                .queries
                .values()
                .find(|record| {
                    record.sql == sql && matches!(record.state, super::QueryState::Queued)
                })
                .map(|record| record.id.clone())
        }
        .expect("a queued statement is in the history");
        let stats = state.memory_admission.stats();
        assert_eq!((stats.queue_depth, stats.queued), (1, 1));
        let node = json_body(
            super::get_node(axum::extract::State(state.clone()))
                .await
                .into_response(),
        )
        .await;
        assert_eq!(node["admission"]["queue_depth"], 1);
        assert_eq!(node["admission"]["queue_limit"], 2);

        drop(occupied);
        let (status, body) = submitting.await.unwrap();
        assert_eq!(status, axum::http::StatusCode::OK, "{body}");
        assert_eq!(body["data"], serde_json::json!([[2], [3]]));
        assert_eq!(body["id"], queued);
        let record = record(&queued, &analyst).await;
        assert_eq!(record["state"], "FINISHED");
        let waited = record["admission_wait_ms"].as_u64().unwrap();
        assert!((150..5_000).contains(&waited), "waited {waited} ms");
        let stats = state.memory_admission.stats();
        assert_eq!(
            (stats.queue_depth, stats.admitted, stats.rejected),
            (0, 2, 0)
        );
        std::fs::remove_dir_all(directory).unwrap();
    }

    /// The wait expires: HTTP 429 with the wait recorded, the record failed;
    /// a statement that asked not to wait is refused on arrival; a full
    /// queue is refused on arrival.
    #[tokio::test]
    async fn an_expired_admission_wait_is_a_429_with_the_wait_recorded() {
        let (state, _commit, directory) = admission_test_state(1).await;
        let _occupied = state
            .memory_admission
            .admit("occupying", state.config.query_memory_limit_bytes)
            .unwrap();
        let analyst = crate::security::Identity {
            principal: "analyst".into(),
            display_identity: None,
            role: Role::Analyst,
        };
        let sql = "SELECT id FROM orders WHERE id > 0 ORDER BY id";
        let state = Arc::new(state);

        let started = std::time::Instant::now();
        let (status, body) = submit(
            &state,
            &analyst,
            sql,
            serde_json::json!({"admission_wait_seconds": 1}),
        )
        .await;
        assert_eq!(status, axum::http::StatusCode::TOO_MANY_REQUESTS, "{body}");
        assert_eq!(body["code"], "MEMORY_ADMISSION_REJECTED");
        assert!(started.elapsed() >= std::time::Duration::from_secs(1));
        let waited = body["admission_wait_ms"].as_u64().unwrap();
        assert!(waited >= 1_000, "{body}");
        assert!(
            body["error"]
                .as_str()
                .unwrap()
                .contains("admission wait of 1 s expired"),
            "{body}"
        );
        let failed = {
            let store = super::QUERY_STORE.read().await;
            store
                .queries
                .values()
                .find(|record| {
                    record.sql == sql && matches!(record.state, super::QueryState::Failed)
                })
                .cloned()
        }
        .expect("the expired statement stays in the history as failed");
        assert_eq!(failed.admission_wait_ms, waited);
        assert!(failed.completed_at_ms > 0);

        // No wait asked: refused at once, nothing queued, no record.
        let (status, body) = submit(
            &state,
            &analyst,
            sql,
            serde_json::json!({"admission_wait_seconds": 0}),
        )
        .await;
        assert_eq!(status, axum::http::StatusCode::TOO_MANY_REQUESTS, "{body}");
        assert_eq!(body["admission_wait_ms"], 0);
        let stats = state.memory_admission.stats();
        assert_eq!((stats.queue_depth, stats.rejected), (0, 2));

        // The queue holds one: the second arrival is refused on arrival
        // while the first waits.
        let waiting = {
            let state = state.clone();
            let analyst = analyst.clone();
            tokio::spawn(async move {
                submit(
                    &state,
                    &analyst,
                    sql,
                    serde_json::json!({"admission_wait_seconds": 1}),
                )
                .await
            })
        };
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        let (status, body) = submit(
            &state,
            &analyst,
            sql,
            serde_json::json!({"admission_wait_seconds": 1}),
        )
        .await;
        assert_eq!(status, axum::http::StatusCode::TOO_MANY_REQUESTS, "{body}");
        assert!(
            body["error"].as_str().unwrap().contains("queue is full"),
            "{body}"
        );
        assert_eq!(body["admission_wait_ms"], 0);
        let (status, _) = waiting.await.unwrap();
        assert_eq!(status, axum::http::StatusCode::TOO_MANY_REQUESTS);
        assert_eq!(state.memory_admission.stats().rejected, 4);
        std::fs::remove_dir_all(directory).unwrap();
    }

    /// Cancelling a queued statement by ID leaves the queue at once and
    /// answers the submitter with the cancellation.
    #[tokio::test]
    async fn a_queued_statement_can_be_cancelled_by_id() {
        let (state, _commit, directory) = admission_test_state(2).await;
        let occupied = state
            .memory_admission
            .admit("occupying", state.config.query_memory_limit_bytes)
            .unwrap();
        let analyst = crate::security::Identity {
            principal: "analyst".into(),
            display_identity: None,
            role: Role::Analyst,
        };
        let sql = "SELECT id FROM orders WHERE id > 2 ORDER BY id";
        let state = Arc::new(state);
        let submitting = {
            let state = state.clone();
            let analyst = analyst.clone();
            tokio::spawn(
                async move { submit(&state, &analyst, sql, serde_json::Value::Null).await },
            )
        };
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        let queued = {
            let store = super::QUERY_STORE.read().await;
            store
                .queries
                .values()
                .find(|record| {
                    record.sql == sql && matches!(record.state, super::QueryState::Queued)
                })
                .map(|record| record.id.clone())
        }
        .expect("a queued statement is in the history");

        let cancelled = super::cancel_query(
            axum::extract::State(state.clone()),
            axum::Extension(analyst.clone()),
            axum::extract::Path(queued.clone()),
        )
        .await
        .into_response();
        assert_eq!(cancelled.status(), axum::http::StatusCode::NO_CONTENT);
        let (status, body) = submitting.await.unwrap();
        assert_eq!(status, axum::http::StatusCode::CONFLICT, "{body}");
        assert_eq!(body["code"], "QUERY_CANCELED");
        assert_eq!(record(&queued, &analyst).await["state"], "CANCELED");
        let stats = state.memory_admission.stats();
        assert_eq!(
            (stats.queue_depth, stats.withdrawn, stats.rejected),
            (0, 1, 0)
        );
        drop(occupied);
        std::fs::remove_dir_all(directory).unwrap();
    }

    /// The analyze test state with an admission limit of one statement's
    /// budget, a queue of `queue` and a long configured wait.
    async fn admission_test_state(
        queue: usize,
    ) -> (
        crate::AppState,
        kaveon_catalog::product_commit::ProductCatalogCommit,
        std::path::PathBuf,
    ) {
        let (state, commit, directory) = analyze_test_state().await;
        let mut state = Arc::try_unwrap(state).unwrap_or_else(|_| unreachable!());
        state.config.query_memory_limit_bytes = 1 << 20;
        state.config.memory_admission_limit_bytes = 1 << 20;
        state.config.memory_admission_queue = queue;
        state.config.memory_admission_wait_seconds = 60;
        state.memory_admission = kaveon_core::MemoryAdmissionController::new(1 << 20)
            .unwrap()
            .with_queue_limit(queue);
        (state, commit, directory)
    }

    async fn analyze_test_state() -> (
        Arc<crate::AppState>,
        kaveon_catalog::product_commit::ProductCatalogCommit,
        std::path::PathBuf,
    ) {
        let schema = Arc::new(Schema::new(vec![Field::new("id", DataType::Int64, false)]));
        let batch = RecordBatch::try_new(
            schema.clone(),
            vec![Arc::new(Int64Array::from(vec![1, 2, 3]))],
        )
        .unwrap();
        analyze_test_state_over(schema, batch).await
    }

    /// A coordinator over the durable product catalog with one Parquet
    /// table `lake.sales.orders` holding `batch`.
    async fn analyze_test_state_over(
        schema: Arc<Schema>,
        batch: RecordBatch,
    ) -> (
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

    use axum::response::IntoResponse as _;

    async fn json_body(response: axum::response::Response) -> serde_json::Value {
        let body = axum::body::to_bytes(response.into_body(), 16 * 1024 * 1024)
            .await
            .unwrap();
        serde_json::from_slice(&body).unwrap()
    }

    async fn submit(
        state: &Arc<crate::AppState>,
        identity: &crate::security::Identity,
        query: &str,
        settings: serde_json::Value,
    ) -> (axum::http::StatusCode, serde_json::Value) {
        submit_with_delivery(state, identity, query, settings, None).await
    }

    async fn submit_with_delivery(
        state: &Arc<crate::AppState>,
        identity: &crate::security::Identity,
        query: &str,
        settings: serde_json::Value,
        result_delivery: Option<&str>,
    ) -> (axum::http::StatusCode, serde_json::Value) {
        let response = super::submit_statement(
            axum::extract::State(state.clone()),
            axum::Extension(identity.clone()),
            axum::Json(super::StatementRequest {
                query: query.into(),
                catalog: Some("lake".into()),
                schema: Some("sales".into()),
                source: None,
                client: None,
                user: None,
                time_zone: None,
                client_tags: vec![],
                result_delivery: result_delivery.map(str::to_owned),
                settings: settings.as_object().cloned(),
            }),
        )
        .await
        .into_response();
        let status = response.status();
        (status, json_body(response).await)
    }

    async fn record(id: &str, identity: &crate::security::Identity) -> serde_json::Value {
        let response = super::get_query(
            axum::extract::Path(id.to_owned()),
            axum::Extension(identity.clone()),
        )
        .await
        .into_response();
        assert_eq!(response.status(), axum::http::StatusCode::OK);
        json_body(response).await
    }

    #[tokio::test]
    async fn catalog_statements_run_through_the_statement_api() {
        use parquet::arrow::ArrowWriter;
        let directory =
            std::env::temp_dir().join(format!("kaveon-server-ddl-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&directory).unwrap();
        let schema = Arc::new(Schema::new(vec![Field::new("id", DataType::Int64, false)]));
        let batch = RecordBatch::try_new(
            schema.clone(),
            vec![Arc::new(Int64Array::from(vec![1, 2, 3, 4, 5]))],
        )
        .unwrap();
        let mut writer = ArrowWriter::try_new(
            std::fs::File::create(directory.join("orders.parquet")).unwrap(),
            schema.clone(),
            None,
        )
        .unwrap();
        writer.write(&batch).unwrap();
        writer.close().unwrap();

        // An empty coordinator: no catalog, so the session context the
        // request names does not exist yet. Catalog statements still run.
        let state = Arc::new(catalog_test_state());
        let admin = crate::security::Identity {
            principal: "admin".into(),
            display_identity: None,
            role: Role::Admin,
        };
        let analyst = crate::security::Identity {
            principal: "analyst".into(),
            display_identity: None,
            role: Role::Analyst,
        };
        let (status, body) = submit(&state, &analyst, "SELECT 1", serde_json::Value::Null).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(body["code"], "CATALOG_NOT_FOUND");

        let create_catalog = format!(
            "CREATE CATALOG lake WITH (storage = 'local', base_path = '{}')",
            directory.display().to_string().replace('\'', "''")
        );
        let (status, body) =
            submit(&state, &analyst, &create_catalog, serde_json::Value::Null).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
        assert_eq!(body["code"], "FORBIDDEN");
        let (status, body) = submit(&state, &admin, &create_catalog, serde_json::Value::Null).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(body["data"], serde_json::json!([["lake", "created"]]));
        assert_eq!(body["state"], "FINISHED");
        let created = record(body["id"].as_str().unwrap(), &admin).await;
        assert_eq!(created["state"], "FINISHED");
        assert_eq!(created["columns"][0]["name"], "catalog");
        assert_eq!(created["context"]["catalog"], "lake");

        let (status, body) = submit(
            &state,
            &analyst,
            "CREATE SCHEMA lake.sales",
            serde_json::Value::Null,
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        let (status, body) = submit(
            &state,
            &analyst,
            "CREATE TABLE orders WITH (location = 'orders.parquet', format = 'parquet')",
            serde_json::Value::Null,
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(
            body["data"],
            serde_json::json!([["lake.sales.orders", "created"]])
        );

        // The registered table answers queries at once.
        let (status, body) = submit(
            &state,
            &analyst,
            "SELECT COUNT(*) FROM orders",
            serde_json::Value::Null,
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(body["data"], serde_json::json!([[5]]));

        // A location that cannot be read fails the statement and its record.
        let (status, body) = submit(
            &state,
            &analyst,
            "CREATE TABLE ghosts WITH (location = 'ghosts.parquet', format = 'parquet')",
            serde_json::Value::Null,
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
        assert_eq!(body["code"], "TABLE_NOT_READABLE");
        let failed = record(body["id"].as_str().unwrap(), &analyst).await;
        assert_eq!(failed["state"], "FAILED");
        let (status, body) = submit(&state, &analyst, "SHOW TABLES", serde_json::Value::Null).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(body["data"], serde_json::json!([["orders"]]));

        // A malformed catalog statement is a syntax error, not a query.
        let (status, body) = submit(
            &state,
            &analyst,
            "CREATE TABLE orders WITH (location = 'x')",
            serde_json::Value::Null,
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
        assert_eq!(body["code"], "SYNTAX_ERROR");
        assert!(body["error"].as_str().unwrap().contains("format"));
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[tokio::test]
    async fn a_paged_statement_is_not_kept_in_the_result_cache() {
        let (state, _commit, _directory) = analyze_test_state().await;
        let analyst = crate::security::Identity {
            principal: "analyst".into(),
            display_identity: None,
            role: Role::Analyst,
        };
        let sql = "SELECT id FROM orders WHERE id > 1 ORDER BY id";
        let (status, paged) = submit_with_delivery(
            &state,
            &analyst,
            sql,
            serde_json::Value::Null,
            Some("paged"),
        )
        .await;
        assert_eq!(status, axum::http::StatusCode::OK, "{paged}");
        assert!(paged["next_uri"].is_string(), "{paged}");
        assert_eq!(state.result_cache.stats().entries, 0);

        // The same statement inline afterwards is computed, with its rows.
        let (status, inline) = submit(&state, &analyst, sql, serde_json::Value::Null).await;
        assert_eq!(status, axum::http::StatusCode::OK, "{inline}");
        assert_eq!(inline["data"], serde_json::json!([[2], [3]]));
        assert_eq!(
            record(inline["id"].as_str().unwrap(), &analyst).await["execution"]["mode"],
            "coordinator"
        );
    }

    #[tokio::test]
    async fn a_paged_statement_record_advertises_its_pages_while_running() {
        // The record a paged statement gets the moment it runs links its
        // first page; a queued record and an inline statement's do not.
        let context = |delivery: Option<&str>| super::QueryContext {
            engine_version: String::new(),
            environment: String::new(),
            principal: None,
            user: None,
            source: None,
            client: None,
            catalog: "lake".into(),
            schema: "sales".into(),
            time_zone: None,
            client_address: None,
            client_tags: vec![],
            result_delivery: delivery.map(str::to_owned),
            catalog_snapshot_id: String::new(),
            settings: super::QuerySettings::default(),
        };
        let pending = |delivery: Option<&str>, state: super::QueryState| {
            serde_json::to_value(super::pending_query_record(
                "q",
                "SELECT 1",
                &super::QuerySettings::default(),
                0,
                &context(delivery),
                state,
                0,
            ))
            .unwrap()
        };
        assert_eq!(
            pending(Some("paged"), super::QueryState::Running)["next_uri"],
            "/v1/query/q/results/0"
        );
        assert!(
            pending(Some("paged"), super::QueryState::Queued)
                .get("next_uri")
                .is_none()
        );
        assert!(
            pending(None, super::QueryState::Running)
                .get("next_uri")
                .is_none()
        );

        // While the statement runs, the page it has not flushed yet is a 202
        // the client retries; the record and its pages are owner-scoped.
        let (state, _commit, _directory) = analyze_test_state().await;
        let analyst = crate::security::Identity {
            principal: "analyst".into(),
            display_identity: None,
            role: Role::Analyst,
        };
        let other = crate::security::Identity {
            principal: "other".into(),
            display_identity: None,
            role: Role::Analyst,
        };
        let page = |state: &Arc<crate::AppState>,
                    id: &str,
                    index: usize,
                    identity: &crate::security::Identity| {
            let state = state.clone();
            let id = id.to_owned();
            let identity = identity.clone();
            async move {
                let response = super::get_result_page(
                    axum::extract::State(state),
                    axum::extract::Path((id, index)),
                    axum::Extension(identity),
                )
                .await;
                let status = response.status();
                let retry_after = response
                    .headers()
                    .get(axum::http::header::RETRY_AFTER)
                    .map(|value| value.to_str().unwrap().to_owned());
                let body = if status == axum::http::StatusCode::OK
                    || status == axum::http::StatusCode::ACCEPTED
                {
                    json_body(response).await
                } else {
                    serde_json::Value::Null
                };
                (status, retry_after, body)
            }
        };
        let mut writer = state.results.begin("running", "analyst").unwrap();
        writer.push(vec![serde_json::json!(1)]).unwrap();
        let (status, retry_after, body) = page(&state, "running", 0, &analyst).await;
        assert_eq!(status, axum::http::StatusCode::ACCEPTED, "{body}");
        assert_eq!(retry_after.as_deref(), Some("1"));
        assert_eq!(
            body,
            serde_json::json!({"id": "running", "row_count": 0, "complete": false})
        );
        assert_eq!(
            page(&state, "running", 0, &other).await.0,
            axum::http::StatusCode::NOT_FOUND
        );
        drop(writer);
        assert_eq!(
            page(&state, "running", 0, &analyst).await.0,
            axum::http::StatusCode::GONE
        );

        // A finished paged statement keeps the link on its record and its
        // last page says so.
        let sql = "SELECT id FROM orders WHERE id > 1 ORDER BY id";
        let (status, paged) = submit_with_delivery(
            &state,
            &analyst,
            sql,
            serde_json::Value::Null,
            Some("paged"),
        )
        .await;
        assert_eq!(status, axum::http::StatusCode::OK, "{paged}");
        let id = paged["id"].as_str().unwrap().to_owned();
        let first_page = format!("/v1/query/{id}/results/0");
        assert_eq!(paged["next_uri"], first_page);
        let finished = record(&id, &analyst).await;
        assert_eq!(finished["state"], "FINISHED");
        assert_eq!(finished["next_uri"], first_page);
        let (status, _, body) = page(&state, &id, 0, &analyst).await;
        assert_eq!(status, axum::http::StatusCode::OK, "{body}");
        assert_eq!(
            body,
            serde_json::json!({
                "id": id,
                "data": [[2], [3]],
                "next_uri": null,
                "row_count": 2,
                "complete": true,
            })
        );
        assert_eq!(
            page(&state, &id, 1, &analyst).await.0,
            axum::http::StatusCode::NOT_FOUND
        );

        // An inline statement's record never links pages.
        let (status, inline) = submit(&state, &analyst, sql, serde_json::Value::Null).await;
        assert_eq!(status, axum::http::StatusCode::OK, "{inline}");
        assert!(
            record(inline["id"].as_str().unwrap(), &analyst)
                .await
                .get("next_uri")
                .is_none()
        );
    }

    #[tokio::test]
    async fn a_repeated_statement_is_served_from_the_result_cache() {
        let (state, _commit, directory) = analyze_test_state().await;
        let analyst = crate::security::Identity {
            principal: "analyst".into(),
            display_identity: None,
            role: Role::Analyst,
        };
        let sql = "SELECT id FROM orders WHERE id > 1 ORDER BY id";

        let (status, first) = submit(&state, &analyst, sql, serde_json::Value::Null).await;
        assert_eq!(status, axum::http::StatusCode::OK, "{first}");
        assert_eq!(first["data"], serde_json::json!([[2], [3]]));
        let first_id = first["id"].as_str().unwrap().to_owned();
        let first_record = record(&first_id, &analyst).await;
        assert_eq!(first_record["execution"]["mode"], "coordinator");
        assert!(first_record.get("cached_from").is_none());
        assert!(first_record.get("settings").is_none());
        let stats = state.result_cache.stats();
        assert_eq!((stats.hits, stats.misses, stats.entries), (0, 1, 1));

        // Same statement, different spelling outside literals: a hit with
        // the same rows, no worker or coordinator execution, the original
        // named on the record.
        let (status, second) = submit(
            &state,
            &analyst,
            "select   ID from ORDERS\n where id > 1 order by id;",
            serde_json::Value::Null,
        )
        .await;
        assert_eq!(status, axum::http::StatusCode::OK, "{second}");
        assert_eq!(second["data"], first["data"]);
        assert_eq!(second["columns"], first["columns"]);
        let second_id = second["id"].as_str().unwrap();
        assert_ne!(second_id, first_id);
        let second_record = record(second_id, &analyst).await;
        assert_eq!(second_record["execution"]["mode"], "cache");
        assert_eq!(second_record["execution"]["detail"], "hit");
        assert_eq!(second_record["cached_from"], first_id);
        // The kept elapsed is what producing the rows took, measured before
        // the original's serialization; never more than its record's total.
        assert!(
            second_record["cached_elapsed_ms"].as_u64().unwrap()
                <= first_record["elapsed_ms"].as_u64().unwrap()
        );
        assert_eq!(second_record["state"], "FINISHED");
        assert!(second_record["timings"]["execution_us"].is_null());
        assert_eq!(second_record["stages"].as_array().unwrap().len(), 0);
        let stats = state.result_cache.stats();
        assert_eq!((stats.hits, stats.misses, stats.entries), (1, 1, 1));

        // A different literal is a different statement.
        let (status, other) = submit(
            &state,
            &analyst,
            "SELECT id FROM orders WHERE id > 2 ORDER BY id",
            serde_json::Value::Null,
        )
        .await;
        assert_eq!(status, axum::http::StatusCode::OK, "{other}");
        assert_eq!(other["data"], serde_json::json!([[3]]));
        assert_eq!(
            record(other["id"].as_str().unwrap(), &analyst).await["execution"]["mode"],
            "coordinator"
        );
        assert_eq!(state.result_cache.stats().entries, 2);

        // The bypass: neither served from nor kept in the cache, and the
        // record says what the statement set.
        let (status, bypassed) = submit(
            &state,
            &analyst,
            sql,
            serde_json::json!({"result_cache": false}),
        )
        .await;
        assert_eq!(status, axum::http::StatusCode::OK, "{bypassed}");
        let bypassed_record = record(bypassed["id"].as_str().unwrap(), &analyst).await;
        assert_eq!(bypassed_record["execution"]["mode"], "coordinator");
        assert_eq!(
            bypassed_record["settings"],
            serde_json::json!({"result_cache": false})
        );
        let stats = state.result_cache.stats();
        assert_eq!((stats.hits, stats.misses, stats.entries), (1, 2, 2));

        // SET SESSION in the statement text is the same bypass.
        let (status, prefixed) = submit(
            &state,
            &analyst,
            &format!("SET SESSION result_cache = false; {sql}"),
            serde_json::Value::Null,
        )
        .await;
        assert_eq!(status, axum::http::StatusCode::OK, "{prefixed}");
        let prefixed_record = record(prefixed["id"].as_str().unwrap(), &analyst).await;
        assert_eq!(prefixed_record["execution"]["mode"], "coordinator");
        assert_eq!(prefixed_record["sql"], sql);
        assert_eq!(state.result_cache.stats().hits, 1);

        // An unknown setting is refused before anything runs.
        let (status, refused) =
            submit(&state, &analyst, sql, serde_json::json!({"cache": false})).await;
        assert_eq!(status, axum::http::StatusCode::BAD_REQUEST);
        assert_eq!(refused["code"], "INVALID_SETTING");
        assert_eq!(refused["error"], "unknown setting 'cache'");

        // The node reports the counters; clearing is an administrator's call.
        let node = json_body(
            super::get_node(axum::extract::State(state.clone()))
                .await
                .into_response(),
        )
        .await;
        assert_eq!(node["result_cache"]["entries"], 2);
        assert_eq!(node["result_cache"]["hits"], 1);
        let denied = super::clear_result_cache(
            axum::extract::State(state.clone()),
            axum::Extension(analyst.clone()),
        )
        .await;
        assert_eq!(denied.status(), axum::http::StatusCode::FORBIDDEN);
        assert_eq!(state.result_cache.stats().entries, 2);
        let admin = crate::security::Identity {
            principal: "admin".into(),
            display_identity: None,
            role: Role::Admin,
        };
        let cleared =
            super::clear_result_cache(axum::extract::State(state.clone()), axum::Extension(admin))
                .await;
        assert_eq!(cleared.status(), axum::http::StatusCode::OK);
        let cleared = json_body(cleared).await;
        assert_eq!(cleared["cleared_entries"], 2);
        assert_eq!(cleared["result_cache"]["entries"], 0);
        let (status, after) = submit(&state, &analyst, sql, serde_json::Value::Null).await;
        assert_eq!(status, axum::http::StatusCode::OK, "{after}");
        assert_eq!(
            record(after["id"].as_str().unwrap(), &analyst).await["execution"]["mode"],
            "coordinator"
        );

        // A catalog publish drops every entry.
        assert_eq!(state.result_cache.stats().entries, 1);
        assert!(super::refresh_catalog_snapshot(&state).await.is_ok());
        assert_eq!(state.result_cache.stats().entries, 0);
        let _ = std::fs::remove_dir_all(directory);
    }

    /// A parse error is a 400 whose body names the failing token's
    /// position next to the unchanged error text.
    #[tokio::test]
    async fn a_parse_error_carries_its_position() {
        let (state, _commit, directory) = analyze_test_state().await;
        let analyst = crate::security::Identity {
            principal: "analyst".into(),
            display_identity: None,
            role: Role::Analyst,
        };
        let (status, body) =
            submit(&state, &analyst, "SELECT FROM t", serde_json::Value::Null).await;
        assert_eq!(status, axum::http::StatusCode::BAD_REQUEST, "{body}");
        assert_eq!(body["code"], "SYNTAX_ERROR");
        let error = body["error"].as_str().unwrap();
        assert!(error.starts_with("SQL parse error: "), "{error}");
        let (_, reported) = error.rsplit_once(" at Line: 1, Column: ").expect(error);
        assert_eq!(body["position"]["line"], 1);
        assert_eq!(body["position"]["column"], reported.parse::<u64>().unwrap());
        let _ = std::fs::remove_dir_all(directory);
    }

    #[test]
    fn parse_error_positions_are_read_from_the_message() {
        assert_eq!(
            super::parse_error_position(
                "SQL parse error: sql parser error: Expected: an expression, found: FROM at Line: 1, Column: 8"
            ),
            Some((1, 8))
        );
        assert_eq!(
            super::parse_error_position("SQL parse error: only single statements are supported"),
            None
        );
        assert_eq!(super::parse_error_position("at Line: x, Column: 2"), None);
        // An error at end of input carries no location, so no position.
        let at_eof = kaveon_sql::logical_plan::sql_to_logical_plan("SELECT 1\nFROM").unwrap_err();
        assert_eq!(super::parse_error_position(&at_eof.to_string()), None);
        let on_line_two =
            kaveon_sql::logical_plan::sql_to_logical_plan("SELECT 1\nFROM t WHERE 'abc")
                .unwrap_err();
        assert_eq!(
            super::parse_error_position(&on_line_two.to_string()).map(|(line, _)| line),
            Some(2)
        );
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
            settings: crate::settings::QuerySettings::default(),
        }
    }

    fn admin() -> crate::security::Identity {
        crate::security::Identity {
            principal: "admin".into(),
            display_identity: None,
            role: Role::Admin,
        }
    }

    /// The stored document's `distinct` per column name.
    async fn stored_distinct(
        commit: &kaveon_catalog::product_commit::ProductCatalogCommit,
    ) -> Vec<(String, serde_json::Value)> {
        let document = super::stored_statistics_document(commit, "lake.sales.orders")
            .await
            .unwrap()
            .expect("a statistics document");
        assert_eq!(document["version"], 2);
        document["columns"]
            .as_array()
            .unwrap()
            .iter()
            .map(|column| {
                (
                    column["name"].as_str().unwrap().to_owned(),
                    column["distinct"].clone(),
                )
            })
            .collect()
    }

    /// The query records tagged as sub-statements of `parent`, newest last.
    async fn sub_statement_records(parent: &str) -> Vec<super::QueryRecord> {
        let tag = format!("analyze:{parent}");
        let store = super::QUERY_STORE.read().await;
        let mut records = store
            .queries
            .values()
            .filter(|record| record.context.client_tags.contains(&tag))
            .cloned()
            .collect::<Vec<_>>();
        records.sort_by_key(|record| record.submitted_at_ms);
        records
    }

    #[tokio::test]
    async fn analyze_with_distinct_counts_exact_values_and_keeps_them_for_the_same_source() {
        use arrow::array::Float64Array;
        let schema = Arc::new(Schema::new(vec![
            Field::new("id", DataType::Int64, true),
            Field::new("region", DataType::Utf8, false),
            Field::new("amount", DataType::Float64, true),
        ]));
        let batch = RecordBatch::try_new(
            schema.clone(),
            vec![
                Arc::new(Int64Array::from(vec![
                    Some(1),
                    Some(2),
                    Some(2),
                    Some(3),
                    None,
                ])),
                Arc::new(StringArray::from(vec![
                    "east", "west", "east", "west", "east",
                ])),
                Arc::new(Float64Array::from(vec![
                    Some(1.5),
                    Some(1.5),
                    None,
                    Some(2.5),
                    None,
                ])),
            ],
        )
        .unwrap();
        let (state, commit, directory) = analyze_test_state_over(schema, batch).await;
        let admin = admin();
        let null = serde_json::Value::Null;

        // Every column: three sub-statements, exact counts of non-null
        // distinct values, none in the result cache.
        let (status, body) = submit(
            &state,
            &admin,
            "ANALYZE orders WITH (distinct = true)",
            serde_json::Value::Null,
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(
            body["columns"]
                .as_array()
                .unwrap()
                .iter()
                .map(|column| column["name"].as_str().unwrap())
                .collect::<Vec<_>>(),
            ["table", "row_count", "distinct_columns"]
        );
        assert_eq!(body["columns"][2]["type"], "BIGINT");
        assert_eq!(
            body["data"],
            serde_json::json!([["lake.sales.orders", 5, 3]])
        );
        assert_eq!(
            stored_distinct(&commit).await,
            [
                ("id".to_owned(), serde_json::json!(3)),
                ("region".into(), serde_json::json!(2)),
                ("amount".into(), serde_json::json!(2)),
            ]
        );
        assert_eq!(state.result_cache.stats().entries, 0);
        let parent = body["id"].as_str().unwrap().to_owned();
        let children = sub_statement_records(&parent).await;
        assert_eq!(
            children
                .iter()
                .map(|record| record.sql.as_str())
                .collect::<Vec<_>>(),
            [
                "SELECT COUNT(DISTINCT \"id\") FROM lake.sales.orders",
                "SELECT COUNT(DISTINCT \"region\") FROM lake.sales.orders",
                "SELECT COUNT(DISTINCT \"amount\") FROM lake.sales.orders",
            ]
        );
        for child in &children {
            assert!(
                matches!(child.state, super::QueryState::Finished),
                "{} {:?}",
                child.id,
                child.error
            );
            assert_eq!(child.settings.result_cache, Some(false));
            assert_eq!(child.context.principal.as_deref(), Some("admin"));
            assert_ne!(child.id, parent);
        }
        let listed = super::list_queries(axum::Extension(admin.clone())).await.0;
        assert!(
            listed
                .iter()
                .filter(|record| record.sql.starts_with("SELECT COUNT(DISTINCT"))
                .all(|record| record.context.client_tags == [format!("analyze:{parent}")])
        );

        // SHOW STATS FOR presents the counts.
        let (status, body) = submit(
            &state,
            &admin,
            "SHOW STATS FOR orders",
            serde_json::Value::Null,
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        let rows = body["data"].as_array().unwrap();
        assert_eq!(rows.len(), 4);
        assert_eq!(
            (&rows[0][0], &rows[0][4]),
            (&serde_json::json!("id"), &serde_json::json!(3))
        );
        assert_eq!(rows[0][3], 0.2);
        assert_eq!(
            (&rows[1][0], &rows[1][4]),
            (&serde_json::json!("region"), &serde_json::json!(2))
        );
        assert_eq!(
            (&rows[2][0], &rows[2][4]),
            (&serde_json::json!("amount"), &serde_json::json!(2))
        );
        assert_eq!((&rows[3][0], &rows[3][4]), (&null, &null));

        // Named columns: only those are counted; the others keep their
        // counts, the source being unchanged.
        let (status, body) = submit(
            &state,
            &admin,
            "ANALYZE orders WITH (columns = ARRAY['region'])",
            serde_json::Value::Null,
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(
            body["data"],
            serde_json::json!([["lake.sales.orders", 5, 1]])
        );
        assert_eq!(
            sub_statement_records(body["id"].as_str().unwrap())
                .await
                .len(),
            1
        );
        assert_eq!(
            stored_distinct(&commit).await,
            [
                ("id".to_owned(), serde_json::json!(3)),
                ("region".into(), serde_json::json!(2)),
                ("amount".into(), serde_json::json!(2)),
            ]
        );

        // A plain ANALYZE keeps them too.
        let (status, body) =
            submit(&state, &admin, "ANALYZE orders", serde_json::Value::Null).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(
            body["data"],
            serde_json::json!([["lake.sales.orders", 5, 0]])
        );
        assert!(
            sub_statement_records(body["id"].as_str().unwrap())
                .await
                .is_empty()
        );
        assert_eq!(
            stored_distinct(&commit).await,
            [
                ("id".to_owned(), serde_json::json!(3)),
                ("region".into(), serde_json::json!(2)),
                ("amount".into(), serde_json::json!(2)),
            ]
        );

        // An unknown column is refused before any count, the record failed,
        // the document untouched; so are both properties together.
        let before = commit.read_current().await.unwrap();
        let (status, body) = submit(
            &state,
            &admin,
            "ANALYZE orders WITH (columns = ARRAY['region', 'nope'])",
            serde_json::Value::Null,
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
        assert_eq!(body["code"], "ANALYSIS_ERROR");
        assert_eq!(
            body["error"],
            "column 'nope' does not exist in lake.sales.orders"
        );
        assert!(
            sub_statement_records(body["id"].as_str().unwrap())
                .await
                .is_empty()
        );
        assert_eq!(
            record(body["id"].as_str().unwrap(), &admin).await["state"],
            "FAILED"
        );
        let (status, body) = submit(
            &state,
            &admin,
            "ANALYZE orders WITH (distinct = true, columns = ARRAY['id'])",
            serde_json::Value::Null,
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
        assert_eq!(body["code"], "SYNTAX_ERROR");
        assert_eq!(body["error"], "ANALYZE takes distinct or columns, not both");
        assert_eq!(
            commit.read_current().await.unwrap().table_statistics["lake.sales.orders"],
            before.table_statistics["lake.sales.orders"]
        );

        // A changed source: the counts of the columns not measured are
        // gone, the measured one is fresh.
        let schema = Arc::new(Schema::new(vec![
            Field::new("id", DataType::Int64, true),
            Field::new("region", DataType::Utf8, false),
            Field::new("amount", DataType::Float64, true),
        ]));
        let batch = RecordBatch::try_new(
            schema.clone(),
            vec![
                Arc::new(Int64Array::from(vec![Some(7), Some(8)])),
                Arc::new(StringArray::from(vec!["north", "north"])),
                Arc::new(Float64Array::from(vec![Some(1.0), Some(2.0)])),
            ],
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
        let (status, body) =
            submit(&state, &admin, "ANALYZE orders", serde_json::Value::Null).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(
            body["data"],
            serde_json::json!([["lake.sales.orders", 2, 0]])
        );
        assert_eq!(
            stored_distinct(&commit).await,
            [
                ("id".to_owned(), null.clone()),
                ("region".into(), null.clone()),
                ("amount".into(), null.clone()),
            ]
        );
        let (status, body) = submit(
            &state,
            &admin,
            "ANALYZE \"lake\".\"sales\".\"orders\" WITH (columns = ARRAY['region'])",
            serde_json::Value::Null,
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(
            body["data"],
            serde_json::json!([["lake.sales.orders", 2, 1]])
        );
        assert_eq!(
            stored_distinct(&commit).await,
            [
                ("id".to_owned(), null.clone()),
                ("region".into(), serde_json::json!(1)),
                ("amount".into(), null),
            ]
        );
        std::fs::remove_dir_all(directory).unwrap();
    }

    /// Cancelling the ANALYZE cancels the count it is running, the counts
    /// after it never start, and no document is written.
    #[tokio::test]
    async fn a_cancelled_analyze_cancels_its_counts_and_writes_no_document() {
        let columns = 12;
        let rows = 50_000i64;
        let schema = Arc::new(Schema::new(
            (0..columns)
                .map(|index| Field::new(format!("c{index}"), DataType::Int64, false))
                .collect::<Vec<_>>(),
        ));
        let batch = RecordBatch::try_new(
            schema.clone(),
            (0..columns)
                .map(|index| {
                    Arc::new(Int64Array::from(
                        (0..rows).map(|row| row * (index + 1)).collect::<Vec<_>>(),
                    )) as Arc<dyn arrow::array::Array>
                })
                .collect(),
        )
        .unwrap();
        let (state, commit, directory) = analyze_test_state_over(schema, batch).await;
        let admin = admin();
        let parent = format!("analyze-parent-{}", uuid::Uuid::new_v4());
        let running = {
            let state = state.clone();
            let admin = admin.clone();
            let parent = parent.clone();
            tokio::spawn(async move {
                let response = super::run_statement(
                    state,
                    admin,
                    super::StatementRequest {
                        query: "ANALYZE orders WITH (distinct = true)".into(),
                        catalog: Some("lake".into()),
                        schema: Some("sales".into()),
                        source: None,
                        client: None,
                        user: None,
                        time_zone: None,
                        client_tags: vec!["nightly".into()],
                        result_delivery: None,
                        settings: None,
                    },
                    parent,
                )
                .await;
                let status = response.status();
                (status, json_body(response).await)
            })
        };
        // The first count is under way: the ANALYZE record runs, and so
        // does a sub-statement tagged with it.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
        loop {
            assert!(
                std::time::Instant::now() < deadline,
                "no sub-statement started"
            );
            assert!(
                !running.is_finished(),
                "ANALYZE finished before it was cancelled"
            );
            if !sub_statement_records(&parent).await.is_empty() {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(2)).await;
        }
        assert_eq!(record(&parent, &admin).await["state"], "RUNNING");
        let cancelled = super::cancel_query(
            axum::extract::State(state.clone()),
            axum::Extension(admin.clone()),
            axum::extract::Path(parent.clone()),
        )
        .await
        .into_response();
        assert_eq!(cancelled.status(), StatusCode::NO_CONTENT);
        let (status, body) = running.await.unwrap();
        assert_eq!(status, StatusCode::CONFLICT, "{body}");
        assert_eq!(body["code"], "QUERY_CANCELED");
        assert_eq!(record(&parent, &admin).await["state"], "CANCELED");
        let children = sub_statement_records(&parent).await;
        assert!(!children.is_empty());
        assert!(
            (children.len() as i64) < columns,
            "every count ran: {}",
            children.len()
        );
        for child in &children {
            assert!(
                !matches!(
                    child.state,
                    super::QueryState::Queued | super::QueryState::Running
                ),
                "{} {:?}",
                child.id,
                child.error
            );
            assert_eq!(
                child.context.client_tags,
                ["nightly".to_owned(), format!("analyze:{parent}")]
            );
        }
        assert!(matches!(
            children.last().unwrap().state,
            super::QueryState::Canceled
        ));
        assert!(
            commit
                .read_current()
                .await
                .unwrap()
                .table_statistics
                .is_empty()
        );
        std::fs::remove_dir_all(directory).unwrap();
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
            analyze("ANALYZE orders"),
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
            analyze("ANALYZE orders"),
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
        assert!(enabled.native_analyze);
        assert!(enabled.transactions.enabled);
        assert_eq!(enabled.transactions.supported_statements[0], "BEGIN");
        assert!(enabled.transactions.single_statement_per_request);
        assert!(!enabled.transactions.parameter_binding);
        assert!(!enabled.transactions.multi_row_insert);
        assert!(!enabled.transactions.returning);
        assert!(!enabled.transactions.savepoints);
        assert!(!enabled.transactions.explicit_isolation_modes);
        assert!(!enabled.transactions.arbitrary_table_dml);
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[tokio::test]
    async fn capability_is_false_without_durable_statistics_authority() {
        let state = Arc::new(catalog_test_state());
        let capabilities = capabilities(axum::extract::State(state)).await.0;
        assert!(!capabilities.native_analyze);
        assert!(!capabilities.transactions.enabled);
    }

    #[test]
    fn statement_api_directs_supported_transaction_sql_to_transaction_endpoint() {
        let (status, body) = transaction_api_guidance(
            "INSERT INTO product.datasets (id, document_json) VALUES ('ds-1', '{}')",
            true,
        )
        .expect("supported product DML should be redirected");
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(body["code"], "TRANSACTION_API_REQUIRED");
        assert_eq!(body["transaction_endpoint"], "/v1/transaction/sql");

        let (status, body) = transaction_api_guidance("BEGIN", false)
            .expect("transaction control should be recognized");
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(body["code"], "TRANSACTION_API_UNAVAILABLE");
    }

    #[test]
    fn statement_api_does_not_redirect_unsupported_row_dml_or_reads() {
        assert!(transaction_api_guidance("INSERT INTO app.users (id) VALUES (1)", true).is_none());
        assert!(transaction_api_guidance("SELECT 1", true).is_none());
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

    #[tokio::test]
    async fn whoami_reports_the_authenticated_identity_and_source() {
        let mut state = catalog_test_state();
        state.config.security.principals = vec![crate::security::PrincipalCredential {
            token: "analyst-token-0123456789abcdef0123".into(),
            principal: "ana".into(),
            role: crate::security::Role::Analyst,
        }];
        state.config.security.insecure_development = true;
        let app = super::build_router(Arc::new(state));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let client = reqwest::Client::new();
        let body: serde_json::Value = client
            .get(format!("http://{address}/v1/whoami"))
            .bearer_auth("analyst-token-0123456789abcdef0123")
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(body["principal"], "ana");
        assert_eq!(body["role"], "analyst");
        assert_eq!(body["auth"], "static");
        assert!(body["display"].is_null());
        let development: serde_json::Value = client
            .get(format!("http://{address}/v1/whoami"))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(development["principal"], "development");
        assert_eq!(development["role"], "admin");
        assert_eq!(development["auth"], "development");
        server.abort();
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
    async fn finishing_a_query_cancels_the_tasks_it_still_runs() {
        let state = Arc::new(catalog_test_state());
        let token = state.lifecycle.cancellations.token("query-orphan").unwrap();
        assert!(!token.is_cancelled());
        let mut headers = axum::http::HeaderMap::new();
        headers.insert(
            axum::http::header::AUTHORIZATION,
            "Bearer exchange-token-at-least-32-bytes-long"
                .parse()
                .unwrap(),
        );
        let response = super::finish_worker_query(
            axum::extract::State(state.clone()),
            axum::extract::Path("query-orphan".to_owned()),
            headers,
        )
        .await;
        assert_eq!(response.status(), axum::http::StatusCode::NO_CONTENT);
        // The running task's token observes the cancel; the registry entry is gone.
        assert!(token.is_cancelled());
        let fresh = state.lifecycle.cancellations.token("query-orphan").unwrap();
        assert!(!fresh.is_cancelled());
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
    async fn coordinator_placement_names_its_reason() {
        use super::{ExecutionPlacement, QueryContext, execute_distributed_fragments};
        use crate::planner::SourcePins;
        // A cluster with no workers cannot distribute: the fragments path
        // declines and says why, and the record carries it.
        let state = Arc::new(catalog_test_state());
        let plan = kaveon_sql::logical_plan::sql_to_logical_plan("SELECT id FROM events").unwrap();
        let context = QueryContext {
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
            client_tags: Vec::new(),
            result_delivery: None,
            catalog_snapshot_id: String::new(),
            settings: crate::settings::QuerySettings::default(),
        };
        let snapshot = kaveon_core::CatalogManager::new("kaveon", "default");
        let mut reason = None;
        let outcome = execute_distributed_fragments(
            &state,
            "query-placement",
            &context,
            &plan,
            &snapshot,
            &SourcePins::default(),
            super::DistributedSink {
                placement_reason: &mut reason,
                result_writer: &mut None,
            },
        )
        .await;
        assert!(outcome.is_none());
        let reason = reason.expect("the coordinator path is explained");
        assert!(
            reason.contains("no distributed plan") || reason.contains("worker"),
            "{reason}"
        );
        let placement = ExecutionPlacement::coordinator(Some(reason.clone()));
        assert_eq!(placement.mode, "coordinator");
        assert_eq!(placement.detail.as_deref(), Some(reason.as_str()));
        assert_eq!(
            serde_json::to_value(ExecutionPlacement::distributed("fragments")).unwrap(),
            serde_json::json!({"mode": "distributed", "detail": "fragments"})
        );
        assert_eq!(
            serde_json::to_value(ExecutionPlacement::pending()).unwrap(),
            serde_json::json!({"mode": "pending"})
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
            settings: crate::settings::QuerySettings::default(),
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
            settings: crate::settings::QuerySettings::default(),
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
            compressed_bytes_read: 5,
            row_filter_rows_examined: rows_selected,
            row_filter_rows_admitted: rows_emitted,
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
        assert_eq!(scans[0].compressed_bytes_read, 10);
        assert_eq!(scans[0].row_filter_rows_examined, 110);
        assert_eq!(scans[0].row_filter_rows_admitted, 70);
        assert_ne!(scans[0].rows_emitted, stages[0].tasks[0].output_rows as u64);
        let incomplete = vec![super::StageTelemetry {
            tasks: vec![task(None)],
            ..stages[0].clone()
        }];
        assert!(!super::distributed_scan_telemetry(&incomplete).1);
    }

    /// Tasks land on the running record one at a time: the stage counters
    /// and scan totals rise with each, the stage finishes with its last
    /// task, and the totals equal what the final aggregation reports.
    #[test]
    fn a_running_record_reports_each_finished_task() {
        let task = |partition_index, rows_emitted| super::TaskTelemetry {
            task_id: format!("task-{partition_index}"),
            node_id: "worker".into(),
            partition_index,
            elapsed_us: 1,
            output_rows: 0,
            output_batches: 0,
            output_bytes: 0,
            execution: None,
            scan: Some(super::TaskScanMetrics {
                rows_emitted,
                rows_selected: rows_emitted + 10,
                ..Default::default()
            }),
        };
        let mut record = super::pending_query_record(
            "q",
            "SELECT 1",
            &super::QuerySettings::default(),
            0,
            &super::QueryContext {
                engine_version: String::new(),
                environment: String::new(),
                principal: None,
                user: None,
                source: None,
                client: None,
                catalog: "lake".into(),
                schema: "sales".into(),
                time_zone: None,
                client_address: None,
                client_tags: vec![],
                result_delivery: None,
                catalog_snapshot_id: String::new(),
                settings: super::QuerySettings::default(),
            },
            super::QueryState::Running,
            0,
        );
        assert!(record.stages.is_empty() && record.scans.is_empty());

        super::merge_task_into_record(&mut record, 1, 2, 5, task(1, 30));
        assert_eq!(record.stages.len(), 1);
        assert_eq!(
            (
                record.stages[0].stage_id,
                record.stages[0].state,
                record.stages[0].task_count,
                record.stages[0].completed_tasks,
            ),
            (1, "RUNNING", 2, 1)
        );
        assert_eq!(record.scans[0].rows_emitted, 30);
        assert!(!record.scan_metrics_complete);

        super::merge_task_into_record(&mut record, 0, 1, 7, task(0, 5));
        assert_eq!(
            record
                .stages
                .iter()
                .map(|stage| (stage.stage_id, stage.state))
                .collect::<Vec<_>>(),
            vec![(0, "FINISHED"), (1, "RUNNING")]
        );
        assert_eq!(record.scans[0].rows_emitted, 35);

        super::merge_task_into_record(&mut record, 1, 2, 9, task(0, 40));
        assert_eq!(
            (
                record.stages[1].state,
                record.stages[1].completed_tasks,
                record.stages[1].elapsed_us,
            ),
            ("FINISHED", 2, 9)
        );
        assert_eq!(
            record.stages[1]
                .tasks
                .iter()
                .map(|task| task.partition_index)
                .collect::<Vec<_>>(),
            vec![0, 1]
        );
        let (scans, complete) = super::distributed_scan_telemetry(&record.stages);
        assert!(complete);
        assert_eq!(scans[0].rows_emitted, 75);
        assert_eq!(record.scans[0].rows_emitted, scans[0].rows_emitted);
        assert_eq!(record.scans[0].rows_selected, scans[0].rows_selected);
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
