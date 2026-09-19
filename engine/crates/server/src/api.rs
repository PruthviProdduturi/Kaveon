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
use kaveon_catalog::CascadePolicy;
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
#[cfg(test)]
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
    /// `pending`, `distributed`, `coordinator`, `cache` or `context`.
    mode: &'static str,
    /// The distributed path taken (`fragments`, `aggregate`, `top_n`), the
    /// reason the coordinator ran it instead, `hit` for a cached result,
    /// or the statistics version a `context` answer came from.
    #[serde(skip_serializing_if = "Option::is_none")]
    detail: Option<String>,
    /// For a `context` answer: the source version the statistics that
    /// answered describe, and the source's version as observed by this
    /// statement — equal by construction.
    #[serde(skip_serializing_if = "Option::is_none")]
    source_version: Option<kaveon_core::SourceVersion>,
    #[serde(skip_serializing_if = "Option::is_none")]
    current_source_version: Option<kaveon_core::SourceVersion>,
}

impl ExecutionPlacement {
    fn pending() -> Self {
        Self {
            mode: "pending",
            detail: None,
            source_version: None,
            current_source_version: None,
        }
    }
    fn distributed(path: &str) -> Self {
        Self {
            mode: "distributed",
            detail: Some(path.to_owned()),
            source_version: None,
            current_source_version: None,
        }
    }
    fn coordinator(reason: Option<String>) -> Self {
        Self {
            mode: "coordinator",
            detail: Some(reason.unwrap_or_else(|| "shape has no distributed plan".to_owned())),
            source_version: None,
            current_source_version: None,
        }
    }
    /// Served from the coordinator's result cache: no worker work.
    fn cache() -> Self {
        Self {
            mode: "cache",
            detail: Some("hit".to_owned()),
            source_version: None,
            current_source_version: None,
        }
    }
    /// Answered from the table's statistics at the statement's pinned
    /// source version: no scan.
    fn context(answer: &ContextAnswer) -> Self {
        Self {
            mode: "context",
            detail: Some(format!("statistics at {}", answer.source_version.label())),
            source_version: Some(answer.source_version.clone()),
            current_source_version: Some(answer.current_source_version.clone()),
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
    files_pruned_by_partition: u64,
    files_skipped: u64,
    decoded_batch_cache_hits: u64,
    decoded_batch_cache_misses: u64,
    decoded_batch_cache_evictions: u64,
    decoded_batch_cache_singleflight_waits: u64,
    row_groups_considered: u64,
    row_groups_selected: u64,
    row_groups_pruned_by_bloom: u64,
    bloom_filters_read: u64,
    bloom_filter_bytes_read: u64,
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
    /// Files of a partitioned directory table the scan predicate ruled
    /// out by their path values; never opened, never considered.
    files_pruned_by_partition: u64,
    /// Files the scan predicate ruled out from their recorded bounds —
    /// the Delta log's add-action stats, or the table's statistics —
    /// before any footer was read; considered, never opened.
    files_skipped: u64,
    decoded_batch_cache_hits: u64,
    decoded_batch_cache_misses: u64,
    decoded_batch_cache_evictions: u64,
    decoded_batch_cache_singleflight_waits: u64,
    row_groups_considered: u64,
    row_groups_read: u64,
    row_groups_pruned: u64,
    /// Of the pruned, the row groups a Bloom filter ruled out after the
    /// statistics had kept them (an equality on a column that carries
    /// one), and the filters read to decide it.
    row_groups_pruned_by_bloom: u64,
    bloom_filters_read: u64,
    bloom_filter_bytes_read: u64,
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
            "/v1/task/{query_id}/{stage_id}/{partition}/{attempt}/metrics",
            get(task_metrics),
        )
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
        .route(
            "/v1/catalog/tables/{table_id}/statistics",
            get(get_table_statistics),
        )
        .route(
            "/v1/catalog/tables/{table_id}/version",
            get(get_table_version),
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
    /// A root task (no exchange output) streams its rows in the response
    /// body as it produces them, and its metrics are read afterwards from
    /// `/v1/task/{query}/{stage}/{partition}/{attempt}/metrics`. Absent
    /// for an older coordinator, which gets the collected result as before.
    #[serde(default)]
    stream_result: bool,
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
        if req.stream_result && is_root_fragment(&req, fragment) {
            return stream_root_task(
                state,
                req,
                partition,
                admitted,
                owner,
                cancellation,
                started,
                elapsed_us(admission_started),
            )
            .await;
        }
        let result = execute_fragment_task(
            state,
            &req,
            fragment,
            partition,
            admitted.pool(),
            elapsed_us(admission_started),
            None,
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

/// A root task produces the statement's rows: it has no exchange output.
fn is_root_fragment(req: &TaskRequest, fragment: &ExecutableFragment) -> bool {
    req.exchange_outputs.is_empty()
        && !fragment.nodes.iter().any(|node| {
            node.id == fragment.root
                && matches!(
                    node.operator,
                    kaveon_core::FragmentOperator::ExchangeOutput(_)
                )
        })
}

/// Chunks of a streamed root result held between the executing thread and
/// the response body: with 4 MiB chunks, 32 MiB of back-pressure.
const ROOT_STREAM_CHUNKS: usize = 8;
const ROOT_STREAM_CHUNK_BYTES: usize = 4 * 1024 * 1024;

/// The response header that marks a root task's body as streamed while the
/// task ran: its metrics are at `/metrics`, not in the headers.
const TASK_STREAMED_HEADER: &str = "x-kaveon-task-streamed";

type RootReady = Arc<std::sync::Mutex<Option<tokio::sync::oneshot::Sender<Result<(), String>>>>>;

/// A test's hook into a streamed root task, keyed by query: called from
/// the executing thread after each batch leaves with the batch's index; an
/// error fails the task there.
#[cfg(test)]
type RootStreamProbe = Arc<dyn Fn(usize) -> Result<(), String> + Send + Sync>;
#[cfg(test)]
static ROOT_STREAM_PROBES: std::sync::LazyLock<std::sync::Mutex<HashMap<String, RootStreamProbe>>> =
    std::sync::LazyLock::new(Default::default);

/// The executing thread's side of a streamed root result: an IPC writer
/// whose bytes leave as chunks after every batch, so a batch reaches the
/// coordinator as soon as it is produced. Opening it — the schema is known
/// — is what lets the handler answer `200` and start the body.
struct RootStreamSink {
    /// Which task the test probes address.
    #[cfg_attr(not(test), allow(dead_code))]
    query_id: String,
    writer: Option<arrow::ipc::writer::StreamWriter<Vec<u8>>>,
    chunks: tokio::sync::mpsc::Sender<axum::body::Bytes>,
    ready: RootReady,
    batches: usize,
}

impl RootStreamSink {
    fn cut(&mut self) -> kaveon_core::Result<()> {
        let Some(writer) = self.writer.as_mut() else {
            return Ok(());
        };
        let buffer = writer.get_mut();
        while !buffer.is_empty() {
            let take = buffer.len().min(ROOT_STREAM_CHUNK_BYTES);
            let chunk: Vec<u8> = buffer.drain(..take).collect();
            // From the executing thread: waits while the body is behind,
            // which is the back-pressure.
            self.chunks
                .blocking_send(axum::body::Bytes::from(chunk))
                .map_err(|_| {
                    kaveon_core::KaveonError::Execution(
                        "the result's consumer stopped reading".into(),
                    )
                })?;
        }
        Ok(())
    }

    /// Close the stream: the end-of-stream marker leaves as the last chunk.
    fn finish(mut self) -> kaveon_core::Result<()> {
        let mut writer = self.writer.take().ok_or_else(|| {
            kaveon_core::KaveonError::Execution("root result was never opened".into())
        })?;
        writer
            .finish()
            .map_err(|error| kaveon_core::KaveonError::Execution(error.to_string()))?;
        self.writer = Some(writer);
        self.cut()
    }
}

impl crate::fragment_exec::RootSink for RootStreamSink {
    fn open(&mut self, schema: &arrow::datatypes::SchemaRef) -> kaveon_core::Result<()> {
        if self.writer.is_some() {
            return Ok(());
        }
        let options = arrow::ipc::writer::IpcWriteOptions::default()
            .try_with_compression(Some(arrow::ipc::CompressionType::LZ4_FRAME))
            .map_err(|error| kaveon_core::KaveonError::Execution(error.to_string()))?;
        self.writer = Some(
            arrow::ipc::writer::StreamWriter::try_new_with_options(Vec::new(), schema, options)
                .map_err(|error| kaveon_core::KaveonError::Execution(error.to_string()))?,
        );
        if let Some(ready) = self.ready.lock().ok().and_then(|mut ready| ready.take()) {
            let _ = ready.send(Ok(()));
        }
        self.cut()
    }

    fn write(&mut self, batch: &arrow::record_batch::RecordBatch) -> kaveon_core::Result<()> {
        let writer = self.writer.as_mut().ok_or_else(|| {
            kaveon_core::KaveonError::Execution("root result was never opened".into())
        })?;
        writer
            .write(batch)
            .map_err(|error| kaveon_core::KaveonError::Execution(error.to_string()))?;
        self.cut()?;
        self.batches += 1;
        #[cfg(test)]
        {
            let probe = ROOT_STREAM_PROBES
                .lock()
                .ok()
                .and_then(|probes| probes.get(&self.query_id).cloned());
            if let Some(probe) = probe {
                probe(self.batches - 1).map_err(kaveon_core::KaveonError::Execution)?;
            }
        }
        Ok(())
    }
}

/// Run a root fragment with its rows streamed in the response body. The
/// request is answered once the result schema is known: `200` and a body
/// that carries the Arrow IPC stream as the fragment produces it, or the
/// failure that came first with its status as for a collected task. A
/// failure after the body began leaves the stream without its end marker;
/// the task's outcome, with the failure, is at `/metrics` before the body
/// ends.
#[allow(clippy::too_many_arguments)]
async fn stream_root_task(
    state: &Arc<AppState>,
    req: TaskRequest,
    partition: kaveon_storage::ScanPartition,
    admitted: AdmittedQueryMemory,
    owner: TaskOwner<crate::transport::CachedTaskResult>,
    cancellation: CancellationToken,
    started: Instant,
    admission_wait_us: u64,
) -> Response {
    let (chunks, body) = tokio::sync::mpsc::channel::<axum::body::Bytes>(ROOT_STREAM_CHUNKS);
    let (ready_sender, ready) = tokio::sync::oneshot::channel::<Result<(), String>>();
    let ready_sender: RootReady = Arc::new(std::sync::Mutex::new(Some(ready_sender)));
    let sink = RootStreamSink {
        query_id: req.query_id.clone(),
        writer: None,
        chunks: chunks.clone(),
        ready: Arc::clone(&ready_sender),
        batches: 0,
    };
    let state = Arc::clone(state);
    let task_cancellation = cancellation.clone();
    tokio::spawn(async move {
        let result = match req.fragment.as_ref() {
            Some(fragment) => {
                execute_fragment_task(
                    &state,
                    &req,
                    fragment,
                    partition,
                    admitted.pool(),
                    admission_wait_us,
                    Some(sink),
                )
                .await
            }
            None => Err("streamed task has no fragment".to_owned()),
        };
        let outcome = if task_cancellation.is_cancelled() {
            TaskOutcome::Failed(Arc::from("query canceled"))
        } else {
            match result {
                Ok((_, _, scan, execution)) => {
                    TaskOutcome::Success(Arc::new(crate::transport::CachedTaskResult::streamed(
                        elapsed_us(started),
                        scan.and_then(|scan| serde_json::to_string(&scan).ok()),
                        serde_json::to_string(&execution).ok(),
                    )))
                }
                Err(message) => TaskOutcome::Failed(Arc::from(message)),
            }
        };
        // A failure before the schema answers the request itself.
        if let Some(ready) = ready_sender.lock().ok().and_then(|mut ready| ready.take()) {
            let _ = ready.send(Err(match &outcome {
                TaskOutcome::Failed(message) => message.to_string(),
                TaskOutcome::Success(_) => "task produced no result".to_owned(),
            }));
        }
        // The outcome is recorded before the body ends: a coordinator that
        // sees the end of the body finds the metrics, or the failure, at
        // `/metrics` without waiting.
        let _ = owner.complete(outcome);
        drop(chunks);
        drop(admitted);
    });
    match ready.await {
        Ok(Ok(())) => Response::builder()
            .status(StatusCode::OK)
            .header(header::CONTENT_TYPE, "application/vnd.apache.arrow.stream")
            .header(TASK_STREAMED_HEADER, "1")
            .body(Body::from_stream(futures::stream::unfold(
                body,
                |mut body| async move {
                    body.recv()
                        .await
                        .map(|chunk| (Ok::<_, std::io::Error>(chunk), body))
                },
            )))
            .unwrap_or_else(|error| lifecycle_error_response(error.to_string())),
        Ok(Err(message)) => {
            if cancellation.is_cancelled() {
                canceled_task_response()
            } else {
                task_failure_response(StatusCode::INTERNAL_SERVER_ERROR, &message)
            }
        }
        Err(_) => task_failure_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            "task ended before producing a result",
        ),
    }
}

/// The metrics of a task once it finished: `200` with `elapsed_us`, `scan`
/// and `execution` (null when the task carries none), `202` while it still
/// runs, `500` with its failure, `404` for a task this worker does not know
/// (or has already forgotten with its query).
async fn task_metrics(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path((query_id, stage_id, partition, attempt)): Path<(String, u32, usize, u32)>,
) -> Response {
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
    let task_id = TaskId {
        query_id,
        stage_id: StageId(stage_id),
        partition,
        attempt,
    };
    match state.lifecycle.tasks.status(&task_id) {
        Err(error) => lifecycle_error_response(error.to_string()),
        Ok(crate::lifecycle::TaskStatus::Unknown) => {
            task_failure_response(StatusCode::NOT_FOUND, "unknown task")
        }
        Ok(crate::lifecycle::TaskStatus::Running) => (
            StatusCode::ACCEPTED,
            Json(serde_json::json!({ "state": "RUNNING" })),
        )
            .into_response(),
        Ok(crate::lifecycle::TaskStatus::Completed(TaskOutcome::Failed(message))) => {
            task_failure_response(StatusCode::INTERNAL_SERVER_ERROR, &message)
        }
        Ok(crate::lifecycle::TaskStatus::Completed(TaskOutcome::Success(result))) => {
            let json = |header: Option<&str>| {
                header
                    .filter(|value| !value.is_empty())
                    .and_then(|value| serde_json::from_str::<serde_json::Value>(value).ok())
                    .unwrap_or(serde_json::Value::Null)
            };
            Json(serde_json::json!({
                "elapsed_us": result.elapsed_us,
                "scan": json(result.scan_metrics_header.as_deref()),
                "execution": json(result.execution_metrics_header.as_deref()),
            }))
            .into_response()
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

/// Run a fragment task. With `root`, a root fragment's result streams
/// through it as it is produced and the returned batches are empty.
async fn execute_fragment_task(
    state: &Arc<AppState>,
    req: &TaskRequest,
    fragment: &ExecutableFragment,
    partition: kaveon_storage::ScanPartition,
    memory: &kaveon_core::QueryMemoryPool,
    admission_wait_us: u64,
    root: Option<RootStreamSink>,
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
        let mut root = root;
        let result = crate::fragment_exec::execute_fragment_streaming_root(
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
            root.as_mut()
                .map(|root| root as &mut dyn crate::fragment_exec::RootSink),
        )
        .map_err(|error| error.to_string());
        let outputs = match &result {
            Ok(execution) => outputs
                .finish(&execution.result_schema)
                .map_err(|error| error.to_string()),
            Err(error) => Err(error.clone()),
        };
        // A failed fragment leaves the streamed result without its end
        // marker: that is how the coordinator learns to ask for the outcome.
        let root_finished = match (&result, root) {
            (Ok(_), Some(root)) => root.finish().map_err(|error| error.to_string()),
            _ => Ok(()),
        };
        let cpu_us =
            cpu_started.and_then(|started| thread_cpu_us().map(|end| end.saturating_sub(started)));
        (
            result,
            outputs,
            root_finished,
            cpu_us,
            queue_us,
            elapsed_us(wall_started),
        )
    })
    .await
    .map_err(|error| format!("fragment execution task failed: {error}"))?;
    let (execution, outputs, root_finished, compute_cpu_us, compute_queue_us, compute_wall_us) =
        execution;
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
    root_finished?;
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
        // A streamed root task's rows went to the coordinator as it ran and
        // are not retained: a second submission is refused, and its
        // metrics stay at `/metrics`.
        TaskOutcome::Success(result) if result.streamed => (
            StatusCode::CONFLICT,
            Json(serde_json::json!({
                "error": "streamed root task result is not retained; its metrics are at /metrics",
                "code": "TASK_RESULT_NOT_RETAINED"
            })),
        )
            .into_response(),
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
// retain a token clone and observe cancellation cooperatively. A record the
// handler never finalised — the client went away while the statement was
// queued or running — is marked CANCELED so it does not read as RUNNING
// forever in `GET /v1/query`.
struct StatementLifecycleGuard {
    state: Arc<AppState>,
    query_id: String,
}
impl Drop for StatementLifecycleGuard {
    fn drop(&mut self) {
        let _ = self.state.lifecycle.cancellations.cancel(&self.query_id);
        let _ = self.state.lifecycle.finish_query(&self.query_id);
        let query_id = self.query_id.clone();
        if let Ok(handle) = tokio::runtime::Handle::try_current() {
            handle.spawn(async move {
                if let Some(record) = QUERY_STORE.write().await.queries.get_mut(&query_id)
                    && matches!(record.state, QueryState::Queued | QueryState::Running)
                {
                    record.state = QueryState::Canceled;
                    record.error =
                        Some("client disconnected before the statement finished".to_owned());
                    record.completed_at_ms = unix_time_ms();
                    record.elapsed_ms = record
                        .completed_at_ms
                        .saturating_sub(record.submitted_at_ms);
                }
            });
        }
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
    // OPTIMIZE rewrites data under the statement's admitted memory: its
    // sort runs on the pool like any operator, so the admission stays held.
    if let Some(kaveon_sql::ddl::CatalogStatement::Optimize {
        name,
        filter,
        options,
    }) = catalog_statement
    {
        let result = crate::optimize::execute_optimize(
            &state,
            &identity,
            &context.catalog,
            &context.schema,
            name,
            filter,
            options,
            query_memory.pool().clone(),
        )
        .await
        .map_err(|error| (error.status, error.code, error.message));
        drop(query_memory);
        return finish_catalog_result(&query_id, result, start).await;
    }
    if let Some(statement) = catalog_statement {
        return execute_catalog(&state, &identity, &query_id, &context, statement, start).await;
    }
    if let Some(statement) = parse_analyze_statement(&sql) {
        // ANALYZE runs no operator of its own, and the statements it runs
        // for its distinct counts are admitted in their own right — memory,
        // principal and resource group — so a single-slot coordinator does
        // not wait on itself.
        drop(_group_permit);
        drop(_principal_permit);
        return execute_analyze(
            &state,
            &identity,
            &query_id,
            &context,
            statement,
            start,
            query_memory,
        )
        .await;
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
    let (plan, planning_source_pins, planning_statistics) =
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

    // A COUNT(*), MIN or MAX with no predicate whose table's statistics
    // describe exactly the version this statement is pinned to is answered
    // from them: no scan, on any node.
    if let Some(answer) = context_answer(&plan, &planning_statistics) {
        let mut data = vec![answer.row.clone()];
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
            execution: ExecutionPlacement::context(&answer),
            settings: settings.clone(),
            cached_from: None,
            cached_elapsed_ms: None,
            admission_wait_ms,
            next_uri: paged_next_uri(&query_id, &context),
            id: query_id.clone(),
            sql,
            state: QueryState::Finished,
            columns: answer.columns.clone(),
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
            columns: Some(answer.columns),
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
/// ARRAY['a', 'b'] [, sketches = true])]`, parsed.
#[derive(Clone, Debug, PartialEq, Eq)]
struct AnalyzeStatement {
    table: String,
    distinct: DistinctColumns,
    /// Read every sketchable column once for the distinct-count and
    /// quantile sketches and exact bounds.
    sketches: bool,
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
            sketches: false,
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
            "ANALYZE accepts WITH (distinct = true), WITH (columns = ARRAY['a', 'b']) or WITH (sketches = true) after the table name".to_owned()
        })?;
    let mut distinct = None;
    let mut columns = None;
    let mut sketches = None;
    let flag = |name: &str, value: &str| -> Result<bool, String> {
        match value {
            v if v.eq_ignore_ascii_case("true") => Ok(true),
            v if v.eq_ignore_ascii_case("false") => Ok(false),
            other => Err(format!(
                "ANALYZE property {name} must be true or false, not {other}"
            )),
        }
    };
    for entry in split_property_entries(properties)? {
        let (key, value) = entry
            .split_once('=')
            .map(|(key, value)| (key.trim(), value.trim()))
            .ok_or_else(|| format!("ANALYZE property '{}' needs key = value", entry.trim()))?;
        if key.eq_ignore_ascii_case("distinct") {
            if distinct.is_some() {
                return Err("ANALYZE property distinct is given twice".into());
            }
            distinct = Some(flag("distinct", value)?);
        } else if key.eq_ignore_ascii_case("columns") {
            if columns.is_some() {
                return Err("ANALYZE property columns is given twice".into());
            }
            columns = Some(parse_column_array(value)?);
        } else if key.eq_ignore_ascii_case("sketches") {
            if sketches.is_some() {
                return Err("ANALYZE property sketches is given twice".into());
            }
            sketches = Some(flag("sketches", value)?);
        } else {
            return Err(format!(
                "unknown ANALYZE property '{key}'; the properties are distinct, columns and sketches"
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
    Ok(AnalyzeStatement {
        table,
        distinct,
        sketches: sketches.unwrap_or(false),
    })
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

/// The table a statistics statement names, resolved twice: through the
/// published catalog for its location and format, and through the durable
/// store for the id its statistics are kept under.
struct StatisticsTable {
    qualified: String,
    id: TableId,
    location: String,
    format: kaveon_core::DataFormat,
}

/// A statement's failure before it answered: status, code, message.
type StatementFailure = (StatusCode, &'static str, String);

async fn resolve_statistics_table(
    state: &AppState,
    qualified: &str,
) -> Result<StatisticsTable, StatementFailure> {
    let resolved = state
        .catalog
        .read()
        .await
        .resolve_table(&kaveon_core::TableReference::parse(qualified))
        .map_err(|error| {
            (
                StatusCode::BAD_REQUEST,
                "TABLE_NOT_FOUND",
                error.to_string(),
            )
        })?;
    let definition = state
        .catalog_store
        .table_by_name(&resolved.catalog, &resolved.schema, &resolved.table.name)
        .map_err(|error| {
            (
                StatusCode::SERVICE_UNAVAILABLE,
                "CATALOG_UNAVAILABLE",
                error.to_string(),
            )
        })?
        .ok_or_else(|| {
            (
                StatusCode::BAD_REQUEST,
                "TABLE_NOT_FOUND",
                format!("{qualified} is not in the durable catalog"),
            )
        })?;
    Ok(StatisticsTable {
        qualified: format!(
            "{}.{}.{}",
            resolved.catalog, resolved.schema, resolved.table.name
        ),
        id: definition.id().clone(),
        location: resolved.full_path(),
        format: resolved.table.format,
    })
}

/// The table's statistics on record, whatever source version they
/// describe; `None` when the table was never analyzed.
fn stored_table_statistics(
    state: &AppState,
    id: &TableId,
) -> Result<Option<Arc<kaveon_core::TableStatistics>>, StatementFailure> {
    state
        .catalog_store
        .table_statistics(id)
        .map(|value| value.map(Arc::new))
        .map_err(|error| {
            (
                StatusCode::SERVICE_UNAVAILABLE,
                "STATISTICS_INVALID",
                format!(
                    "stored statistics for {} are not readable: {error}",
                    id.as_str()
                ),
            )
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
/// columns of `SHOW STATS FOR` and `DESCRIBE DETAIL`.
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
fn bound_text(value: Option<&kaveon_storage::StatValue>) -> serde_json::Value {
    match value.map(kaveon_storage::StatValue::to_json) {
        None | Some(serde_json::Value::Null) => serde_json::Value::Null,
        Some(serde_json::Value::String(text)) => serde_json::Value::String(text),
        Some(other) => serde_json::Value::String(other.to_string()),
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

/// The Delta version a statistics document describes, when its source is
/// a Delta table.
fn delta_version_of(statistics: &kaveon_core::TableStatistics) -> serde_json::Value {
    match statistics.source_version.kind {
        kaveon_core::SourceVersionKind::DeltaVersion { version } => serde_json::json!(version),
        _ => serde_json::Value::Null,
    }
}

/// `SHOW STATS FOR` over the table's statistics: Trino's columns, one row
/// per column and a summary row whose `column_name` is null and which
/// carries the table's `row_count` and total `data_size`; `analyzed_at` is
/// the same on every row. `distinct_values_count` is the exact count when
/// `ANALYZE … WITH (distinct = true | columns = …)` counted the column, else
/// the sketch's estimate when `ANALYZE … WITH (sketches = true)` read it,
/// else null.
fn show_stats_result(
    statistics: &kaveon_core::TableStatistics,
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
    let analyzed_at = iso_utc_ms(i64::try_from(statistics.computed_at_ms).ok());
    let mut rows = statistics
        .columns
        .iter()
        .map(|column| {
            let nulls_fraction = match column.null_count {
                Some(nulls) if statistics.rows > 0 => {
                    serde_json::json!(nulls as f64 / statistics.rows as f64)
                }
                _ => serde_json::Value::Null,
            };
            vec![
                serde_json::json!(column.name),
                serde_json::json!(kaveon_sql::ddl::sql_type_name(&column.data_type)),
                serde_json::json!(column.bytes),
                nulls_fraction,
                serde_json::json!(column.distinct_count()),
                bound_text(column.min.as_ref()),
                bound_text(column.max.as_ref()),
                serde_json::Value::Null,
                analyzed_at.clone(),
            ]
        })
        .collect::<Vec<_>>();
    rows.push(vec![
        serde_json::Value::Null,
        serde_json::Value::Null,
        serde_json::json!(statistics.bytes),
        serde_json::Value::Null,
        serde_json::Value::Null,
        serde_json::Value::Null,
        serde_json::Value::Null,
        serde_json::json!(statistics.rows),
        analyzed_at,
    ]);
    (columns, rows)
}

/// `DESCRIBE DETAIL`: the table-level facts, from the statistics on record
/// when the table was analyzed and from a fresh metadata read otherwise;
/// `catalog_snapshot` is the published catalog the statement resolved the
/// table under.
fn describe_detail_result(
    format: kaveon_core::DataFormat,
    location: &str,
    statistics: Option<&kaveon_core::TableStatistics>,
    fresh: Option<&kaveon_storage::SourceProfile>,
    catalog_snapshot: &str,
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
    let row = match (statistics, fresh) {
        (Some(statistics), _) => vec![
            serde_json::json!(format_name(format)),
            serde_json::json!(location),
            serde_json::Value::Null,
            iso_utc_ms(statistics.last_modified_ms),
            serde_json::json!(statistics.files),
            serde_json::json!(statistics.bytes),
            serde_json::json!(statistics.rows),
            delta_version_of(statistics),
            serde_json::json!(statistics.partition_columns.join(",")),
            iso_utc_ms(i64::try_from(statistics.computed_at_ms).ok()),
            serde_json::json!(catalog_snapshot),
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
            serde_json::json!(kaveon_storage::partition_column_names(profile).join(",")),
            serde_json::Value::Null,
            serde_json::json!(catalog_snapshot),
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
    let table = match resolve_statistics_table(state, &qualify_table(context, &table)).await {
        Ok(table) => table,
        Err((status, code, message)) => {
            return analyze_failure(query_id, started, status, code, message).await;
        }
    };
    let stored = match stored_table_statistics(state, &table.id) {
        Ok(stored) => stored,
        Err((status, code, message)) => {
            return analyze_failure(query_id, started, status, code, message).await;
        }
    };
    let (columns, rows) = if show_stats {
        let Some(statistics) = stored else {
            return analyze_failure(
                query_id,
                started,
                StatusCode::BAD_REQUEST,
                "STATISTICS_UNAVAILABLE",
                format!(
                    "no statistics for {}; run ANALYZE {}",
                    table.qualified, table.qualified
                ),
            )
            .await;
        };
        show_stats_result(&statistics)
    } else {
        let fresh = if stored.is_none() {
            let read = tokio::task::spawn_blocking({
                let location = table.location.clone();
                let format = table.format;
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
        describe_detail_result(
            table.format,
            &table.location,
            stored.as_deref(),
            fresh.as_ref(),
            &context.catalog_snapshot_id,
        )
    };
    finish_inline_statement(query_id, started, columns, rows).await
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

/// Files read at once when `ANALYZE … WITH (sketches = true)` reads the
/// columns.
const ANALYZE_SKETCH_THREADS: usize = 8;

/// `ANALYZE`: the table's statistics from its metadata (footers, the Delta
/// log, Iceberg manifests — no data read), or with `WITH (sketches =
/// true)` every sketchable column read once on the coordinator for the
/// distinct-count and quantile sketches and exact bounds; then — for `WITH
/// (distinct = true)` or `WITH (columns = ARRAY[…])` — one `SELECT
/// COUNT(DISTINCT "column")` statement per selected column through
/// [`run_statement`], a few at a time, cancelled with this statement; then
/// the source version is read again and the document is stored in the
/// durable catalog beside the table definition. A column not counted by
/// this statement keeps the exact count of the previous document when the
/// source version is unchanged.
async fn execute_analyze(
    state: &Arc<AppState>,
    identity: &Identity,
    query_id: &str,
    context: &QueryContext,
    statement: Result<AnalyzeStatement, String>,
    started: Instant,
    memory: AdmittedQueryMemory,
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
    let table =
        match resolve_statistics_table(state, &qualify_table(context, &statement.table)).await {
            Ok(table) => table,
            Err((status, code, message)) => {
                return analyze_failure(query_id, started, status, code, message).await;
            }
        };
    let qualified = table.qualified.clone();
    let built = tokio::task::spawn_blocking({
        let location = table.location.clone();
        let format = table.format;
        let id = table.id.clone();
        let sketches = statement.sketches;
        move || -> kaveon_core::Result<kaveon_core::TableStatistics> {
            if sketches {
                let options = kaveon_storage::FullScanOptions {
                    memory: Some(memory.pool().operator("analyze-sketches")?),
                    threads: ANALYZE_SKETCH_THREADS,
                    columns: None,
                };
                let statistics = kaveon_storage::full_statistics(&location, format, id, &options);
                drop(memory);
                statistics
            } else {
                // The metadata read reserves nothing; the permits go back
                // before the counts, which are admitted in their own right.
                drop(memory);
                kaveon_storage::metadata_statistics(&location, format, id)
            }
        }
    })
    .await;
    let mut statistics = match built {
        Ok(Ok(statistics)) => statistics,
        Ok(Err(error)) => {
            return analyze_failure(
                query_id,
                started,
                StatusCode::BAD_REQUEST,
                "ANALYZE_FAILED",
                error.to_string(),
            )
            .await;
        }
        Err(_) => {
            return analyze_failure(
                query_id,
                started,
                StatusCode::INTERNAL_SERVER_ERROR,
                "ANALYZE_FAILED",
                "statistics build did not complete".into(),
            )
            .await;
        }
    };
    let selected = match &statement.distinct {
        DistinctColumns::None => Vec::new(),
        DistinctColumns::All => statistics.column_names(),
        DistinctColumns::Named(names) => {
            if let Some(unknown) = names.iter().find(|name| statistics.column(name).is_none()) {
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
    // The counts run a few at a time: each is one distributed scan of the
    // table, and the cluster has room for more than one. The width stays
    // under the principal's statement limit so no count is refused
    // admission, and the parent's own permits are already released.
    let width = state
        .config
        .principal_query_limit
        .clamp(1, ANALYZE_COUNT_CONCURRENCY);
    let children: Vec<(String, String)> = selected
        .iter()
        .map(|column| (column.clone(), Uuid::new_v4().to_string()))
        .collect();
    let mut pending = children.iter();
    let mut counts = futures::stream::FuturesUnordered::new();
    for child in pending.by_ref().take(width) {
        counts.push(count_distinct_values(
            state,
            identity,
            query_id,
            context,
            &qualified,
            child,
            &cancellation,
        ));
    }
    let mut measured = BTreeMap::new();
    let mut failure = None;
    while let Some(result) = counts.next().await {
        match result {
            Ok((column, count)) => {
                measured.insert(column, count);
                if let Some(child) = pending.next() {
                    counts.push(count_distinct_values(
                        state,
                        identity,
                        query_id,
                        context,
                        &qualified,
                        child,
                        &cancellation,
                    ));
                }
            }
            Err(error) => {
                failure = Some(error);
                break;
            }
        }
    }
    drop(counts);
    if let Some(error) = failure {
        // The other counts still running are cancelled with the parent:
        // their tokens first, then their records and worker tasks.
        for (_, child_id) in &children {
            let _ = state.lifecycle.cancellations.cancel(child_id);
            let _ = cancel_query(
                State(Arc::clone(state)),
                Extension(identity.clone()),
                Path(child_id.clone()),
            )
            .await;
        }
        return match error {
            SubStatementError::Canceled => canceled_task_response(),
            SubStatementError::Failed {
                status,
                code,
                message,
            } => analyze_failure(query_id, started, status, &code, message).await,
        };
    }
    // The counts must have seen the version the document describes.
    let after = tokio::task::spawn_blocking({
        let location = table.location.clone();
        let format = table.format;
        move || kaveon_storage::current_source_version(&location, format)
    })
    .await;
    match after {
        Ok(Ok(after)) if statistics.is_current_for(&after.identity_sha256) => {}
        Ok(Ok(_)) => {
            return analyze_failure(
                query_id,
                started,
                StatusCode::CONFLICT,
                "SOURCE_CHANGED",
                "table source changed during ANALYZE".into(),
            )
            .await;
        }
        Ok(Err(error)) => {
            return analyze_failure(
                query_id,
                started,
                StatusCode::BAD_REQUEST,
                "ANALYZE_FAILED",
                error.to_string(),
            )
            .await;
        }
        Err(_) => {
            return analyze_failure(
                query_id,
                started,
                StatusCode::INTERNAL_SERVER_ERROR,
                "ANALYZE_FAILED",
                "source version read did not complete".into(),
            )
            .await;
        }
    }
    // What the previous document holds for the same source version stays:
    // its exact counts, and — when it read the columns and this statement
    // did not — its sketches and exact bounds. A column counted now takes
    // the new count.
    let previous = match stored_table_statistics(state, &table.id) {
        Ok(previous) => previous,
        Err((status, code, message)) => {
            return analyze_failure(query_id, started, status, code, message).await;
        }
    };
    if let Some(previous) = previous
        .filter(|previous| statistics.is_current_for(&previous.source_version.identity_sha256))
    {
        let keep_read = previous.depth == kaveon_core::StatisticsDepth::Full
            && statistics.depth == kaveon_core::StatisticsDepth::Metadata;
        for column in &mut statistics.columns {
            let Some(kept) = previous.column(&column.name) else {
                continue;
            };
            if let Some(count) = kept.distinct_exact {
                column.distinct_exact = Some(count);
            }
            if keep_read && kept.data_type == column.data_type {
                column.distinct = kept.distinct.clone();
                column.quantiles = kept.quantiles.clone();
                if kept.bounds_exact && !column.bounds_exact {
                    column.min = kept.min.clone();
                    column.max = kept.max.clone();
                    column.bounds_exact = true;
                }
                if column.null_count.is_none() {
                    column.null_count = kept.null_count;
                }
            }
        }
        if keep_read {
            statistics.depth = kaveon_core::StatisticsDepth::Full;
        }
    }
    for column in &mut statistics.columns {
        if let Some(count) = measured.get(&column.name) {
            column.distinct_exact = Some(*count);
        }
    }
    if let Err(error) = state
        .catalog_store
        .put_table_statistics(&identity.principal, &statistics)
    {
        return analyze_failure(
            query_id,
            started,
            StatusCode::SERVICE_UNAVAILABLE,
            "CATALOG_UNAVAILABLE",
            format!("statistics could not be stored: {error}"),
        )
        .await;
    }
    let columns = vec![
        varchar("table"),
        bigint("row_count"),
        bigint("distinct_columns"),
    ];
    let rows = vec![vec![
        serde_json::json!(qualified),
        serde_json::json!(statistics.rows),
        serde_json::json!(selected.len()),
    ]];
    finish_inline_statement(query_id, started, columns, rows).await
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
    .await
    .map_err(|error| (error.status, error.code, error.message));
    finish_catalog_result(query_id, result, started).await
}

/// The finished (or failed) record and response of a statement that
/// answers with a catalog-shaped result: a catalog statement, `OPTIMIZE`.
async fn finish_catalog_result(
    query_id: &str,
    result: Result<crate::catalog_ddl::CatalogStatementResult, (StatusCode, &'static str, String)>,
    started: Instant,
) -> Response {
    match result {
        Ok(result) => finish_inline_statement(query_id, started, result.columns, result.rows).await,
        Err((status, code, message)) => {
            finish_failed_query(query_id, message.clone(), started, None, None, None).await;
            (
                status,
                Json(serde_json::json!({
                    "id": query_id,
                    "error": message,
                    "code": code
                })),
            )
                .into_response()
        }
    }
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
/// How many distinct counts an `ANALYZE … WITH (distinct …)` runs at once.
const ANALYZE_COUNT_CONCURRENCY: usize = 4;

async fn count_distinct_values(
    state: &Arc<AppState>,
    identity: &Identity,
    parent_id: &str,
    context: &QueryContext,
    qualified: &str,
    child: &(String, String),
    parent: &CancellationToken,
) -> Result<(String, u64), SubStatementError> {
    let (column, child_id) = (child.0.as_str(), child.1.as_str());
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
    let child_id = child_id.to_owned();
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
        .map(|count| (column.to_owned(), count))
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
        native_analyze: state.config.coordinator,
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
    table_id: String,
    row_count: u64,
    /// The source version the statistics describe, labelled.
    source_version: String,
    depth: kaveon_core::StatisticsDepth,
    computed_at: serde_json::Value,
    /// Whether the source is still at that version.
    current: bool,
}

/// Administrators only: every table with statistics on record and whether
/// the source is still at the version they describe. Bounded to
/// [`MAX_DIAGNOSTIC_STATISTICS`] rows.
async fn statistics_diagnostics(
    State(state): State<Arc<AppState>>,
    Extension(identity): Extension<Identity>,
) -> Response {
    if identity.role != crate::security::Role::Admin {
        return StatusCode::FORBIDDEN.into_response();
    }
    let summaries = match state.catalog_store.list_table_statistics() {
        Ok(value) => value,
        Err(error) => {
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(serde_json::json!({"error": format!("cannot read statistics: {error}"), "code": "STATISTICS_UNAVAILABLE"})),
            )
                .into_response();
        }
    };
    let catalog = state.catalog.read().await.clone();
    let total = summaries.len();
    let mut statistics = Vec::with_capacity(total.min(MAX_DIAGNOSTIC_STATISTICS));
    for summary in summaries.into_iter().take(MAX_DIAGNOSTIC_STATISTICS) {
        let qualified = summary.name.qualified();
        let current = tokio::task::spawn_blocking({
            let catalog = Arc::clone(&catalog);
            let qualified = qualified.clone();
            let identity = summary.source_version.identity_sha256.clone();
            move || {
                catalog
                    .resolve_table(&kaveon_core::TableReference::parse(&qualified))
                    .ok()
                    .and_then(|resolved| {
                        kaveon_storage::current_source_version(
                            &resolved.full_path(),
                            resolved.table.format,
                        )
                        .ok()
                    })
                    .is_some_and(|version| version.identity_sha256 == identity)
            }
        })
        .await
        .unwrap_or(false);
        statistics.push(StatisticsDiagnostic {
            table: qualified,
            table_id: summary.table_id.as_str().to_owned(),
            row_count: summary.rows,
            source_version: summary.source_version.label(),
            depth: summary.depth,
            computed_at: iso_utc_ms(i64::try_from(summary.computed_at_ms).ok()),
            current,
        });
    }
    Json(serde_json::json!({"statistics":statistics,"total":total,"truncated":total > MAX_DIAGNOSTIC_STATISTICS})).into_response()
}

/// The table's statistics document with the source's version as observed
/// now: `GET /v1/catalog/tables/{id}/statistics`.
async fn get_table_statistics(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Response {
    let id = match TableId::new(id) {
        Ok(id) => id,
        Err(error) => return catalog_error_response(error),
    };
    let statistics = match state.catalog_store.table_statistics(&id) {
        Ok(Some(value)) => value,
        Ok(None) => {
            if let Ok(None) = state.catalog_store.table(&id) {
                return StatusCode::NOT_FOUND.into_response();
            }
            return (
                StatusCode::NOT_FOUND,
                Json(serde_json::json!({
                    "error": format!("no statistics for {}; run ANALYZE", id.as_str()),
                    "code": "STATISTICS_UNAVAILABLE"
                })),
            )
                .into_response();
        }
        Err(error) => return catalog_error_response(error),
    };
    let observed = match observe_table_version(&state, &id).await {
        Ok(observed) => observed,
        Err(response) => return *response,
    };
    let stale = !statistics.is_current_for(&observed.version.identity_sha256);
    Json(serde_json::json!({
        "table_id": id.as_str(),
        "table": observed.table,
        "source_version": statistics.source_version,
        "current_source_version": observed.version,
        "observed_at_ms": observed.observed_at_ms,
        "stale": stale,
        "statistics": statistics,
    }))
    .into_response()
}

/// The table's current source version, from the least metadata that
/// establishes it: `GET /v1/catalog/tables/{id}/version`. Cheap enough to
/// call before every answer.
async fn get_table_version(State(state): State<Arc<AppState>>, Path(id): Path<String>) -> Response {
    let id = match TableId::new(id) {
        Ok(id) => id,
        Err(error) => return catalog_error_response(error),
    };
    match observe_table_version(&state, &id).await {
        Ok(observed) => Json(serde_json::json!({
            "table_id": id.as_str(),
            "table": observed.table,
            "source_version": observed.version,
            "observed_at_ms": observed.observed_at_ms,
        }))
        .into_response(),
        Err(response) => *response,
    }
}

/// A table's source version as observed now.
struct ObservedVersion {
    table: String,
    version: kaveon_core::SourceVersion,
    observed_at_ms: u64,
}

async fn observe_table_version(
    state: &AppState,
    id: &TableId,
) -> Result<ObservedVersion, Box<Response>> {
    let name = match state.catalog_store.table_name(id) {
        Ok(Some(name)) => name,
        Ok(None) => return Err(Box::new(StatusCode::NOT_FOUND.into_response())),
        Err(error) => return Err(Box::new(catalog_error_response(error))),
    };
    let qualified = name.qualified();
    // The published catalog resolves the location the way a statement
    // does; a table it does not publish (a draft, a retired one) has no
    // version to observe.
    let resolved = state
        .catalog
        .read()
        .await
        .resolve_table(&kaveon_core::TableReference::parse(&qualified));
    let (location, format) = match resolved {
        Ok(resolved) => (resolved.full_path(), resolved.table.format),
        Err(_) => {
            return Err(Box::new(
                (
                    StatusCode::CONFLICT,
                    Json(serde_json::json!({
                        "error": format!("{qualified} is not published (a draft or retired table)"),
                        "code": "TABLE_NOT_PUBLISHED"
                    })),
                )
                    .into_response(),
            ));
        }
    };
    let version = tokio::task::spawn_blocking(move || {
        kaveon_storage::current_source_version(&location, format)
    })
    .await;
    match version {
        Ok(Ok(version)) => Ok(ObservedVersion {
            table: qualified,
            version,
            observed_at_ms: unix_time_ms(),
        }),
        Ok(Err(error)) => Err(Box::new(
            (
                StatusCode::BAD_GATEWAY,
                Json(serde_json::json!({
                    "error": format!("source version of {qualified} is unreadable: {error}"),
                    "code": "SOURCE_UNAVAILABLE"
                })),
            )
                .into_response(),
        )),
        Err(_) => Err(Box::new(
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({
                    "error": "source version read did not complete",
                    "code": "SOURCE_UNAVAILABLE"
                })),
            )
                .into_response(),
        )),
    }
}

/// What planning learned about one relation of the statement.
#[derive(Clone)]
struct PlannedRelation {
    table_id: Option<TableId>,
    location: String,
    format: kaveon_core::DataFormat,
    /// The source's current version and exact row count, as read now.
    current: Option<kaveon_storage::SourceStatistics>,
    /// The statistics on record, whatever version they describe.
    statistics: Option<Arc<kaveon_core::TableStatistics>>,
}

impl PlannedRelation {
    /// The statistics on record when they describe the source's current
    /// version: the only ones that may answer rather than cost.
    fn current_statistics(&self) -> Option<&Arc<kaveon_core::TableStatistics>> {
        let current = self.current.as_ref()?;
        self.statistics
            .as_ref()
            .filter(|statistics| statistics.is_current_for(&current.identity_sha256))
    }

    fn current_version(&self) -> Option<kaveon_core::SourceVersion> {
        let current = self.current.as_ref()?;
        let kind = match (self.format, current.delta_version) {
            (kaveon_core::DataFormat::Delta, version) => {
                kaveon_core::SourceVersionKind::DeltaVersion {
                    version: version.unwrap_or(0),
                }
            }
            (kaveon_core::DataFormat::Iceberg, _) => {
                // The profile's snapshot id is not on the row-count read;
                // the statistics on record name it when they are current.
                kaveon_core::SourceVersionKind::IcebergSnapshot {
                    snapshot_id: self
                        .current_statistics()
                        .and_then(|statistics| match statistics.source_version.kind {
                            kaveon_core::SourceVersionKind::IcebergSnapshot { snapshot_id } => {
                                snapshot_id
                            }
                            _ => None,
                        }),
                }
            }
            (kaveon_core::DataFormat::Parquet, _) => match &current.parquet_listing {
                Some(listing) => kaveon_core::SourceVersionKind::Listing {
                    files: listing.files.len() as u64,
                },
                None => kaveon_core::SourceVersionKind::File,
            },
        };
        Some(kaveon_core::SourceVersion {
            identity_sha256: current.identity_sha256.clone(),
            kind,
        })
    }
}

/// The relations planning read, by the name the plan scans them under.
#[derive(Clone, Default)]
struct PlanningStatistics {
    relations: BTreeMap<String, PlannedRelation>,
}

/// Tables whose statistics are being refreshed in the background, so one
/// new source version starts one refresh.
static STATISTICS_REFRESHES: std::sync::LazyLock<
    std::sync::Mutex<std::collections::HashSet<String>>,
> = std::sync::LazyLock::new(Default::default);

/// Files read at once by an automatic refresh.
const REFRESH_SCAN_THREADS: usize = 4;

/// Bring the table's statistics to the source's current version in the
/// background: files added to a full document are read and folded in, a
/// removal or a metadata-only document is recomputed at the document's
/// depth. One refresh per table at a time; a refresh that fails leaves
/// the previous document, which still costs.
fn schedule_statistics_refresh(state: &Arc<AppState>, relation: &PlannedRelation) {
    let (Some(table_id), Some(statistics)) = (&relation.table_id, &relation.statistics) else {
        return;
    };
    let key = table_id.as_str().to_owned();
    if !STATISTICS_REFRESHES.lock().unwrap().insert(key.clone()) {
        return;
    }
    let state = Arc::clone(state);
    let previous = Arc::clone(statistics);
    let location = relation.location.clone();
    let format = relation.format;
    tokio::task::spawn_blocking(move || {
        let refreshed = kaveon_storage::refresh_statistics(
            &previous,
            &location,
            format,
            &kaveon_storage::FullScanOptions {
                memory: None,
                threads: REFRESH_SCAN_THREADS,
                columns: None,
            },
        );
        match refreshed {
            Ok(next) if next.source_version != previous.source_version => {
                if let Err(error) = state
                    .catalog_store
                    .put_table_statistics("engine-statistics-refresh", &next)
                {
                    eprintln!("statistics refresh of {key} could not be stored: {error}");
                }
            }
            Ok(_) => {}
            Err(error) => eprintln!("statistics refresh of {key} failed: {error}"),
        }
        STATISTICS_REFRESHES.lock().unwrap().remove(&key);
    });
}

async fn optimize_with_durable_statistics(
    state: &Arc<AppState>,
    plan: LogicalPlan,
    catalog: &crate::PublishedCatalog,
) -> (LogicalPlan, SourcePins, PlanningStatistics) {
    let mut tables = std::collections::BTreeSet::new();
    collect_join_statistics_tables(&plan, &mut tables);
    if let Some(table) = context_answer_table(&plan) {
        tables.insert(table.to_owned());
    }
    let mut scan_predicates = BTreeMap::new();
    collect_scan_predicates(&plan, &mut scan_predicates);
    // A filtered scan is read too: its statistics, when current, say
    // which files of a directory the predicate can skip.
    for (table, scans) in &scan_predicates {
        if scans.iter().any(Option::is_some) {
            tables.insert(table.clone());
        }
    }
    if tables.is_empty() {
        return (plan, SourcePins::default(), PlanningStatistics::default());
    }
    let mut loads = tokio::task::JoinSet::new();
    for table in tables {
        let Ok(resolved) = catalog.resolve_table(&kaveon_core::TableReference::parse(&table))
        else {
            continue;
        };
        let location = resolved.full_path();
        let format = resolved.table.format;
        let table_id = state
            .catalog_store
            .table_by_name(&resolved.catalog, &resolved.schema, &resolved.table.name)
            .ok()
            .flatten()
            .map(|definition| definition.id().clone());
        let statistics = table_id
            .as_ref()
            .and_then(|id| state.catalog_store.table_statistics(id).ok().flatten())
            .map(Arc::new);
        let predicate = known_scan_predicate(&scan_predicates, &table);
        let catalog_schema = Arc::clone(&resolved.table.arrow_schema);
        loads.spawn_blocking(move || {
            let current = kaveon_storage::analyze_source(&location, format).ok();
            // A directory table under a known predicate is pinned at the
            // files that survive partition pruning and — when the
            // statistics on record are current and carry every file's
            // bounds — the files the bounds prove empty of matches; its
            // row count is the kept files': the statistics see what the
            // scan will read.
            let pruned = current.as_ref().and_then(|current| {
                let listing = current.parquet_listing.as_ref()?;
                let predicate = predicate.as_ref()?;
                let mut pruned = listing.pruned_by(Some(&catalog_schema), predicate).ok()?;
                let mut skipped = 0;
                if let Some((kept, count)) = statistics
                    .as_ref()
                    .filter(|statistics| statistics.is_current_for(&current.identity_sha256))
                    .and_then(|statistics| {
                        kaveon_storage::skip_listing_files(&pruned, statistics, predicate)
                    })
                {
                    pruned = kept;
                    skipped = count;
                }
                if pruned.files.len() == listing.files.len() {
                    return None;
                }
                let pruned = Arc::new(pruned);
                let rows = kaveon_storage::directory_row_count(
                    &location,
                    Arc::clone(&pruned),
                    Some(Arc::clone(&catalog_schema)),
                )
                .ok()?;
                Some((pruned, rows, skipped))
            });
            (
                table,
                PlannedRelation {
                    table_id,
                    location,
                    format,
                    current,
                    statistics,
                },
                pruned,
            )
        });
    }
    let mut cache = HashMap::new();
    let mut pins = SourcePins::default();
    let mut planning = PlanningStatistics::default();
    while let Some(loaded) = loads.join_next().await {
        let Ok((table, relation, pruned)) = loaded else {
            continue;
        };
        if let Some(version) = relation
            .current
            .as_ref()
            .and_then(|value| value.delta_version)
        {
            pins.delta_versions
                .insert(relation.location.clone(), version);
        }
        if let Some(listing) = pruned
            .as_ref()
            .map(|(listing, _, _)| Arc::clone(listing))
            .or_else(|| {
                relation
                    .current
                    .as_ref()
                    .and_then(|value| value.parquet_listing.clone())
            })
        {
            pins.parquet_directories
                .insert(relation.location.clone(), listing);
        }
        if let Some((_, _, skipped)) = &pruned
            && *skipped > 0
        {
            pins.files_skipped
                .insert(relation.location.clone(), *skipped);
        }
        // Statistics behind the source refresh in the background; until
        // then they still cost — they never answer.
        if state.config.statistics_auto_refresh
            && relation.statistics.is_some()
            && relation.current.is_some()
            && relation.current_statistics().is_none()
        {
            schedule_statistics_refresh(state, &relation);
        }
        let value = relation.current.as_ref().map(|current| {
            let rows = match &pruned {
                Some((_, rows, _)) => *rows,
                None => current.row_count,
            };
            kaveon_optim::statistics::RelationStatistics {
                rows,
                columns: current.columns.clone(),
                table: relation.statistics.clone(),
            }
        });
        cache.insert(table.clone(), value);
        planning.relations.insert(table, relation);
    }
    (
        kaveon_optim::statistics::optimize_with_statistics(plan, &mut |table| {
            cache.get(table).cloned().flatten()
        }),
        pins,
        planning,
    )
}

/// The aggregates a statement answered from statistics computes.
enum ContextAggregate {
    Count,
    Min(String),
    Max(String),
}

/// The scan of a `SELECT COUNT(*) | MIN(col) | MAX(col) … FROM t` with no
/// predicate and no grouping — the shape statistics can answer — or
/// `None`.
fn context_answer_table(plan: &LogicalPlan) -> Option<&str> {
    context_answer_shape(plan).map(|(table, _)| table)
}

fn context_answer_shape(plan: &LogicalPlan) -> Option<(&str, Vec<ContextAggregate>)> {
    let aggregate = match plan {
        LogicalPlan::Project { input, columns } => {
            // The projection must keep the aggregates as they are: one
            // output per aggregate, in order, renamed at most.
            let LogicalPlan::Aggregate { aggregates, .. } = input.as_ref() else {
                return None;
            };
            if columns.len() != aggregates.len()
                || !projection_preserves_aggregate_order(columns, &[], aggregates)
            {
                return None;
            }
            input.as_ref()
        }
        other => other,
    };
    let LogicalPlan::Aggregate {
        input,
        group_by,
        aggregates,
    } = aggregate
    else {
        return None;
    };
    let LogicalPlan::Scan { table, .. } = input.as_ref() else {
        return None;
    };
    if !group_by.is_empty() || aggregates.is_empty() {
        return None;
    }
    let mut shape = Vec::with_capacity(aggregates.len());
    for aggregate in aggregates {
        shape.push(match aggregate {
            AggregateExpr::Count {
                expr: kaveon_core::Expr::Star,
                distinct: false,
            } => ContextAggregate::Count,
            AggregateExpr::Min(kaveon_core::Expr::Column(column)) => {
                ContextAggregate::Min(column.clone())
            }
            AggregateExpr::Max(kaveon_core::Expr::Column(column)) => {
                ContextAggregate::Max(column.clone())
            }
            _ => return None,
        });
    }
    Some((table, shape))
}

/// The output names of a context-answerable plan, as the node-local
/// planner names them: an alias as written, else `count_*`, `min_col`,
/// `max_col`.
fn context_output_names(plan: &LogicalPlan) -> Vec<String> {
    let name_of = |aggregate: &AggregateExpr| {
        let (function, expr) = match aggregate {
            AggregateExpr::Count { expr, .. } => ("COUNT", expr),
            AggregateExpr::Sum { expr, .. } => ("SUM", expr),
            AggregateExpr::Avg { expr, .. } => ("AVG", expr),
            AggregateExpr::Min(expr) => ("MIN", expr),
            AggregateExpr::Max(expr) => ("MAX", expr),
        };
        crate::planner::agg_output_name(function, std::slice::from_ref(expr))
    };
    match plan {
        LogicalPlan::Project { columns, .. } => columns
            .iter()
            .map(|column| match column {
                kaveon_core::Expr::Alias { name, .. } => name.clone(),
                kaveon_core::Expr::Function { name, args } => {
                    crate::planner::agg_output_name(name, args)
                }
                other => format!("{other:?}"),
            })
            .collect(),
        LogicalPlan::Aggregate { aggregates, .. } => aggregates.iter().map(name_of).collect(),
        _ => Vec::new(),
    }
}

/// A statement answered from statistics: the columns and the one row.
struct ContextAnswer {
    columns: Vec<ColumnInfo>,
    row: Vec<serde_json::Value>,
    source_version: kaveon_core::SourceVersion,
    current_source_version: kaveon_core::SourceVersion,
}

/// The answer to a `COUNT(*)`, `MIN` or `MAX` statement with no predicate
/// from the table's statistics — only when they describe exactly the
/// source version the statement is pinned to, and, for a bound, when the
/// bound is the column's true extreme and its null count is known. Any
/// other case scans.
fn context_answer(plan: &LogicalPlan, planning: &PlanningStatistics) -> Option<ContextAnswer> {
    let (table, shape) = context_answer_shape(plan)?;
    let relation = planning.relations.get(table)?;
    let statistics = relation.current_statistics()?;
    let current = relation.current.as_ref()?;
    if statistics.rows != current.row_count {
        return None;
    }
    let names = context_output_names(plan);
    if names.len() != shape.len() {
        return None;
    }
    let mut columns = Vec::with_capacity(shape.len());
    let mut row = Vec::with_capacity(shape.len());
    for (aggregate, name) in shape.iter().zip(names) {
        let (data_type, value) = match aggregate {
            ContextAggregate::Count => (
                presented_type(&arrow::datatypes::DataType::UInt64),
                serde_json::json!(statistics.rows),
            ),
            ContextAggregate::Min(column) | ContextAggregate::Max(column) => {
                let column = statistics
                    .column(column)
                    .or_else(|| statistics.column(column.rsplit('.').next().unwrap_or(column)))?;
                let nulls = column.null_count?;
                let bound = match aggregate {
                    ContextAggregate::Min(_) => column.min.as_ref(),
                    _ => column.max.as_ref(),
                };
                let value = if statistics.rows == 0 || nulls == statistics.rows {
                    serde_json::Value::Null
                } else {
                    if !column.bounds_exact {
                        return None;
                    }
                    bound?.to_json()
                };
                (presented_type(&column.data_type), value)
            }
        };
        columns.push(ColumnInfo { name, data_type });
        row.push(value);
    }
    Some(ContextAnswer {
        columns,
        row,
        source_version: statistics.source_version.clone(),
        current_source_version: relation.current_version()?,
    })
}

/// The storage predicate each scan of the plan carries, per table: the
/// filter directly above the scan after pushdown, translated the way the
/// planner translates it for the reader, or `None` for a scan without one.
fn collect_scan_predicates(
    plan: &LogicalPlan,
    predicates: &mut BTreeMap<String, Vec<Option<kaveon_core::StoragePredicate>>>,
) {
    match plan {
        LogicalPlan::Filter { input, predicate } => {
            if let LogicalPlan::Scan { table, .. } = input.as_ref() {
                predicates
                    .entry(table.clone())
                    .or_default()
                    .push(kaveon_optim::rules::to_storage_predicate(predicate));
            } else {
                collect_scan_predicates(input, predicates);
            }
        }
        LogicalPlan::Scan { table, .. } => {
            predicates.entry(table.clone()).or_default().push(None);
        }
        LogicalPlan::Join { left, right, .. }
        | LogicalPlan::Intersect { left, right }
        | LogicalPlan::Except { left, right }
        | LogicalPlan::SemiJoin { left, right, .. }
        | LogicalPlan::AntiJoin { left, right, .. } => {
            collect_scan_predicates(left, predicates);
            collect_scan_predicates(right, predicates);
        }
        LogicalPlan::Project { input, .. }
        | LogicalPlan::Aggregate { input, .. }
        | LogicalPlan::Sort { input, .. }
        | LogicalPlan::Limit { input, .. }
        | LogicalPlan::Offset { input, .. }
        | LogicalPlan::Distinct { input }
        | LogicalPlan::Window { input, .. } => collect_scan_predicates(input, predicates),
        LogicalPlan::Union { inputs, .. } => {
            for input in inputs {
                collect_scan_predicates(input, predicates);
            }
        }
    }
}

/// The predicate every scan of `table` in the plan carries, when they all
/// carry the same one; a table scanned under different predicates (or
/// once without) is planned at its whole listing.
fn known_scan_predicate(
    predicates: &BTreeMap<String, Vec<Option<kaveon_core::StoragePredicate>>>,
    table: &str,
) -> Option<kaveon_core::StoragePredicate> {
    let scans = predicates.get(table)?;
    let first = scans.first()?.as_ref()?;
    scans
        .iter()
        .all(|scan| scan.as_ref() == Some(first))
        .then(|| first.clone())
}

/// Collects the relations the statistics optimizer will cost: a scan, or
/// a filter straight over one, on either side of a join. Metadata reads
/// are independent and can safely overlap; every result remains bound to
/// its own immutable source identity.
fn collect_join_statistics_tables(
    plan: &LogicalPlan,
    tables: &mut std::collections::BTreeSet<String>,
) {
    fn scan_of(plan: &LogicalPlan) -> Option<&str> {
        match plan {
            LogicalPlan::Scan { table, .. } => Some(table),
            LogicalPlan::Filter { input, .. } => match input.as_ref() {
                LogicalPlan::Scan { table, .. } => Some(table),
                _ => None,
            },
            _ => None,
        }
    }
    match plan {
        LogicalPlan::Join { left, right, .. } => {
            if let Some(table) = scan_of(left) {
                tables.insert(table.to_owned());
            }
            if let Some(table) = scan_of(right) {
                tables.insert(table.to_owned());
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
    )?
    .with_layout(value.layout().clone())?;
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
    )?
    .with_layout(value.layout().clone())?;
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
    /// Rows of a streamed root task already in the statement's pages when
    /// it failed: a retry would deliver them again, so there is none.
    rows_delivered: usize,
}

/// The message a statement fails with when a streamed root task failed
/// after some of its rows reached the pages.
const ROWS_DELIVERED_NO_RETRY: &str = " (rows already delivered; the statement is not retried)";

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
        rows_delivered: 0,
    })?;
    Ok((schema, batches, elapsed_us, output_bytes, scan, execution))
}

/// Submit a task and return its successful response; a refusal or an
/// unreachable worker is the failure, classified for retry as before.
async fn send_task_request(
    client: &reqwest::Client,
    worker: &NodeInfo,
    request: &TaskRequest,
    exchange_token: Option<&str>,
) -> Result<reqwest::Response, RemoteTaskFailure> {
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
            rows_delivered: 0,
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
            rows_delivered: 0,
        });
    }
    Ok(response)
}

/// The task metrics a collected task carries in its response headers.
/// Absence is valid for an older worker or a fragment without reader
/// telemetry.
fn task_metrics_from_headers(
    headers: &HeaderMap,
) -> (u64, Option<TaskScanMetrics>, Option<TaskExecutionMetrics>) {
    let elapsed_us = headers
        .get("x-kaveon-task-elapsed-us")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse().ok())
        .unwrap_or_default();
    let scan = headers
        .get("x-kaveon-task-scan-metrics")
        .and_then(|value| value.to_str().ok())
        .filter(|value| !value.is_empty())
        .and_then(|value| serde_json::from_str(value).ok());
    let execution = headers
        .get("x-kaveon-task-execution-metrics")
        .and_then(|value| value.to_str().ok())
        .filter(|value| !value.is_empty())
        .and_then(|value| serde_json::from_str(value).ok());
    (elapsed_us, scan, execution)
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
    let response = send_task_request(client, worker, request, exchange_token).await?;
    let (elapsed_us, scan, execution) = task_metrics_from_headers(response.headers());
    let payload = crate::transport::receive(response)
        .await
        .map_err(|message| RemoteTaskFailure {
            retryable: message.starts_with("network receive:"),
            message,
            rows_delivered: 0,
        })?;
    Ok((payload, elapsed_us, scan, execution))
}

/// What a streamed root task delivered: its rows are in the pages; what
/// is left is what the telemetry needs.
struct StreamedTaskResult {
    elapsed_us: u64,
    output_rows: usize,
    output_batches: usize,
    output_bytes: usize,
    scan: Option<TaskScanMetrics>,
    execution: Option<TaskExecutionMetrics>,
}

/// One finished task as the coordinator's loop sees it.
enum RemoteTaskOutput {
    /// The whole result, spooled: non-root tasks and inline delivery.
    Spooled {
        payload: crate::transport::ArrowPayload,
        elapsed_us: u64,
        scan: Option<TaskScanMetrics>,
        execution: Option<TaskExecutionMetrics>,
    },
    /// A root task whose rows streamed into the statement's pages.
    Streamed(StreamedTaskResult),
}

/// What the root tasks of a paged statement share while they run at once:
/// the writer their rows interleave into and the schema they must agree on.
#[derive(Clone)]
struct StreamedRootSink {
    writer: Arc<std::sync::Mutex<Option<crate::results::ResultWriter>>>,
    schema: Arc<std::sync::Mutex<Option<arrow::datatypes::SchemaRef>>>,
}

/// Takes the shared writer out when the run is left by any path that did
/// not publish it: the dropped writer leaves the `410` tombstone, and a
/// root task still streaming finds the writer gone rather than a page
/// nobody will read.
struct StreamedRootSinkGuard(Option<StreamedRootSink>);

impl Drop for StreamedRootSinkGuard {
    fn drop(&mut self) {
        if let Some(sink) = &self.0
            && let Ok(mut writer) = sink.writer.lock()
        {
            drop(writer.take());
        }
    }
}

/// The metrics of a finished task as its worker's `/metrics` reports them.
struct FinishedTaskMetrics {
    elapsed_us: u64,
    scan: Option<TaskScanMetrics>,
    execution: Option<TaskExecutionMetrics>,
}

/// The answer of a worker's `/metrics` for one task.
enum TaskMetricsAnswer {
    Finished(Box<FinishedTaskMetrics>),
    Failed(String),
    Running,
}

/// How long the coordinator waits for a task's metrics to settle after its
/// stream ended; a worker records the outcome before it ends the body, so
/// `202` here means the body was cut, not that the task is slow.
const TASK_METRICS_ATTEMPTS: usize = 20;
const TASK_METRICS_INTERVAL: Duration = Duration::from_millis(100);

async fn fetch_task_metrics(
    client: &reqwest::Client,
    worker: &NodeInfo,
    request: &TaskRequest,
    exchange_token: &str,
) -> Result<TaskMetricsAnswer, String> {
    let url = format!(
        "{}/v1/task/{}/{}/{}/{}/metrics",
        worker.address.trim_end_matches('/'),
        request.query_id,
        request.stage_id,
        request.partition_index,
        request.attempt
    );
    for attempt in 0..TASK_METRICS_ATTEMPTS {
        if attempt > 0 {
            tokio::time::sleep(TASK_METRICS_INTERVAL).await;
        }
        let response = client
            .get(&url)
            .bearer_auth(exchange_token)
            .timeout(Duration::from_secs(30))
            .send()
            .await
            .map_err(|error| format!("worker '{}' is unavailable: {error}", worker.node_id))?;
        let status = response.status();
        let body: serde_json::Value = response.json().await.unwrap_or_default();
        match status {
            StatusCode::OK => {
                return Ok(TaskMetricsAnswer::Finished(Box::new(FinishedTaskMetrics {
                    elapsed_us: body["elapsed_us"].as_u64().unwrap_or_default(),
                    scan: serde_json::from_value(body["scan"].clone()).ok(),
                    execution: serde_json::from_value(body["execution"].clone()).ok(),
                })));
            }
            StatusCode::ACCEPTED => continue,
            StatusCode::INTERNAL_SERVER_ERROR => {
                return Ok(TaskMetricsAnswer::Failed(
                    body["error"].as_str().unwrap_or("task failed").to_owned(),
                ));
            }
            _ => {
                return Err(format!(
                    "worker '{}' answered {status} for the task's metrics",
                    worker.node_id
                ));
            }
        }
    }
    Ok(TaskMetricsAnswer::Running)
}

/// The failure of a streamed root task whose body did not arrive whole:
/// the worker's own outcome when it has one — the stream ends without its
/// marker when the fragment fails — else the transport's account.
async fn streamed_task_failure(
    client: &reqwest::Client,
    worker: &NodeInfo,
    request: &TaskRequest,
    exchange_token: &str,
    streamed: bool,
    transport_message: String,
    rows_delivered: usize,
) -> RemoteTaskFailure {
    if streamed
        && let Ok(TaskMetricsAnswer::Failed(error)) =
            fetch_task_metrics(client, worker, request, exchange_token).await
    {
        return RemoteTaskFailure {
            message: format!("worker '{}' failed task: {error}", worker.node_id),
            retryable: true,
            rows_delivered,
        };
    }
    RemoteTaskFailure {
        retryable: transport_message.starts_with("network receive:"),
        message: transport_message,
        rows_delivered,
    }
}

/// Run a root task with its rows streamed into the statement's pages as
/// the worker produces them. The first task to deliver a schema publishes
/// the columns; every task's rows go to the shared writer in arrival
/// order. The task's metrics come from `/metrics` once its body ended —
/// or from the headers, for a worker that answered the old way.
async fn execute_remote_root_task_streamed(
    client: &reqwest::Client,
    worker: &NodeInfo,
    request: &TaskRequest,
    exchange_token: &str,
    query_id: &str,
    sink: &StreamedRootSink,
) -> Result<StreamedTaskResult, RemoteTaskFailure> {
    let response = send_task_request(client, worker, request, Some(exchange_token)).await?;
    let streamed = response.headers().contains_key(TASK_STREAMED_HEADER);
    let header_metrics = task_metrics_from_headers(response.headers());
    let mut stream = match crate::transport::receive_stream(response).await {
        Ok(stream) => stream,
        Err(message) => {
            return Err(streamed_task_failure(
                client,
                worker,
                request,
                exchange_token,
                streamed,
                message,
                0,
            )
            .await);
        }
    };
    let schema = stream.schema();
    let publish = {
        let mut expected = sink.writer_schema()?;
        match expected.as_ref() {
            Some(expected) if expected != &schema => {
                return Err(RemoteTaskFailure {
                    message: "root tasks returned incompatible schemas".into(),
                    retryable: false,
                    rows_delivered: 0,
                });
            }
            Some(_) => false,
            None => {
                *expected = Some(Arc::clone(&schema));
                true
            }
        }
    };
    if publish {
        // A paged reader can render page 0 the moment it lands: give the
        // running record its columns now.
        publish_columns(query_id, &column_infos(&schema)).await;
    }
    let mut output_rows = 0;
    let mut output_batches = 0;
    loop {
        match stream.next_batch().await {
            Some(Ok(batch)) => {
                output_batches += 1;
                let rows = batches_to_json(&[batch]);
                let mut writer = sink.writer.lock().map_err(|_| RemoteTaskFailure {
                    message: "result writer unavailable".into(),
                    retryable: false,
                    rows_delivered: output_rows,
                })?;
                let Some(writer) = writer.as_mut() else {
                    return Err(RemoteTaskFailure {
                        message: "the statement's result was closed while its rows streamed".into(),
                        retryable: false,
                        rows_delivered: output_rows,
                    });
                };
                for row in rows {
                    writer.push(row).map_err(|error| RemoteTaskFailure {
                        message: error.to_string(),
                        retryable: false,
                        rows_delivered: output_rows,
                    })?;
                    output_rows += 1;
                }
            }
            Some(Err(message)) => {
                return Err(streamed_task_failure(
                    client,
                    worker,
                    request,
                    exchange_token,
                    streamed,
                    message,
                    output_rows,
                )
                .await);
            }
            None => break,
        }
    }
    let output_bytes = stream.bytes() as usize;
    let (elapsed_us, scan, execution) = if streamed {
        match fetch_task_metrics(client, worker, request, exchange_token).await {
            Ok(TaskMetricsAnswer::Finished(metrics)) => {
                (metrics.elapsed_us, metrics.scan, metrics.execution)
            }
            // The rows arrived whole; the task's telemetry did not. The
            // record shows the scan totals as incomplete.
            Ok(TaskMetricsAnswer::Running) => (0, None, None),
            Ok(TaskMetricsAnswer::Failed(error)) => {
                return Err(RemoteTaskFailure {
                    message: format!(
                        "worker '{}' reported a failure after streaming a complete result: {error}",
                        worker.node_id
                    ),
                    retryable: false,
                    rows_delivered: output_rows,
                });
            }
            Err(error) => {
                eprintln!(
                    "task {}/{}/{} metrics unavailable: {error}",
                    request.query_id, request.stage_id, request.partition_index
                );
                (0, None, None)
            }
        }
    } else {
        header_metrics
    };
    Ok(StreamedTaskResult {
        elapsed_us,
        output_rows,
        output_batches,
        output_bytes,
        scan,
        execution,
    })
}

impl StreamedRootSink {
    fn writer_schema(
        &self,
    ) -> Result<std::sync::MutexGuard<'_, Option<arrow::datatypes::SchemaRef>>, RemoteTaskFailure>
    {
        self.schema.lock().map_err(|_| RemoteTaskFailure {
            message: "result schema unavailable".into(),
            retryable: false,
            rows_delivered: 0,
        })
    }
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
    // Paged delivery: the root tasks stream their rows into the writer as
    // they run, several at once. The guard takes the writer back out on
    // every exit but the publish.
    let streamed_sink = result_writer.take().map(|writer| StreamedRootSink {
        writer: Arc::new(std::sync::Mutex::new(Some(writer))),
        schema: Arc::new(std::sync::Mutex::new(None)),
    });
    let sink_guard = StreamedRootSinkGuard(streamed_sink.clone());

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
            let mut request = task_request_from_dispatch(&dispatch, context);
            let root = dispatch.exchange_outputs.is_empty();
            let root_sink = if root { streamed_sink.clone() } else { None };
            request.stream_result = root_sink.is_some();
            let client = client.clone();
            let token = token.clone();
            let query_id = query_id.to_owned();
            tasks.spawn(async move {
                let result = match root_sink {
                    Some(sink) => execute_remote_root_task_streamed(
                        &client, &worker, &request, &token, &query_id, &sink,
                    )
                    .await
                    .map(RemoteTaskOutput::Streamed),
                    None => execute_remote_task_payload(&client, &worker, &request, Some(&token))
                        .await
                        .map(
                            |(payload, elapsed_us, scan, execution)| RemoteTaskOutput::Spooled {
                                payload,
                                elapsed_us,
                                scan,
                                execution,
                            },
                        ),
                };
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
                Ok(output) => {
                    let (elapsed_us, output_rows, output_batches, output_bytes, scan, execution) =
                        match output {
                            RemoteTaskOutput::Streamed(streamed) => (
                                streamed.elapsed_us,
                                streamed.output_rows,
                                streamed.output_batches,
                                streamed.output_bytes,
                                streamed.scan,
                                streamed.execution,
                            ),
                            RemoteTaskOutput::Spooled {
                                mut payload,
                                elapsed_us,
                                scan,
                                execution,
                            } => {
                                let schema = payload.schema();
                                let output_bytes = payload.bytes();
                                let mut output_rows = 0;
                                let mut output_batches = 0;
                                // A spooled root is inline delivery: the
                                // rows collect here under the inline cap.
                                let root = dispatch.exchange_outputs.is_empty();
                                if root {
                                    if result_schema
                                        .as_ref()
                                        .is_some_and(|expected| expected != &schema)
                                    {
                                        orchestrator.cancel();
                                        return Some(Err(
                                            "root tasks returned incompatible schemas".into(),
                                        ));
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
                                        result_bytes = result_bytes
                                            .saturating_add(batch.get_array_memory_size());
                                        if result_bytes > 16 * 1024 * 1024 {
                                            return Some(Err("inline results exceed 16 MiB; request result_delivery=paged".into()));
                                        }
                                        result_batches.push(batch);
                                    }
                                }
                                (
                                    elapsed_us,
                                    output_rows,
                                    output_batches,
                                    output_bytes,
                                    scan,
                                    execution,
                                )
                            }
                        };
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
                    // Rows of a streamed root task are already in the pages:
                    // running it again would deliver them twice.
                    if failure.retryable && failure.rows_delivered > 0 {
                        orchestrator.cancel();
                        return Some(Err(format!("{}{ROWS_DELIVERED_NO_RETRY}", failure.message)));
                    }
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
    let streamed_schema = streamed_sink
        .as_ref()
        .and_then(|sink| sink.schema.lock().ok().and_then(|schema| schema.clone()));
    let Some(schema) = result_schema.or(streamed_schema) else {
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
    let data = if let Some(sink) = &streamed_sink {
        let writer = sink.writer.lock().ok().and_then(|mut writer| writer.take());
        let Some(writer) = writer else {
            return Some(Err(
                "the statement's result was closed before it completed".into()
            ));
        };
        if let Err(error) = state.results.publish(query_id, writer) {
            return Some(Err(error.to_string()));
        }
        Vec::new()
    } else {
        batches_to_json(&result_batches)
    };
    drop(sink_guard);
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
        stream_result: false,
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
                    stream_result: false,
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
                    stream_result: false,
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
                    // Temporal and decimal values as their logical text —
                    // ISO 8601 dates, timestamps and times, exact decimal
                    // digits — the rendering the statistics use, so an
                    // answer from statistics reads as a scanned one.
                    DataType::Date32 => {
                        kaveon_storage::StatValue::Date(arr.as_primitive::<Date32Type>().value(row))
                            .to_json()
                    }
                    DataType::Date64 => kaveon_storage::StatValue::Date(
                        arr.as_primitive::<Date64Type>()
                            .value(row)
                            .div_euclid(86_400_000) as i32,
                    )
                    .to_json(),
                    DataType::Timestamp(unit, zone) => {
                        let value = match unit {
                            TimeUnit::Second => {
                                arr.as_primitive::<TimestampSecondType>().value(row)
                            }
                            TimeUnit::Millisecond => {
                                arr.as_primitive::<TimestampMillisecondType>().value(row)
                            }
                            TimeUnit::Microsecond => {
                                arr.as_primitive::<TimestampMicrosecondType>().value(row)
                            }
                            TimeUnit::Nanosecond => {
                                arr.as_primitive::<TimestampNanosecondType>().value(row)
                            }
                        };
                        kaveon_storage::StatValue::Timestamp {
                            value,
                            unit: *unit,
                            utc: zone.is_some(),
                        }
                        .to_json()
                    }
                    DataType::Time32(unit) => {
                        let value = match unit {
                            TimeUnit::Second => arr.as_primitive::<Time32SecondType>().value(row),
                            _ => arr.as_primitive::<Time32MillisecondType>().value(row),
                        };
                        kaveon_storage::StatValue::Time {
                            value: i64::from(value),
                            unit: *unit,
                        }
                        .to_json()
                    }
                    DataType::Time64(unit) => {
                        let value = match unit {
                            TimeUnit::Microsecond => {
                                arr.as_primitive::<Time64MicrosecondType>().value(row)
                            }
                            _ => arr.as_primitive::<Time64NanosecondType>().value(row),
                        };
                        kaveon_storage::StatValue::Time { value, unit: *unit }.to_json()
                    }
                    DataType::Decimal128(_, scale) => kaveon_storage::StatValue::Decimal {
                        unscaled: arr.as_primitive::<Decimal128Type>().value(row),
                        scale: *scale,
                    }
                    .to_json(),
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
        files_pruned_by_partition: snapshot.files_pruned_by_partition,
        files_skipped: snapshot.files_skipped,
        decoded_batch_cache_hits: snapshot.decoded_batch_cache_hits,
        decoded_batch_cache_misses: snapshot.decoded_batch_cache_misses,
        decoded_batch_cache_evictions: snapshot.decoded_batch_cache_evictions,
        decoded_batch_cache_singleflight_waits: snapshot.decoded_batch_cache_singleflight_waits,
        row_groups_considered: snapshot.row_groups_considered,
        row_groups_read: snapshot.row_groups_selected,
        row_groups_pruned: snapshot.row_groups_pruned(),
        row_groups_pruned_by_bloom: snapshot.row_groups_pruned_by_bloom,
        bloom_filters_read: snapshot.bloom_filters_read,
        bloom_filter_bytes_read: snapshot.bloom_filter_bytes_read,
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
        total.files_pruned_by_partition += snapshot.files_pruned_by_partition;
        total.files_skipped += snapshot.files_skipped;
        total.decoded_batch_cache_hits += snapshot.decoded_batch_cache_hits;
        total.decoded_batch_cache_misses += snapshot.decoded_batch_cache_misses;
        total.decoded_batch_cache_evictions += snapshot.decoded_batch_cache_evictions;
        total.decoded_batch_cache_singleflight_waits +=
            snapshot.decoded_batch_cache_singleflight_waits;
        total.row_groups_considered += snapshot.row_groups_considered;
        total.row_groups_selected += snapshot.row_groups_selected;
        total.row_groups_pruned_by_bloom += snapshot.row_groups_pruned_by_bloom;
        total.bloom_filters_read += snapshot.bloom_filters_read;
        total.bloom_filter_bytes_read += snapshot.bloom_filter_bytes_read;
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
            total.files_pruned_by_partition += scan.files_pruned_by_partition;
            total.files_skipped += scan.files_skipped;
            total.decoded_batch_cache_hits += scan.decoded_batch_cache_hits;
            total.decoded_batch_cache_misses += scan.decoded_batch_cache_misses;
            total.decoded_batch_cache_evictions += scan.decoded_batch_cache_evictions;
            total.decoded_batch_cache_singleflight_waits +=
                scan.decoded_batch_cache_singleflight_waits;
            total.row_groups_considered += scan.row_groups_considered;
            total.row_groups_selected += scan.row_groups_selected;
            total.row_groups_pruned_by_bloom += scan.row_groups_pruned_by_bloom;
            total.bloom_filters_read += scan.bloom_filters_read;
            total.bloom_filter_bytes_read += scan.bloom_filter_bytes_read;
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
            files_pruned_by_partition: total.files_pruned_by_partition,
            files_skipped: total.files_skipped,
            decoded_batch_cache_hits: total.decoded_batch_cache_hits,
            decoded_batch_cache_misses: total.decoded_batch_cache_misses,
            decoded_batch_cache_evictions: total.decoded_batch_cache_evictions,
            decoded_batch_cache_singleflight_waits: total.decoded_batch_cache_singleflight_waits,
            row_groups_considered: total.row_groups_considered,
            row_groups_read: total.row_groups_selected,
            row_groups_pruned: total
                .row_groups_considered
                .saturating_sub(total.row_groups_selected),
            row_groups_pruned_by_bloom: total.row_groups_pruned_by_bloom,
            bloom_filters_read: total.bloom_filters_read,
            bloom_filter_bytes_read: total.bloom_filter_bytes_read,
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
            stream_result: false,
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
        encode_arrow_stream, exact_metadata_count_plan, execute_analyze,
        general_distributed_eligible, iso_utc_ms, merge_partial_aggregates, mutation_actor,
        parse_analyze_statement, parse_statistics_statement, statistics_diagnostics,
        task_request_from_dispatch, top_n_merge_contract, transaction_api_guidance,
        validate_replacement,
    };
    use crate::security::Role;
    use arrow::array::{Int64Array, StringArray};
    use axum::http::StatusCode;

    fn analyze(sql: &str) -> Result<AnalyzeStatement, String> {
        parse_analyze_statement(sql).expect("an ANALYZE statement")
    }

    fn analyze_of(table: &str, distinct: DistinctColumns) -> Result<AnalyzeStatement, String> {
        Ok(AnalyzeStatement {
            table: table.into(),
            distinct,
            sketches: false,
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
            "unknown ANALYZE property 'sample'; the properties are distinct, columns and sketches"
        );
        assert_eq!(
            analyze("ANALYZE orders WITH (sketches = true)"),
            Ok(AnalyzeStatement {
                table: "orders".into(),
                distinct: DistinctColumns::None,
                sketches: true,
            })
        );
        assert_eq!(
            analyze("ANALYZE orders WITH (columns = ARRAY['a'], sketches = TRUE)"),
            Ok(AnalyzeStatement {
                table: "orders".into(),
                distinct: named(&["a"]),
                sketches: true,
            })
        );
        assert_eq!(
            error("ANALYZE orders WITH (sketches = maybe)"),
            "ANALYZE property sketches must be true or false, not maybe"
        );
        assert_eq!(
            error("ANALYZE orders WITH (sketches = true, sketches = false)"),
            "ANALYZE property sketches is given twice"
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
    async fn statistics_statements_read_the_stored_statistics() {
        let (state, directory) = analyze_test_state().await;
        let admin = admin();
        let analyst = crate::security::Identity {
            principal: "analyst".into(),
            display_identity: None,
            role: Role::Analyst,
        };
        let file_bytes = std::fs::metadata(directory.join("orders.parquet"))
            .unwrap()
            .len();
        let catalog_snapshot = state.catalog.read().await.snapshot_id.clone();

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
        assert_eq!(row[10], catalog_snapshot);
        assert_eq!(body["data"].as_array().unwrap().len(), 1);

        // ANALYZE keeps its result and stores the statistics beside the
        // table definition, versioned by the source version.
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
        let stored = stored_statistics(&state).expect("statistics on record");
        assert_eq!(
            stored.version,
            kaveon_core::statistics::TABLE_STATISTICS_VERSION
        );
        assert_eq!(stored.depth, kaveon_core::StatisticsDepth::Metadata);
        assert_eq!(stored.format, kaveon_core::DataFormat::Parquet);
        assert!(stored.location.ends_with("orders.parquet"));
        assert_eq!(
            stored.source_version.kind,
            kaveon_core::SourceVersionKind::File
        );
        assert_eq!(
            stored.source_version.identity_sha256,
            kaveon_storage::analyze_source(&stored.location, stored.format)
                .unwrap()
                .identity_sha256
        );
        assert!(stored.computed_at_ms > 0);
        assert_eq!(stored.rows, 3);
        assert_eq!(stored.files, 1);
        assert_eq!(stored.row_groups, Some(1));
        assert_eq!(stored.bytes, file_bytes);
        assert!(stored.uncompressed_bytes.unwrap() > 0);
        assert!(stored.last_modified_ms.unwrap() > 0);
        assert!(stored.partition_columns.is_empty());
        assert!(stored.per_file_complete);
        assert_eq!(stored.per_file.len(), 1);
        let column = &stored.columns[0];
        assert_eq!(column.name, "id");
        assert_eq!(column.data_type, DataType::Int64);
        assert_eq!(column.null_count, Some(0));
        assert_eq!(column.min, Some(kaveon_core::StatValue::Int(1)));
        assert_eq!(column.max, Some(kaveon_core::StatValue::Int(3)));
        assert!(column.bounds_exact);
        assert!(column.bytes.unwrap() > 0);
        assert!(column.distinct_count().is_none());
        assert_eq!(stored.columns.len(), 1);

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
        let analyzed_at = iso_utc_ms(Some(stored.computed_at_ms as i64));
        assert!(analyzed_at.as_str().unwrap().ends_with('Z'));
        assert_eq!(
            body["data"],
            serde_json::json!([
                [
                    "id",
                    "bigint",
                    column.bytes,
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

        // DESCRIBE DETAIL after ANALYZE answers from the statistics.
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
        assert_eq!(row[10], catalog_snapshot);

        // The statistics endpoint: the document, the version on record and
        // the version observed now.
        let table_id = stored.table_id.as_str().to_owned();
        let response = super::get_table_statistics(
            axum::extract::State(state.clone()),
            axum::extract::Path(table_id.clone()),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        let body = json_body(response).await;
        assert_eq!(body["table"], "lake.sales.orders");
        assert_eq!(body["table_id"], table_id);
        assert_eq!(body["stale"], false);
        assert_eq!(body["source_version"], body["current_source_version"]);
        assert_eq!(body["source_version"]["kind"], "file");
        assert!(body["observed_at_ms"].as_u64().unwrap() > 0);
        assert_eq!(body["statistics"]["rows"], 3);
        assert_eq!(body["statistics"]["columns"][0]["name"], "id");
        let response = super::get_table_version(
            axum::extract::State(state.clone()),
            axum::extract::Path(table_id.clone()),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        let version = json_body(response).await;
        assert_eq!(version["table"], "lake.sales.orders");
        assert_eq!(version["source_version"], body["current_source_version"]);
        assert!(version["observed_at_ms"].as_u64().unwrap() > 0);

        // The source replaced: the version endpoint sees the new identity,
        // the statistics endpoint reports the record stale.
        write_orders(&directory, &[10, 11, 12, 13]);
        let response = super::get_table_version(
            axum::extract::State(state.clone()),
            axum::extract::Path(table_id.clone()),
        )
        .await;
        let changed = json_body(response).await;
        assert_ne!(changed["source_version"], version["source_version"]);
        let response = super::get_table_statistics(
            axum::extract::State(state.clone()),
            axum::extract::Path(table_id.clone()),
        )
        .await;
        let body = json_body(response).await;
        assert_eq!(body["stale"], true);
        assert_eq!(body["current_source_version"], changed["source_version"]);
        assert_eq!(body["source_version"], version["source_version"]);
        let response = super::get_table_statistics(
            axum::extract::State(state.clone()),
            axum::extract::Path("table:nope".to_owned()),
        )
        .await;
        assert_eq!(response.status(), StatusCode::NOT_FOUND);

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
        // A filter straight over a scan is costed from the scan's statistics.
        let plan = kaveon_sql::logical_plan::sql_to_logical_plan(
            "SELECT * FROM events e JOIN customers c ON e.customer_id = c.id WHERE e.id = 1",
        )
        .unwrap();
        let plan = kaveon_optim::rules::push_filter_down(plan);
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
        let (state, directory) = admission_test_state(2).await;
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
        let (state, directory) = admission_test_state(1).await;
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
        let (state, directory) = admission_test_state(2).await;
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
    async fn admission_test_state(queue: usize) -> (crate::AppState, std::path::PathBuf) {
        let (state, directory) = analyze_test_state().await;
        let mut state = Arc::try_unwrap(state).unwrap_or_else(|_| unreachable!());
        state.config.query_memory_limit_bytes = 1 << 20;
        state.config.memory_admission_limit_bytes = 1 << 20;
        state.config.memory_admission_queue = queue;
        state.config.memory_admission_wait_seconds = 60;
        state.memory_admission = kaveon_core::MemoryAdmissionController::new(1 << 20)
            .unwrap()
            .with_queue_limit(queue);
        (state, directory)
    }

    async fn analyze_test_state() -> (Arc<crate::AppState>, std::path::PathBuf) {
        let schema = Arc::new(Schema::new(vec![Field::new("id", DataType::Int64, false)]));
        let batch = RecordBatch::try_new(
            schema.clone(),
            vec![Arc::new(Int64Array::from(vec![1, 2, 3]))],
        )
        .unwrap();
        analyze_test_state_over(schema, batch).await
    }

    /// `orders.parquet` under `directory` holding one `id` column of
    /// `values`.
    fn write_orders(directory: &std::path::Path, values: &[i64]) {
        let schema = Arc::new(Schema::new(vec![Field::new("id", DataType::Int64, false)]));
        let batch = RecordBatch::try_new(
            schema.clone(),
            vec![Arc::new(Int64Array::from(values.to_vec()))],
        )
        .unwrap();
        write_batch(&directory.join("orders.parquet"), schema, &batch);
    }

    fn write_batch(path: &std::path::Path, schema: Arc<Schema>, batch: &RecordBatch) {
        let mut writer = parquet::arrow::ArrowWriter::try_new(
            std::fs::File::create(path).unwrap(),
            schema,
            None,
        )
        .unwrap();
        writer.write(batch).unwrap();
        writer.close().unwrap();
    }

    /// A coordinator over the durable catalog with one Parquet table
    /// `lake.sales.orders` holding `batch`, registered through the catalog
    /// statements the way a client registers one.
    async fn analyze_test_state_over(
        schema: Arc<Schema>,
        batch: RecordBatch,
    ) -> (Arc<crate::AppState>, std::path::PathBuf) {
        analyze_test_state_configured(schema, batch, |_| {}).await
    }

    /// [`analyze_test_state_over`] with the server configuration adjusted
    /// before the state is built.
    async fn analyze_test_state_configured(
        schema: Arc<Schema>,
        batch: RecordBatch,
        configure: impl FnOnce(&mut crate::config::ServerConfig),
    ) -> (Arc<crate::AppState>, std::path::PathBuf) {
        let directory =
            std::env::temp_dir().join(format!("kaveon-server-analyze-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&directory).unwrap();
        write_batch(&directory.join("orders.parquet"), schema, &batch);
        let mut state = catalog_test_state();
        configure(&mut state.config);
        let catalog = kaveon_core::CatalogDefinition::new(
            kaveon_core::CatalogId::new("catalog:lake").unwrap(),
            "lake",
            kaveon_core::CatalogAdapter::Native,
            kaveon_core::StorageType::Local {
                base_path: directory.clone(),
            },
        )
        .unwrap()
        .transition(kaveon_core::CatalogLifecycle::Active)
        .unwrap();
        state
            .catalog_store
            .create_catalog("test", &catalog)
            .unwrap();
        let state = Arc::new(state);
        for sql in [
            "CREATE SCHEMA lake.sales",
            "CREATE TABLE orders WITH (location = 'orders.parquet', format = 'parquet')",
        ] {
            let (status, body) = submit(&state, &admin(), sql, serde_json::Value::Null).await;
            assert_eq!(status, StatusCode::OK, "{sql}: {body}");
        }
        (state, directory)
    }

    /// The statistics on record for `lake.sales.orders`.
    fn stored_statistics(state: &crate::AppState) -> Option<kaveon_core::TableStatistics> {
        let table = state
            .catalog_store
            .table_by_name("lake", "sales", "orders")
            .unwrap()
            .expect("the orders table");
        state.catalog_store.table_statistics(table.id()).unwrap()
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
        let (state, _directory) = analyze_test_state().await;
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
        let (state, _directory) = analyze_test_state().await;
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
        let (state, directory) = analyze_test_state().await;
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
        let (state, directory) = analyze_test_state().await;
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

    /// The stored statistics' exact distinct count per column name.
    fn stored_distinct(state: &crate::AppState) -> Vec<(String, serde_json::Value)> {
        let statistics = stored_statistics(state).expect("statistics on record");
        assert_eq!(
            statistics.version,
            kaveon_core::statistics::TABLE_STATISTICS_VERSION
        );
        statistics
            .columns
            .iter()
            .map(|column| {
                (
                    column.name.clone(),
                    serde_json::json!(column.distinct_exact),
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
        let (state, directory) = analyze_test_state_over(schema, batch).await;
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
            stored_distinct(&state),
            [
                ("id".to_owned(), serde_json::json!(3)),
                ("region".into(), serde_json::json!(2)),
                ("amount".into(), serde_json::json!(2)),
            ]
        );
        assert_eq!(state.result_cache.stats().entries, 0);
        let parent = body["id"].as_str().unwrap().to_owned();
        let children = sub_statement_records(&parent).await;
        // The counts run a few at a time, so their records finish in any order.
        let mut child_sql: Vec<&str> = children.iter().map(|record| record.sql.as_str()).collect();
        child_sql.sort_unstable();
        assert_eq!(
            child_sql,
            [
                "SELECT COUNT(DISTINCT \"amount\") FROM lake.sales.orders",
                "SELECT COUNT(DISTINCT \"id\") FROM lake.sales.orders",
                "SELECT COUNT(DISTINCT \"region\") FROM lake.sales.orders",
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
            stored_distinct(&state),
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
            stored_distinct(&state),
            [
                ("id".to_owned(), serde_json::json!(3)),
                ("region".into(), serde_json::json!(2)),
                ("amount".into(), serde_json::json!(2)),
            ]
        );

        // An unknown column is refused before any count, the record failed,
        // the document untouched; so are both properties together.
        let before = stored_statistics(&state).unwrap();
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
        assert_eq!(stored_statistics(&state).unwrap(), before);

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
        write_batch(&directory.join("orders.parquet"), schema, &batch);
        let (status, body) =
            submit(&state, &admin, "ANALYZE orders", serde_json::Value::Null).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(
            body["data"],
            serde_json::json!([["lake.sales.orders", 2, 0]])
        );
        assert_eq!(
            stored_distinct(&state),
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
            stored_distinct(&state),
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
        let (state, directory) = analyze_test_state_over(schema, batch).await;
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
        assert!(stored_statistics(&state).is_none());
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[tokio::test]
    async fn analyze_requires_admin_and_stores_versioned_statistics() {
        let (state, directory) = analyze_test_state().await;
        let reader = crate::security::Identity {
            principal: "reader".into(),
            display_identity: None,
            role: Role::Reader,
        };
        let memory = || {
            state
                .memory_admission
                .admit(uuid::Uuid::new_v4().to_string(), 1 << 20)
                .unwrap()
        };
        let denied = execute_analyze(
            &state,
            &reader,
            "denied",
            &analyze_context(),
            analyze("ANALYZE orders"),
            std::time::Instant::now(),
            memory(),
        )
        .await;
        assert_eq!(denied.status(), axum::http::StatusCode::FORBIDDEN);
        assert!(stored_statistics(&state).is_none());
        let admin = admin();
        let response = execute_analyze(
            &state,
            &admin,
            "allowed",
            &analyze_context(),
            analyze("ANALYZE orders"),
            std::time::Instant::now(),
            memory(),
        )
        .await;
        assert_eq!(response.status(), axum::http::StatusCode::OK);
        let stored = stored_statistics(&state).unwrap();
        assert_eq!(stored.rows, 3);
        let diagnostic = statistics_diagnostics(
            axum::extract::State(state.clone()),
            axum::Extension(admin.clone()),
        )
        .await;
        let json = json_body(diagnostic).await;
        assert_eq!(json["total"], 1);
        assert_eq!(json["truncated"], false);
        assert_eq!(json["statistics"][0]["table"], "lake.sales.orders");
        assert_eq!(json["statistics"][0]["table_id"], stored.table_id.as_str());
        assert_eq!(json["statistics"][0]["row_count"], 3);
        assert_eq!(json["statistics"][0]["depth"], "metadata");
        assert_eq!(json["statistics"][0]["current"], true);
        assert!(
            json["statistics"][0]["source_version"]
                .as_str()
                .unwrap()
                .starts_with("file (")
        );
        assert!(
            json["statistics"][0]["computed_at"]
                .as_str()
                .unwrap()
                .ends_with('Z')
        );
        // The source replaced: the record is stale until the next ANALYZE.
        write_orders(&directory, &[10, 11, 12, 13]);
        let stale =
            statistics_diagnostics(axum::extract::State(state.clone()), axum::Extension(admin))
                .await;
        let json = json_body(stale).await;
        assert_eq!(json["statistics"][0]["current"], false);
        assert_eq!(json["statistics"][0]["row_count"], 3);
        let enabled = capabilities(axum::extract::State(state)).await.0;
        assert!(enabled.native_analyze);
        assert!(!enabled.transactions.enabled);
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
    async fn native_analyze_is_a_coordinator_capability() {
        let mut state = catalog_test_state();
        state.config.coordinator = false;
        let state = Arc::new(state);
        let capabilities = capabilities(axum::extract::State(state)).await.0;
        assert!(!capabilities.native_analyze);
        assert!(!capabilities.transactions.enabled);
    }

    /// `events/` under the lake directory: three files of `rows` rows each
    /// over `id` (dense from `start`), `score` (null every seventh row),
    /// `name` (null every fifth), `day` (dates), registered as
    /// `lake.sales.events`.
    async fn register_events_directory(
        state: &Arc<crate::AppState>,
        directory: &std::path::Path,
        rows: i64,
    ) {
        let events = directory.join("events");
        std::fs::create_dir_all(&events).unwrap();
        for (index, name) in ["a", "b", "c"].iter().enumerate() {
            let start = index as i64 * rows;
            write_events_file(&events.join(format!("{name}.parquet")), start, rows);
        }
        let (status, body) = submit(
            state,
            &admin(),
            "CREATE TABLE events WITH (location = 'events', format = 'parquet')",
            serde_json::Value::Null,
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{body}");
    }

    fn events_schema() -> Arc<Schema> {
        Arc::new(Schema::new(vec![
            Field::new("id", DataType::Int64, false),
            Field::new("score", DataType::Float64, true),
            Field::new("name", DataType::Utf8, true),
            Field::new("day", DataType::Date32, false),
        ]))
    }

    fn write_events_file(path: &std::path::Path, start: i64, rows: i64) {
        use arrow::array::{Date32Array, Float64Array};
        let schema = events_schema();
        let ids = (start..start + rows).collect::<Vec<_>>();
        let batch = RecordBatch::try_new(
            schema.clone(),
            vec![
                Arc::new(Int64Array::from(ids.clone())),
                Arc::new(Float64Array::from(
                    ids.iter()
                        .map(|id| (id % 7 != 3).then_some(*id as f64 * 0.5 + 0.25))
                        .collect::<Vec<_>>(),
                )),
                Arc::new(StringArray::from(
                    ids.iter()
                        .map(|id| (id % 5 != 2).then(|| format!("name-{:04}", id % 97)))
                        .collect::<Vec<_>>(),
                )),
                Arc::new(Date32Array::from(
                    ids.iter()
                        .map(|id| 19_700 + (*id as i32 % 400))
                        .collect::<Vec<_>>(),
                )),
            ],
        )
        .unwrap();
        write_batch(path, schema, &batch);
    }

    /// The statistics on record for `lake.sales.events`.
    fn stored_events_statistics(state: &crate::AppState) -> Option<kaveon_core::TableStatistics> {
        let table = state
            .catalog_store
            .table_by_name("lake", "sales", "events")
            .unwrap()
            .expect("the events table");
        state.catalog_store.table_statistics(table.id()).unwrap()
    }

    /// A statement's response and its finished record, the result cache
    /// off so every run is planned and placed afresh.
    async fn submit_and_record(
        state: &Arc<crate::AppState>,
        sql: &str,
    ) -> (serde_json::Value, serde_json::Value) {
        let (status, body) = submit(
            state,
            &admin(),
            sql,
            serde_json::json!({"result_cache": false}),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{sql}: {body}");
        let record = record(body["id"].as_str().unwrap(), &admin()).await;
        (body, record)
    }

    /// Every statement statistics may answer equals the scanned answer —
    /// columns and rows — and is answered from statistics only while they
    /// describe the statement's pinned version exactly.
    #[tokio::test]
    async fn context_answers_equal_the_scanned_answers_and_refuse_a_changed_source() {
        let (state, directory) = analyze_test_state().await;
        register_events_directory(&state, &directory, 100).await;
        let statements = [
            "SELECT COUNT(*) FROM events",
            "SELECT COUNT(*) AS n FROM events",
            "SELECT MIN(id), MAX(id) FROM events",
            "SELECT MIN(score), MAX(score) AS top FROM events",
            "SELECT MIN(name), MAX(name) FROM events",
            "SELECT MIN(day), MAX(day) FROM events",
            "SELECT COUNT(*), MIN(id), MAX(day) FROM events",
        ];
        // Scanned: no statistics on record yet.
        let mut scanned = Vec::new();
        for sql in statements {
            let (body, record) = submit_and_record(&state, sql).await;
            assert_ne!(record["execution"]["mode"], "context", "{sql}");
            scanned.push((body["columns"].clone(), body["data"].clone()));
        }
        assert_eq!(scanned[0].1, serde_json::json!([[300]]));
        assert_eq!(scanned[2].1, serde_json::json!([[0, 299]]));

        // From statistics: the same columns and rows, no scan, the version
        // on the record.
        let (_, body) = submit(&state, &admin(), "ANALYZE events", serde_json::Value::Null).await;
        assert_eq!(body["data"][0][1], 300, "{body}");
        let stored = stored_events_statistics(&state).unwrap();
        let mut from_context = 0;
        for (sql, (columns, data)) in statements.iter().zip(&scanned) {
            let (body, record) = submit_and_record(&state, sql).await;
            assert_eq!(&body["columns"], columns, "{sql}");
            assert_eq!(&body["data"], data, "{sql}");
            assert_eq!(record["columns"], *columns, "{sql}");
            assert_eq!(record["rows"], *data, "{sql}");
            if record["execution"]["mode"] == "context" {
                from_context += 1;
                assert_eq!(
                    record["execution"]["detail"],
                    format!("statistics at {}", stored.source_version.label()),
                    "{sql}"
                );
                assert_eq!(
                    record["execution"]["source_version"],
                    serde_json::to_value(&stored.source_version).unwrap(),
                    "{sql}"
                );
                assert_eq!(
                    record["execution"]["current_source_version"],
                    record["execution"]["source_version"],
                    "{sql}"
                );
                assert_eq!(record["execution"]["source_version"]["kind"], "listing");
                assert_eq!(record["execution"]["source_version"]["files"], 3);
                assert!(record["scans"].as_array().unwrap().is_empty(), "{sql}");
            }
        }
        // Counts and numeric, text and date bounds all came from the
        // statistics: every statement of the set.
        assert_eq!(from_context, statements.len());

        // A predicate, a grouping or another aggregate scans.
        for sql in [
            "SELECT COUNT(*) FROM events WHERE id > 5",
            "SELECT MIN(id) FROM events GROUP BY name",
            "SELECT SUM(id) FROM events",
            "SELECT COUNT(DISTINCT id) FROM events",
            "SELECT COUNT(*) FROM events e JOIN orders o ON e.id = o.id",
        ] {
            let (_, record) = submit_and_record(&state, sql).await;
            assert_ne!(record["execution"]["mode"], "context", "{sql}");
        }

        // The source moves on: the statistics on record no longer describe
        // the pinned version, so the count scans — and comes back right.
        write_events_file(&directory.join("events").join("d.parquet"), 300, 50);
        let (body, record) = submit_and_record(&state, "SELECT COUNT(*) FROM events").await;
        assert_eq!(body["data"], serde_json::json!([[350]]));
        assert_ne!(record["execution"]["mode"], "context");

        // Planning saw the new version and refreshed the statistics in the
        // background: the added file folded in, the rest not re-read.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
        let refreshed = loop {
            let current = stored_events_statistics(&state).unwrap();
            if current.source_version != stored.source_version {
                break current;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "statistics were not refreshed"
            );
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        };
        assert_eq!(refreshed.rows, 350);
        assert_eq!(refreshed.files, 4);
        assert_eq!(refreshed.depth, kaveon_core::StatisticsDepth::Metadata);
        assert_eq!(
            refreshed.column("id").unwrap().max,
            Some(kaveon_core::StatValue::Int(349))
        );
        let (body, record) =
            submit_and_record(&state, "SELECT COUNT(*), MAX(id) FROM events").await;
        assert_eq!(body["data"], serde_json::json!([[350, 349]]));
        assert_eq!(record["execution"]["mode"], "context");
        assert_eq!(
            record["execution"]["source_version"]["files"], 4,
            "{}",
            record["execution"]
        );
        std::fs::remove_dir_all(directory).unwrap();
    }

    /// `WITH (sketches = true)` reads the columns once for the sketches;
    /// a later metadata-only ANALYZE at the same version keeps them, a
    /// new version drops them.
    #[tokio::test]
    async fn analyze_with_sketches_reads_the_columns_once_and_keeps_them_at_the_same_version() {
        let (state, directory) = analyze_test_state().await;
        register_events_directory(&state, &directory, 100).await;
        let (status, body) = submit(
            &state,
            &admin(),
            "ANALYZE events WITH (sketches = true)",
            serde_json::Value::Null,
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(
            body["data"],
            serde_json::json!([["lake.sales.events", 300, 0]])
        );
        let full = stored_events_statistics(&state).unwrap();
        assert_eq!(full.depth, kaveon_core::StatisticsDepth::Full);
        let id = full.column("id").unwrap();
        assert!(id.distinct.is_some() && id.quantiles.is_some());
        let close = |estimate: Option<u64>, exact: u64| {
            let estimate = estimate.expect("an estimate");
            assert!(
                estimate.abs_diff(exact) * 50 <= exact,
                "{estimate} is not within 2 % of {exact}"
            );
        };
        close(id.distinct_count(), 300);
        assert!(id.bounds_exact);
        let name = full.column("name").unwrap();
        assert!(name.distinct.is_some() && name.quantiles.is_none());
        close(name.distinct_count(), 97);
        assert_eq!(name.null_count, Some(60));
        assert!(name.bounds_exact);
        assert!(full.column("day").unwrap().quantiles.is_some());
        // SHOW STATS FOR presents the estimates.
        let (body, _) = submit_and_record(&state, "SHOW STATS FOR events").await;
        assert_eq!(body["data"][0][4], serde_json::json!(id.distinct_count()));
        assert_eq!(body["data"][2][4], serde_json::json!(name.distinct_count()));
        // A metadata-only ANALYZE at the same version keeps the sketches
        // and adds its exact count.
        let (status, body) = submit(
            &state,
            &admin(),
            "ANALYZE events WITH (columns = ARRAY['name'])",
            serde_json::Value::Null,
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        let again = stored_events_statistics(&state).unwrap();
        assert_eq!(again.depth, kaveon_core::StatisticsDepth::Full);
        assert_eq!(again.column("id").unwrap().distinct, id.distinct);
        assert_eq!(again.column("id").unwrap().quantiles, id.quantiles);
        assert_eq!(again.column("name").unwrap().distinct_exact, Some(97));
        assert_eq!(again.column("name").unwrap().distinct, name.distinct);
        // A new version: the sketches are gone until the next read (the
        // automatic refresh folds added files into a full record).
        write_events_file(&directory.join("events").join("d.parquet"), 300, 50);
        let (status, body) =
            submit(&state, &admin(), "ANALYZE events", serde_json::Value::Null).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        let moved = stored_events_statistics(&state).unwrap();
        assert_eq!(moved.depth, kaveon_core::StatisticsDepth::Metadata);
        assert_eq!(moved.rows, 350);
        assert!(moved.column("id").unwrap().distinct.is_none());
        assert!(moved.column("name").unwrap().distinct_exact.is_none());
        std::fs::remove_dir_all(directory).unwrap();
    }

    /// With the automatic refresh off, statistics behind the source stay
    /// as they are: they still cost a join, they never answer.
    #[tokio::test]
    async fn stale_statistics_cost_but_never_answer() {
        let schema = Arc::new(Schema::new(vec![Field::new("id", DataType::Int64, false)]));
        let batch = RecordBatch::try_new(
            schema.clone(),
            vec![Arc::new(Int64Array::from(vec![1, 2, 3]))],
        )
        .unwrap();
        let (state, directory) = analyze_test_state_configured(schema, batch, |config| {
            config.statistics_auto_refresh = false;
        })
        .await;
        register_events_directory(&state, &directory, 100).await;
        let (_, body) = submit(&state, &admin(), "ANALYZE events", serde_json::Value::Null).await;
        assert_eq!(body["data"][0][1], 300, "{body}");
        let stored = stored_events_statistics(&state).unwrap();
        let (_, record) = submit_and_record(&state, "SELECT COUNT(*) FROM events").await;
        assert_eq!(record["execution"]["mode"], "context");

        write_events_file(&directory.join("events").join("d.parquet"), 300, 50);
        for sql in [
            "SELECT COUNT(*) FROM events",
            "SELECT MAX(id) FROM events",
            "SELECT MIN(day), MAX(day) FROM events",
        ] {
            let (body, record) = submit_and_record(&state, sql).await;
            assert_ne!(record["execution"]["mode"], "context", "{sql}");
            if sql.starts_with("SELECT COUNT") {
                assert_eq!(body["data"], serde_json::json!([[350]]));
            }
        }
        // A join over the stale table still plans from its statistics.
        let (body, record) = submit_and_record(
            &state,
            "SELECT COUNT(*) FROM orders o JOIN events e ON o.id = e.id WHERE e.id = 2",
        )
        .await;
        assert_eq!(body["data"], serde_json::json!([[1]]));
        assert_ne!(record["execution"]["mode"], "context");
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        assert_eq!(stored_events_statistics(&state).unwrap(), stored);
        // The statistics endpoint says so.
        let response = super::get_table_statistics(
            axum::extract::State(state.clone()),
            axum::extract::Path(stored.table_id.as_str().to_owned()),
        )
        .await;
        let body = json_body(response).await;
        assert_eq!(body["stale"], true);
        assert_eq!(body["current_source_version"]["files"], 4);
        assert_eq!(body["source_version"]["files"], 3);
        std::fs::remove_dir_all(directory).unwrap();
    }

    /// A filtered scan of a directory table opens only the files whose
    /// recorded bounds admit the predicate, once the table's statistics
    /// are current; the skipped files are on the record.
    #[tokio::test]
    async fn current_statistics_skip_directory_files_by_their_bounds() {
        let (state, directory) = analyze_test_state().await;
        register_events_directory(&state, &directory, 100).await;
        let sql = "SELECT id FROM events WHERE id >= 250 ORDER BY id";
        let (body, record) = submit_and_record(&state, sql).await;
        assert_eq!(body["data"].as_array().unwrap().len(), 50);
        assert_eq!(body["data"][0], serde_json::json!([250]));
        let scan = &record["scans"][0];
        // Without statistics every file is opened; row groups are pruned
        // from the footers.
        assert_eq!(scan["files_considered"], 3, "{scan}");
        assert_eq!(scan["files_opened"], 3, "{scan}");
        assert_eq!(scan["files_skipped"], 0, "{scan}");

        let (_, body) = submit(&state, &admin(), "ANALYZE events", serde_json::Value::Null).await;
        assert_eq!(body["data"][0][1], 300, "{body}");
        let (body, record) = submit_and_record(&state, sql).await;
        assert_eq!(body["data"].as_array().unwrap().len(), 50);
        assert_eq!(body["data"][0], serde_json::json!([250]));
        assert_eq!(body["data"][49], serde_json::json!([299]));
        let scan = &record["scans"][0];
        assert_eq!(scan["files_considered"], 3, "{scan}");
        assert_eq!(scan["files_opened"], 1, "{scan}");
        assert_eq!(scan["files_skipped"], 2, "{scan}");

        // Bounds on a text column and on a nullable column skip too, and a
        // file the bounds admit is opened.
        let (body, record) =
            submit_and_record(&state, "SELECT COUNT(*) FROM events WHERE score < 10").await;
        assert_eq!(body["data"], serde_json::json!([[17]]));
        assert_eq!(record["scans"][0]["files_skipped"], 2);
        let (body, record) =
            submit_and_record(&state, "SELECT COUNT(*) FROM events WHERE score IS NULL").await;
        assert_eq!(body["data"], serde_json::json!([[43]]));
        assert_eq!(record["scans"][0]["files_skipped"], 0);
        let (body, record) = submit_and_record(
            &state,
            "SELECT COUNT(*) FROM events WHERE id BETWEEN 90 AND 110",
        )
        .await;
        assert_eq!(body["data"], serde_json::json!([[21]]));
        assert_eq!(record["scans"][0]["files_skipped"], 1);
        assert_eq!(record["scans"][0]["files_opened"], 2);

        // A file that lands after ANALYZE makes the statistics stale: no
        // skipping — nothing is judged from bounds of another version —
        // and the new rows are read.
        write_events_file(&directory.join("events").join("d.parquet"), 300, 50);
        let (body, record) = submit_and_record(&state, sql).await;
        assert_eq!(body["data"].as_array().unwrap().len(), 100);
        let scan = &record["scans"][0];
        assert_eq!(scan["files_considered"], 4, "{scan}");
        assert_eq!(scan["files_skipped"], 0, "{scan}");
        std::fs::remove_dir_all(directory).unwrap();
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
        let (state, directory) = analyze_test_state().await;
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
    async fn a_dropped_statement_future_marks_its_running_record_canceled() {
        let state = std::sync::Arc::new(catalog_test_state());
        let query_id = format!("abandoned-{}", uuid::Uuid::new_v4());
        let context = analyze_context();
        super::QUERY_STORE.write().await.queries.insert(
            query_id.clone(),
            super::pending_query_record(
                &query_id,
                "SELECT 1",
                &crate::settings::QuerySettings::default(),
                super::unix_time_ms() - 5,
                &context,
                super::QueryState::Running,
                0,
            ),
        );
        let _ = state.lifecycle.cancellations.token(&query_id).unwrap();
        drop(super::StatementLifecycleGuard {
            state: state.clone(),
            query_id: query_id.clone(),
        });
        // The record is marked on the runtime, not inside Drop.
        for _ in 0..50 {
            tokio::task::yield_now().await;
            if let Some(record) = super::QUERY_STORE.read().await.queries.get(&query_id)
                && matches!(record.state, super::QueryState::Canceled)
            {
                break;
            }
        }
        let store = super::QUERY_STORE.read().await;
        let record = store.queries.get(&query_id).expect("the record stays");
        assert!(matches!(record.state, super::QueryState::Canceled));
        assert_eq!(
            record.error.as_deref(),
            Some("client disconnected before the statement finished")
        );
        assert!(record.completed_at_ms > 0 && record.elapsed_ms >= 5);
        drop(store);
        // A finished record is left alone.
        let finished = format!("finished-{}", uuid::Uuid::new_v4());
        let mut record = super::pending_query_record(
            &finished,
            "SELECT 1",
            &crate::settings::QuerySettings::default(),
            super::unix_time_ms(),
            &context,
            super::QueryState::Running,
            0,
        );
        record.state = super::QueryState::Finished;
        super::QUERY_STORE
            .write()
            .await
            .queries
            .insert(finished.clone(), record);
        let _ = state.lifecycle.cancellations.token(&finished).unwrap();
        drop(super::StatementLifecycleGuard {
            state,
            query_id: finished.clone(),
        });
        for _ in 0..20 {
            tokio::task::yield_now().await;
        }
        assert!(matches!(
            super::QUERY_STORE.read().await.queries[&finished].state,
            super::QueryState::Finished
        ));
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
            stream_result: false,
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

/// The streamed root path: a worker streams a root task's rows while the
/// task runs, the coordinator pages them as they arrive, and a failure
/// after delivery is final.
#[cfg(test)]
mod streamed_root_tests {
    use super::*;
    use arrow::array::Int64Array;
    use arrow::datatypes::{DataType, Field, Schema};
    use arrow::record_batch::RecordBatch;
    use kaveon_core::{
        AccessPattern, CatalogManager, CatalogProvider, DataFormat, MemoryCatalog, StorageType,
        TableMeta,
    };

    const TOKEN: &str = "exchange-token-at-least-32-bytes-long";
    const SNAPSHOT: &str = "sha256:streamed-root-tests";
    const ROW_GROUPS: i64 = 2;
    const ROWS_PER_GROUP: i64 = 20_000;
    /// The reader's batch size: a row group of `ROWS_PER_GROUP` rows comes
    /// out as three batches, the first this long.
    const FIRST_BATCH_ROWS: usize = 8_192;

    /// A catalog over one Parquet table of `ROW_GROUPS` row groups of
    /// `ROWS_PER_GROUP` rows each: one row group per task of a two-task
    /// scan, several batches per task.
    fn table(directory: &std::path::Path) -> CatalogManager {
        let schema = Arc::new(Schema::new(vec![Field::new("v", DataType::Int64, false)]));
        let path = directory.join("numbers.parquet");
        if !path.exists() {
            let properties = parquet::file::properties::WriterProperties::builder()
                .set_max_row_group_size(ROWS_PER_GROUP as usize)
                .build();
            let mut writer = parquet::arrow::ArrowWriter::try_new(
                std::fs::File::create(&path).unwrap(),
                Arc::clone(&schema),
                Some(properties),
            )
            .unwrap();
            for group in 0..ROW_GROUPS {
                let values = (0..ROWS_PER_GROUP)
                    .map(|row| group * ROWS_PER_GROUP + row)
                    .collect::<Vec<_>>();
                let batch = RecordBatch::try_new(
                    Arc::clone(&schema),
                    vec![Arc::new(Int64Array::from(values))],
                )
                .unwrap();
                writer.write(&batch).unwrap();
                writer.flush().unwrap();
            }
            writer.close().unwrap();
        }
        let mut catalog = MemoryCatalog::new(
            "lake",
            StorageType::Local {
                base_path: directory.to_path_buf(),
            },
        )
        .with_schema("data");
        catalog
            .register_table(
                "data",
                TableMeta {
                    name: "numbers".into(),
                    arrow_schema: schema,
                    location: "numbers.parquet".into(),
                    access: AccessPattern::Shortcut,
                    format: DataFormat::Parquet,
                },
            )
            .unwrap();
        let mut manager = CatalogManager::new("lake", "data");
        manager.register_catalog(Box::new(catalog));
        manager
    }

    fn plan(statement: &str, manager: &CatalogManager) -> LogicalPlan {
        let mut plan = sql_to_logical_plan_for_binder(statement).unwrap();
        crate::planner::qualify_tables(&mut plan, "lake", "data");
        kaveon_optim::binder::bind(plan, manager).unwrap()
    }

    fn state_over(manager: CatalogManager, coordinator: bool, node_id: &str) -> Arc<AppState> {
        let mut state = catalog_test_state();
        state.config.coordinator = coordinator;
        state.config.node_id = node_id.to_owned();
        *state.catalog.get_mut() = Arc::new(crate::PublishedCatalog {
            manager,
            snapshot_id: SNAPSHOT.into(),
        });
        Arc::new(state)
    }

    /// A worker over the table, served on a loopback port.
    async fn spawn_worker(
        manager: CatalogManager,
        node_id: &str,
    ) -> (NodeInfo, tokio::task::JoinHandle<()>) {
        let state = state_over(manager, false, node_id);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let app = build_router(Arc::clone(&state));
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let mut info = state.cluster.read().await.this_node.clone();
        info.node_id = node_id.to_owned();
        info.address = format!("http://{address}");
        info.role = NodeRole::Worker;
        info.catalog_snapshot_id = Some(SNAPSHOT.into());
        info.last_heartbeat = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();
        (info, server)
    }

    fn context() -> QueryContext {
        QueryContext {
            engine_version: "test".into(),
            environment: "test".into(),
            principal: None,
            user: None,
            source: None,
            client: None,
            catalog: "lake".into(),
            schema: "data".into(),
            time_zone: None,
            client_address: None,
            client_tags: Vec::new(),
            result_delivery: Some("paged".into()),
            catalog_snapshot_id: SNAPSHOT.into(),
            settings: QuerySettings::default(),
        }
    }

    /// A gate a probe waits at, from the executing thread, until the test
    /// has seen what it needs.
    struct Gate(std::sync::Mutex<bool>, std::sync::Condvar);
    impl Gate {
        fn new() -> Arc<Self> {
            Arc::new(Self(
                std::sync::Mutex::new(false),
                std::sync::Condvar::new(),
            ))
        }
        fn wait(&self) {
            let mut released = self.0.lock().unwrap();
            while !*released {
                released = self.1.wait(released).unwrap();
            }
        }
        fn release(&self) {
            *self.0.lock().unwrap() = true;
            self.1.notify_all();
        }
    }

    /// A probe that holds every task of `query_id` after its first batch.
    fn hold_after_first_batch(query_id: &str) -> Arc<Gate> {
        let gate = Gate::new();
        let probe: RootStreamProbe = Arc::new({
            let gate = Arc::clone(&gate);
            move |index| {
                if index == 0 {
                    gate.wait();
                }
                Ok(())
            }
        });
        ROOT_STREAM_PROBES
            .lock()
            .unwrap()
            .insert(query_id.to_owned(), probe);
        gate
    }

    fn task_request(query_id: &str, fragment: ExecutableFragment, count: usize) -> TaskRequest {
        TaskRequest {
            query_id: query_id.into(),
            stage_id: fragment.stage_id.0,
            attempt: 0,
            query: String::new(),
            catalog: "lake".into(),
            schema: "data".into(),
            catalog_snapshot_id: None,
            partition_index: 0,
            partition_count: count,
            fragment: Some(fragment),
            execution_partition: Some(ExecutionPartitionRequest { index: 0, count }),
            exchange_inputs: vec![],
            exchange_outputs: vec![],
            settings: QuerySettings::default(),
            stream_result: true,
        }
    }

    fn root_fragment(
        query_id: &str,
        manager: &CatalogManager,
        workers: usize,
    ) -> ExecutableFragment {
        let plan = plan("SELECT v FROM numbers", manager);
        let graph = crate::planner::build_stage_graph(query_id, &plan, workers).unwrap();
        let fragments =
            crate::planner::build_executable_fragments(query_id, &plan, manager, workers).unwrap();
        fragments[&graph.root_stage].clone()
    }

    async fn metrics_status(
        client: &reqwest::Client,
        worker: &NodeInfo,
        request: &TaskRequest,
    ) -> (StatusCode, serde_json::Value) {
        let response = client
            .get(format!(
                "{}/v1/task/{}/{}/{}/{}/metrics",
                worker.address,
                request.query_id,
                request.stage_id,
                request.partition_index,
                request.attempt
            ))
            .bearer_auth(TOKEN)
            .send()
            .await
            .unwrap();
        let status = response.status();
        (status, response.json().await.unwrap_or_default())
    }

    #[tokio::test]
    async fn a_worker_streams_a_root_tasks_batches_before_the_fragment_finishes() {
        let directory =
            std::env::temp_dir().join(format!("kaveon-streamed-root-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&directory).unwrap();
        let manager = table(&directory);
        let fragment = root_fragment("q-stream", &manager, 1);
        let (worker, server) = spawn_worker(manager, "worker-stream").await;
        let client = reqwest::Client::new();
        let request = task_request("q-stream", fragment.clone(), 1);
        assert!(is_root_fragment(&request, &fragment));

        let gate = hold_after_first_batch("q-stream");
        let response = client
            .post(format!("{}/v1/task", worker.address))
            .bearer_auth(TOKEN)
            .json(&request)
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert!(response.headers().contains_key(TASK_STREAMED_HEADER));
        assert!(!response.headers().contains_key("x-kaveon-task-elapsed-us"));
        let mut stream = crate::transport::receive_stream(response).await.unwrap();
        assert_eq!(stream.schema().fields().len(), 1);
        // The first batch arrives while the task is held: still running.
        let first = stream.next_batch().await.unwrap().unwrap();
        assert_eq!(first.num_rows(), FIRST_BATCH_ROWS);
        let (status, body) = metrics_status(&client, &worker, &request).await;
        assert_eq!(status, StatusCode::ACCEPTED, "{body}");
        gate.release();
        let mut rows = first.num_rows();
        let mut batches = 1;
        while let Some(batch) = stream.next_batch().await {
            rows += batch.unwrap().num_rows();
            batches += 1;
        }
        assert_eq!(rows, (ROW_GROUPS * ROWS_PER_GROUP) as usize);
        assert!(batches >= 2);
        // The body ended after the outcome was recorded: the metrics are
        // there at once.
        let (status, body) = metrics_status(&client, &worker, &request).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert!(body["elapsed_us"].is_u64());
        assert!(body["scan"].is_object(), "{body}");
        assert!(body["execution"].is_object(), "{body}");
        let scan: TaskScanMetrics = serde_json::from_value(body["scan"].clone()).unwrap();
        assert_eq!(scan.rows_emitted, (ROW_GROUPS * ROWS_PER_GROUP) as u64);
        // A second submission of a streamed task is refused; the metrics stay.
        let duplicate = client
            .post(format!("{}/v1/task", worker.address))
            .bearer_auth(TOKEN)
            .json(&request)
            .send()
            .await
            .unwrap();
        assert_eq!(duplicate.status(), StatusCode::CONFLICT);
        let body: serde_json::Value = duplicate.json().await.unwrap();
        assert_eq!(body["code"], "TASK_RESULT_NOT_RETAINED");
        // Unknown and unauthenticated lookups.
        let mut unknown = task_request("q-stream", fragment.clone(), 1);
        unknown.attempt = 7;
        let (status, _) = metrics_status(&client, &worker, &unknown).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        let unauthorized = client
            .get(format!(
                "{}/v1/task/q-stream/{}/0/0/metrics",
                worker.address, request.stage_id
            ))
            .send()
            .await
            .unwrap();
        assert_eq!(unauthorized.status(), StatusCode::UNAUTHORIZED);

        // Without `stream_result` the task answers the collected way, with
        // its metrics in the headers.
        let mut collected = task_request("q-collected", fragment, 1);
        collected.stream_result = false;
        let response = client
            .post(format!("{}/v1/task", worker.address))
            .bearer_auth(TOKEN)
            .json(&collected)
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert!(!response.headers().contains_key(TASK_STREAMED_HEADER));
        assert!(response.headers().contains_key("x-kaveon-task-elapsed-us"));
        let (_, batches) = crate::transport::receive(response)
            .await
            .unwrap()
            .collect()
            .unwrap();
        assert_eq!(
            batches.iter().map(|batch| batch.num_rows()).sum::<usize>(),
            (ROW_GROUPS * ROWS_PER_GROUP) as usize
        );
        server.abort();
        let _ = std::fs::remove_dir_all(&directory);
    }

    /// A coordinator over two workers, and the paged writer for `query_id`.
    async fn cluster(
        directory: &std::path::Path,
        query_id: &str,
    ) -> (
        Arc<AppState>,
        crate::results::ResultWriter,
        Vec<tokio::task::JoinHandle<()>>,
    ) {
        let (first, first_server) = spawn_worker(table(directory), "worker-a").await;
        let (second, second_server) = spawn_worker(table(directory), "worker-b").await;
        let state = state_over(table(directory), true, "coordinator");
        {
            let mut cluster = state.cluster.write().await;
            cluster.register_worker(first);
            cluster.register_worker(second);
        }
        let writer = state.results.begin(query_id, "alice").unwrap();
        (state, writer, vec![first_server, second_server])
    }

    fn alice() -> Identity {
        Identity {
            principal: "alice".into(),
            display_identity: None,
            role: crate::security::Role::Analyst,
        }
    }

    #[allow(clippy::type_complexity)]
    async fn run_statement(
        state: &Arc<AppState>,
        query_id: &str,
        writer: crate::results::ResultWriter,
    ) -> Option<Result<(TaskResponse, Vec<StageTelemetry>, u64), String>> {
        let context = context();
        let snapshot = Arc::clone(&*state.catalog.read().await);
        let plan = plan("SELECT v FROM numbers", &snapshot);
        let mut reason = None;
        let mut writer = Some(writer);
        execute_distributed_fragments(
            state,
            query_id,
            &context,
            &plan,
            &snapshot,
            &SourcePins::default(),
            DistributedSink {
                placement_reason: &mut reason,
                result_writer: &mut writer,
            },
        )
        .await
    }

    #[tokio::test]
    async fn the_coordinator_pages_root_rows_before_the_root_task_ends() {
        let directory =
            std::env::temp_dir().join(format!("kaveon-streamed-pages-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&directory).unwrap();
        let (state, writer, servers) = cluster(&directory, "q-pages").await;
        // Both root tasks hold after their first batch: whatever page 0
        // shows arrived while they ran.
        let gate = hold_after_first_batch("q-pages");
        let run = run_statement(&state, "q-pages", writer);
        let watch = async {
            let deadline = Instant::now() + Duration::from_secs(60);
            let page = loop {
                match state.results.page("q-pages", 0, &alice()) {
                    Ok(crate::results::ResultPage::Ready(page)) => break page,
                    Ok(crate::results::ResultPage::Pending(_)) => {}
                    Err(status) => panic!("page 0 answered {status}"),
                }
                assert!(Instant::now() < deadline, "page 0 never landed");
                tokio::time::sleep(Duration::from_millis(10)).await;
            };
            gate.release();
            page
        };
        let (page, outcome) = tokio::join!(watch, run);
        assert_eq!(page["complete"], serde_json::Value::Bool(false));
        assert_eq!(page["data"].as_array().unwrap().len(), 1_000);
        let (response, stages, _) = outcome.expect("distributed").expect("statement succeeds");
        assert_eq!(response.columns[0].name, "v");
        assert!(response.data.is_empty());
        let root = stages.last().unwrap();
        assert_eq!(root.tasks.len(), 2);
        assert_eq!(
            root.tasks
                .iter()
                .map(|task| task.output_rows)
                .sum::<usize>(),
            (ROW_GROUPS * ROWS_PER_GROUP) as usize
        );
        for task in &root.tasks {
            assert!(task.output_batches >= 2, "{}", task.output_batches);
            assert!(task.output_bytes > 0);
            assert!(task.scan.is_some(), "scan metrics from /metrics");
            assert!(task.execution.is_some(), "execution metrics from /metrics");
        }
        let (_, complete) = distributed_scan_telemetry(&stages);
        assert!(complete);
        // The result is complete, with every row.
        let crate::results::ResultPage::Ready(first) =
            state.results.page("q-pages", 0, &alice()).unwrap()
        else {
            panic!("page 0 is ready");
        };
        assert_eq!(first["complete"], serde_json::Value::Bool(true));
        assert_eq!(
            first["row_count"],
            serde_json::Value::from(ROW_GROUPS * ROWS_PER_GROUP)
        );
        for server in servers {
            server.abort();
        }
        let _ = std::fs::remove_dir_all(&directory);
    }

    #[tokio::test]
    async fn a_root_task_that_fails_after_delivering_rows_fails_the_statement_for_good() {
        let directory =
            std::env::temp_dir().join(format!("kaveon-streamed-fail-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&directory).unwrap();
        let (state, writer, servers) = cluster(&directory, "q-fail").await;
        let probe: RootStreamProbe = Arc::new(|index| {
            if index == 1 {
                Err("injected failure after the first batch".into())
            } else {
                Ok(())
            }
        });
        ROOT_STREAM_PROBES
            .lock()
            .unwrap()
            .insert("q-fail".to_owned(), probe);
        let error = match run_statement(&state, "q-fail", writer)
            .await
            .expect("distributed")
        {
            Ok(_) => panic!("the statement fails"),
            Err(error) => error,
        };
        assert!(
            error.contains("injected failure after the first batch"),
            "{error}"
        );
        assert!(error.ends_with(ROWS_DELIVERED_NO_RETRY), "{error}");
        // The pages are gone: a client following them sees 410, not 404.
        assert_eq!(
            state.results.page("q-fail", 0, &alice()).unwrap_err(),
            StatusCode::GONE
        );
        assert!(!state.results.contains("q-fail"));
        for server in servers {
            server.abort();
        }
        let _ = std::fs::remove_dir_all(&directory);
    }
}
