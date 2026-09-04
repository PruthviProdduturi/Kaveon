use crate::AppState;
use crate::cluster::{NodeInfo, NodeRole};
use crate::lifecycle::{CancellationToken, TaskClaim, TaskOutcome, TaskOwner};
use axum::body::Body;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, post};
use axum::{Json, Router};
use kaveon_core::collect_batches;
use kaveon_core::{ExchangeId, ExecutableFragment, StageId, TaskId};
use kaveon_exec::sort::SortExpr;
use kaveon_exec::topn::merge_top_n;
use kaveon_sql::logical_plan::sql_to_logical_plan;
use kaveon_sql::logical_plan::{AggregateExpr, LogicalPlan};
use serde::{Deserialize, Serialize};
use std::cmp::Reverse;
use std::collections::{BTreeMap, HashMap};
use std::io::Cursor;
use std::sync::Arc;
use std::time::Duration;
use std::time::{Instant, SystemTime, UNIX_EPOCH};
use tower_http::cors::CorsLayer;
use uuid::Uuid;

use tokio::sync::RwLock;

use crate::orchestrator::{CoordinatorOrchestrator, TaskDispatch};

struct QueryStore {
    queries: HashMap<String, QueryRecord>,
}

#[derive(Clone, Serialize)]
struct QueryRecord {
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
        .route("/v1/task", post(execute_task))
        .route(
            "/v1/internal/query/{query_id}/finish",
            post(finish_worker_query),
        )
        .route("/v1/query", get(list_queries))
        .route("/v1/query/{query_id}", get(get_query))
        .route("/v1/query/{query_id}", delete(cancel_query))
        .route("/v1/cluster", get(get_cluster))
        .route("/v1/node", get(get_node))
        .route("/v1/node/heartbeat", post(receive_heartbeat))
        .route("/v1/catalog", get(list_catalogs))
        .route("/v1/catalog/{catalog}/schema", get(list_schemas))
        .route(
            "/v1/catalog/{catalog}/schema/{schema}/table",
            get(list_tables),
        )
        .route("/ui", get(crate::ui::dashboard))
        .route("/health", get(health))
        .route("/ready", get(ready))
        .layer(CorsLayer::permissive())
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
    user: Option<String>,
    #[serde(default)]
    source: Option<String>,
    #[serde(default)]
    client: Option<String>,
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
    owner: TaskOwner<(Vec<u8>, u64)>,
    cancellation: CancellationToken,
) -> Response {
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
    let started = Instant::now();
    if let Some(fragment) = req.fragment.as_ref() {
        let result = execute_fragment_task(state, &req, fragment, partition).await;
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
    let result = {
        let catalog = state.catalog.read().await;
        crate::planner::plan_partitioned_query(&plan, &catalog, partition).and_then(
            |mut planned| {
                let schema = planned.operator.schema().clone();
                collect_batches(&mut *planned.operator).map(|batches| (schema, batches))
            },
        )
    };
    if cancellation.is_cancelled() {
        let _ = owner.complete(TaskOutcome::Failed(Arc::from("query canceled")));
        return canceled_task_response();
    }
    match result {
        Ok((schema, batches)) => match encode_arrow_stream(&schema, &batches) {
            Ok(bytes) => {
                let elapsed = elapsed_us(started);
                let outcome = TaskOutcome::Success(Arc::new((bytes, elapsed)));
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

struct PrefetchedExchangeInputs {
    inputs: HashMap<ExchangeId, crate::fragment_exec::ExchangeBatches>,
}

impl crate::fragment_exec::ExchangeInputProvider for PrefetchedExchangeInputs {
    fn read(
        &self,
        exchange_id: &ExchangeId,
    ) -> kaveon_core::Result<crate::fragment_exec::ExchangeBatches> {
        let input = self.inputs.get(exchange_id).ok_or_else(|| {
            kaveon_core::KaveonError::Execution(format!(
                "exchange input '{}' was not supplied to the fragment task",
                exchange_id.0
            ))
        })?;
        Ok(crate::fragment_exec::ExchangeBatches {
            schema: input.schema.clone(),
            batches: input.batches.clone(),
        })
    }
}

async fn execute_fragment_task(
    state: &Arc<AppState>,
    req: &TaskRequest,
    fragment: &ExecutableFragment,
    partition: kaveon_storage::ScanPartition,
) -> Result<
    (
        arrow::datatypes::SchemaRef,
        Vec<arrow::record_batch::RecordBatch>,
    ),
    String,
> {
    let client = reqwest::Client::new();
    let token = state
        .config
        .exchange_token
        .as_deref()
        .ok_or_else(|| "fragment execution requires an exchange bearer token".to_owned())?;
    let mut inputs = HashMap::<ExchangeId, crate::fragment_exec::ExchangeBatches>::new();
    for location in &req.exchange_inputs {
        let identity = crate::exchange::ExchangeIdentity {
            exchange_id: location.exchange_id.clone(),
            task_id: location.producer.clone(),
            output_partition: location.output_partition,
        };
        let (schema, mut batches) =
            crate::exchange::fetch_batches(&client, &location.worker_uri, token, &identity)
                .await
                .map_err(|error| {
                    format!(
                        "cannot fetch exchange '{}': {error}",
                        location.exchange_id.0
                    )
                })?;
        match inputs.get_mut(&location.exchange_id) {
            Some(input) if input.schema != schema => {
                return Err(format!(
                    "exchange '{}' producers returned incompatible schemas",
                    location.exchange_id.0
                ));
            }
            Some(input) => input.batches.append(&mut batches),
            None => {
                inputs.insert(
                    location.exchange_id.clone(),
                    crate::fragment_exec::ExchangeBatches { schema, batches },
                );
            }
        }
    }
    let execution = {
        let catalog = state.catalog.read().await;
        crate::fragment_exec::execute_fragment(
            fragment,
            &catalog,
            &PrefetchedExchangeInputs { inputs },
            partition,
        )
        .map_err(|error| error.to_string())?
    };
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
                let chunks = crate::exchange::encode_batches(
                    identity,
                    &output.schema,
                    batches,
                    crate::exchange::ExchangeLimits::default(),
                )
                .map_err(|error| format!("cannot encode exchange '{}': {error}", exchange_id.0))?;
                crate::exchange::upload_chunks(
                    &client,
                    &destination.worker_uri,
                    token,
                    &chunks,
                    crate::exchange::ExchangeLimits::default(),
                )
                .await
                .map_err(|error| format!("cannot upload exchange '{}': {error}", exchange_id.0))?;
            }
            if destination_count == 0 {
                return Err(format!(
                    "exchange '{}' output partition {output_partition} has no destination",
                    exchange_id.0
                ));
            }
        }
    }
    Ok((execution.result_schema, execution.result_batches))
}

fn requested_partition_index(req: &TaskRequest) -> usize {
    req.execution_partition
        .map(|partition| partition.index)
        .unwrap_or(req.partition_index)
}

fn complete_owned_task(
    owner: TaskOwner<(Vec<u8>, u64)>,
    started: Instant,
    result: Result<
        (
            arrow::datatypes::SchemaRef,
            Vec<arrow::record_batch::RecordBatch>,
        ),
        String,
    >,
) -> Response {
    match result {
        Ok((schema, batches)) => match encode_arrow_stream(&schema, &batches) {
            Ok(bytes) => {
                let elapsed = elapsed_us(started);
                let outcome = TaskOutcome::Success(Arc::new((bytes, elapsed)));
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

fn task_outcome_response(outcome: TaskOutcome<(Vec<u8>, u64)>) -> Response {
    match outcome {
        TaskOutcome::Success(result) => Response::builder()
            .status(StatusCode::OK)
            .header(header::CONTENT_TYPE, "application/vnd.apache.arrow.stream")
            .header("x-kaveon-task-elapsed-us", result.1)
            .body(Body::from(result.0.clone()))
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

async fn submit_statement(
    State(state): State<Arc<AppState>>,
    Json(req): Json<StatementRequest>,
) -> impl IntoResponse {
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

    let query_id = Uuid::new_v4().to_string();
    let sql = req.query.trim().trim_end_matches(';').to_owned();
    let submitted_at_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64;
    let start = Instant::now();
    let context = {
        let catalog = state.catalog.read().await;
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
            principal: None,
            user: req.user.clone(),
            source: req.source,
            client: req.client,
            catalog: catalog_name.to_owned(),
            schema: schema_name.to_owned(),
            time_zone: req.time_zone,
            client_address: None,
            client_tags: req.client_tags,
            result_delivery: req.result_delivery,
        }
    };
    if let Err(error) = state.lifecycle.cancellations.token(&query_id) {
        return lifecycle_error_response(error.to_string());
    }

    QUERY_STORE.write().await.queries.insert(
        query_id.clone(),
        QueryRecord {
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
    let optimized_plan = crate::planner::optimized_plan_tree(&plan);
    let physical_plan = crate::planner::physical_plan_tree(&plan);
    if let Some(record) = QUERY_STORE.write().await.queries.get_mut(&query_id) {
        record.timings.analysis_us = Some(analysis_us);
        record.plan.logical = Some(logical_plan.clone());
        record.plan.optimized = Some(optimized_plan.clone());
        record.plan.physical = Some(physical_plan.clone());
    }

    if let Some(distributed) =
        execute_distributed_fragments(&state, &query_id, &context, &plan).await
    {
        match distributed {
            Ok((result, stages, planning_us)) => {
                let elapsed = start.elapsed().as_millis() as u64;
                let record = QueryRecord {
                    id: query_id.clone(),
                    sql,
                    state: QueryState::Finished,
                    columns: result.columns.clone(),
                    rows: result.data.clone(),
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
                    scans: vec![],
                    stages,
                    context,
                };
                QUERY_STORE
                    .write()
                    .await
                    .queries
                    .insert(query_id.clone(), record);
                cleanup_distributed_query(&state, &query_id).await;
                return Json(StatementResponse {
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
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(serde_json::json!({ "error": error, "code": "DISTRIBUTED_EXECUTION_ERROR" })),
                )
                    .into_response();
            }
        }
    }

    if let Some(distributed) =
        execute_distributed_aggregate(&state, &query_id, &sql, &context, &plan).await
    {
        match distributed {
            Ok((result, stage)) => {
                let elapsed = start.elapsed().as_millis() as u64;
                let record = QueryRecord {
                    id: query_id.clone(),
                    sql,
                    state: QueryState::Finished,
                    columns: result.columns.clone(),
                    rows: result.data.clone(),
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
                    scans: vec![],
                    stages: vec![stage],
                    context,
                };
                QUERY_STORE
                    .write()
                    .await
                    .queries
                    .insert(query_id.clone(), record);
                cleanup_distributed_query(&state, &query_id).await;
                return Json(StatementResponse {
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
                let elapsed = start.elapsed().as_millis() as u64;
                let record = QueryRecord {
                    id: query_id.clone(),
                    sql,
                    state: QueryState::Finished,
                    columns: result.columns.clone(),
                    rows: result.data.clone(),
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
                    scans: vec![],
                    stages: vec![stage],
                    context,
                };
                QUERY_STORE
                    .write()
                    .await
                    .queries
                    .insert(query_id.clone(), record);
                cleanup_distributed_query(&state, &query_id).await;
                return Json(StatementResponse {
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
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(serde_json::json!({ "error": error, "code": "DISTRIBUTED_EXECUTION_ERROR" })),
                )
                    .into_response();
            }
        }
    }

    let planned_execution = {
        let catalog = state.catalog.read().await;
        let planning_start = Instant::now();
        crate::planner::plan_query(&plan, &catalog).map(|planned| {
            let planning_us = elapsed_us(planning_start);
            let scan_handles = planned.scan_metrics;
            let mut operator = planned.operator;
            let execution_start = Instant::now();
            let result = collect_batches(&mut *operator);
            let execution_us = elapsed_us(execution_start);
            let scans = scan_handles.iter().map(scan_telemetry).collect::<Vec<_>>();
            (planning_us, execution_us, scans, result)
        })
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
            QUERY_STORE
                .write()
                .await
                .queries
                .insert(query_id.clone(), record);
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

    let columns: Vec<ColumnInfo> = if let Some(first) = batches.first() {
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
    let result_serialization_us = elapsed_us(serialization_start);
    let elapsed = start.elapsed().as_millis() as u64;

    let record = QueryRecord {
        id: query_id.clone(),
        sql,
        state: QueryState::Finished,
        columns: columns.clone(),
        rows: rows.clone(),
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
    QUERY_STORE
        .write()
        .await
        .queries
        .insert(query_id.clone(), record);

    let resp = StatementResponse {
        id: query_id,
        state: QueryState::Finished,
        columns: Some(columns),
        data: Some(rows),
        error: None,
        elapsed_ms: elapsed,
    };

    Json(resp).into_response()
}

async fn list_queries() -> Json<Vec<QueryRecord>> {
    let store = QUERY_STORE.read().await;
    let mut queries: Vec<QueryRecord> = store.queries.values().cloned().collect();
    queries.sort_unstable_by_key(|query| Reverse(query.submitted_at_ms));
    queries.truncate(QUERY_HISTORY_LIMIT);
    Json(queries)
}

async fn get_query(Path(query_id): Path<String>) -> impl IntoResponse {
    let store = QUERY_STORE.read().await;
    match store.queries.get(&query_id) {
        Some(record) => Json(record.clone()).into_response(),
        None => (
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
    if matches!(record.state, QueryState::Running) {
        record.state = QueryState::Canceled;
        record.error = Some("query canceled by client".into());
        record.completed_at_ms = unix_time_ms();
        let _ = state.lifecycle.cancellations.cancel(&query_id);
    }
    drop(store);

    let workers = {
        let mut cluster = state.cluster.write().await;
        cluster.remove_stale_workers();
        cluster.workers.values().cloned().collect::<Vec<_>>()
    };
    let client = reqwest::Client::new();
    for worker in workers {
        let url = format!(
            "{}/v1/query/{query_id}",
            worker.address.trim_end_matches('/')
        );
        let _ = client.delete(url).send().await;
    }
    StatusCode::NO_CONTENT.into_response()
}

// --- Cluster / Node ---

#[derive(Serialize)]
struct ClusterResponse {
    environment: String,
    coordinator: NodeInfo,
    workers: Vec<NodeInfo>,
    active_workers: usize,
    total_nodes: usize,
}

async fn get_cluster(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let mut cluster = state.cluster.write().await;
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
        coordinator,
        workers: workers.clone(),
        active_workers: workers.len(),
        total_nodes: nodes.len(),
    })
}

async fn get_node(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let mut cluster = state.cluster.write().await;
    cluster.update_uptime();
    Json(cluster.this_node.clone())
}

async fn receive_heartbeat(
    State(state): State<Arc<AppState>>,
    Json(info): Json<NodeInfo>,
) -> impl IntoResponse {
    if !state.config.coordinator {
        return StatusCode::BAD_REQUEST;
    }
    let mut cluster = state.cluster.write().await;
    cluster.register_worker(info);
    StatusCode::OK
}

// --- Catalog ---

async fn list_catalogs(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let catalog = state.catalog.read().await;
    let names = catalog.catalog_names();
    Json(serde_json::json!({ "catalogs": names }))
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
    if has_catalogs {
        (StatusCode::OK, Json(serde_json::json!({ "ready": true }))).into_response()
    } else {
        (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(serde_json::json!({ "ready": false, "reason": "no catalogs loaded" })),
        )
            .into_response()
    }
}

// --- Helpers ---

fn encode_arrow_stream(
    schema: &arrow::datatypes::SchemaRef,
    batches: &[arrow::record_batch::RecordBatch],
) -> Result<Vec<u8>, String> {
    let mut bytes = Vec::new();
    {
        let mut writer = arrow::ipc::writer::StreamWriter::try_new(&mut bytes, schema)
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
    Ok(bytes)
}

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
        let message = response.text().await.unwrap_or_default();
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
    let bytes = response.bytes().await.map_err(|error| RemoteTaskFailure {
        message: format!(
            "worker '{}' result could not be read: {error}",
            worker.node_id
        ),
        retryable: true,
    })?;
    let output_bytes = bytes.len();
    let (schema, batches) = decode_arrow_stream(&bytes).map_err(|error| RemoteTaskFailure {
        message: format!(
            "worker '{}' returned an invalid Arrow stream: {error}",
            worker.node_id
        ),
        retryable: false,
    })?;
    Ok((schema, batches, elapsed_us, output_bytes))
}

async fn cleanup_distributed_query(state: &Arc<AppState>, query_id: &str) {
    let workers = {
        let mut cluster = state.cluster.write().await;
        cluster.remove_stale_workers();
        cluster.workers.values().cloned().collect::<Vec<_>>()
    };
    if let Some(token) = state.config.exchange_token.as_deref() {
        let client = reqwest::Client::new();
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
    let _ = state.lifecycle.finish_query(query_id);
}

async fn execute_distributed_fragments(
    state: &Arc<AppState>,
    query_id: &str,
    context: &QueryContext,
    plan: &LogicalPlan,
) -> Option<Result<(TaskResponse, Vec<StageTelemetry>, u64), String>> {
    if !general_distributed_eligible(plan) {
        return None;
    }
    let mut workers = {
        let mut cluster = state.cluster.write().await;
        cluster.remove_stale_workers();
        cluster.workers.values().cloned().collect::<Vec<_>>()
    };
    workers.sort_unstable_by(|left, right| left.node_id.cmp(&right.node_id));
    let token = state.config.exchange_token.clone()?;
    if workers.len() < 2 || token.is_empty() {
        return None;
    }

    let planning_start = Instant::now();
    let (graph, fragments) = {
        let catalog = state.catalog.read().await;
        let graph = crate::planner::build_stage_graph(query_id, plan, workers.len()).ok()?;
        let fragments =
            crate::planner::build_executable_fragments(query_id, plan, &catalog, workers.len())
                .ok()?;
        (graph, fragments)
    };
    let planning_us = elapsed_us(planning_start);
    let mut orchestrator = match CoordinatorOrchestrator::new(graph, fragments, workers.clone()) {
        Ok(orchestrator) => orchestrator,
        Err(error) => return Some(Err(format!("cannot initialize stage execution: {error}"))),
    };
    let cancellation = match state.lifecycle.cancellations.token(query_id) {
        Ok(cancellation) => cancellation,
        Err(error) => return Some(Err(error.to_string())),
    };
    let client = reqwest::Client::new();
    let execution_start = Instant::now();
    let mut stage_started = BTreeMap::<StageId, Instant>::new();
    let mut stage_tasks = BTreeMap::<StageId, Vec<TaskTelemetry>>::new();
    let mut result_schema = None;
    let mut result_batches = Vec::new();

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
                let result = execute_remote_task(&client, &worker, &request, Some(&token)).await;
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
                Ok((schema, batches, elapsed_us, output_bytes)) => {
                    if dispatch.exchange_outputs.is_empty() {
                        if result_schema
                            .as_ref()
                            .is_some_and(|expected| expected != &schema)
                        {
                            orchestrator.cancel();
                            return Some(Err("root tasks returned incompatible schemas".into()));
                        }
                        result_schema.get_or_insert(schema);
                        result_batches.extend(batches.iter().cloned());
                    }
                    stage_tasks
                        .entry(task_id.stage_id)
                        .or_default()
                        .push(TaskTelemetry {
                            task_id: task_id.to_string(),
                            node_id: worker.node_id,
                            partition_index: task_id.partition,
                            elapsed_us,
                            output_rows: batches.iter().map(|batch| batch.num_rows()).sum(),
                            output_batches: batches.len(),
                            output_bytes,
                        });
                    if let Err(error) = orchestrator.finish_task(task_id) {
                        return Some(Err(format!("cannot finish distributed task: {error}")));
                    }
                }
                Err(failure) => {
                    release_dispatch_outputs(&client, &token, &dispatch).await;
                    if !failure.retryable {
                        orchestrator.cancel();
                        return Some(Err(failure.message));
                    }
                    match orchestrator.fail_task(task_id, &failure.message) {
                        Ok(true) => {}
                        Ok(false) => return Some(Err(failure.message)),
                        Err(error) => {
                            return Some(Err(format!(
                                "cannot record distributed task failure: {error}"
                            )));
                        }
                    }
                }
            }
        }
        release_completed_exchanges(&client, &token, &mut orchestrator).await;
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
    Some(Ok((
        TaskResponse {
            columns: columns_from_schema(&schema),
            data: batches_to_json(&result_batches),
            elapsed_us: execution_us,
        },
        stages,
        planning_us,
    )))
}

async fn release_dispatch_outputs(client: &reqwest::Client, token: &str, dispatch: &TaskDispatch) {
    for location in &dispatch.exchange_outputs {
        let identity = crate::exchange::ExchangeIdentity {
            exchange_id: location.exchange_id.clone(),
            task_id: location.producer.clone(),
            output_partition: location.output_partition,
        };
        let _ =
            crate::exchange::release_exchange(client, &location.worker_uri, token, &identity).await;
    }
}

fn general_distributed_eligible(plan: &LogicalPlan) -> bool {
    match plan {
        LogicalPlan::Aggregate { input, .. } => general_distributed_eligible(input),
        LogicalPlan::Project { input, .. }
        | LogicalPlan::Filter { input, .. }
        | LogicalPlan::Sort { input, .. }
        | LogicalPlan::Limit { input, .. } => general_distributed_eligible(input),
        LogicalPlan::Join { left, right, .. } => {
            general_distributed_eligible(left) && general_distributed_eligible(right)
        }
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

async fn release_completed_exchanges(
    client: &reqwest::Client,
    token: &str,
    orchestrator: &mut CoordinatorOrchestrator,
) {
    let Ok(cleanups) = orchestrator.drain_exchange_cleanup() else {
        return;
    };
    for cleanup in cleanups {
        for location in cleanup.locations {
            let identity = crate::exchange::ExchangeIdentity {
                exchange_id: cleanup.exchange_id.clone(),
                task_id: location.producer,
                output_partition: location.output_partition,
            };
            let _ =
                crate::exchange::release_exchange(client, &location.worker_uri, token, &identity)
                    .await;
        }
    }
}

async fn execute_distributed_top_n(
    state: &Arc<AppState>,
    query_id: &str,
    sql: &str,
    context: &QueryContext,
    plan: &LogicalPlan,
) -> Option<Result<(TaskResponse, StageTelemetry), String>> {
    let (sort_exprs, limit) = top_n_merge_contract(plan)?;
    let mut workers = {
        let mut cluster = state.cluster.write().await;
        cluster.remove_stale_workers();
        cluster.workers.values().cloned().collect::<Vec<_>>()
    };
    workers.sort_unstable_by(|left, right| left.node_id.cmp(&right.node_id));
    if workers.len() < 2 {
        return None;
    }

    let started = Instant::now();
    let partition_count = workers.len();
    let client = reqwest::Client::new();
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
                    Ok((schema, batches, elapsed_us, output_bytes)) => {
                        let output_rows = batches.iter().map(|batch| batch.num_rows()).sum();
                        let telemetry = TaskTelemetry {
                            task_id,
                            node_id: worker.node_id,
                            partition_index,
                            elapsed_us,
                            output_rows,
                            output_batches: batches.len(),
                            output_bytes,
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
) -> Option<Result<(TaskResponse, StageTelemetry), String>> {
    let (group_count, operations) = aggregate_merge_contract(plan)?;
    let mut workers = {
        let mut cluster = state.cluster.write().await;
        cluster.remove_stale_workers();
        cluster.workers.values().cloned().collect::<Vec<_>>()
    };
    workers.sort_unstable_by(|left, right| left.node_id.cmp(&right.node_id));
    if workers.len() < 2 {
        return None;
    }

    let started = Instant::now();
    let partition_count = workers.len();
    let client = reqwest::Client::new();
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
                    Ok((schema, batches, elapsed_us, output_bytes)) => {
                        let data = batches_to_json(&batches);
                        let telemetry = TaskTelemetry {
                            task_id,
                            node_id: worker.node_id,
                            partition_index,
                            elapsed_us,
                            output_rows: data.len(),
                            output_batches: batches.len(),
                            output_bytes,
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
    let merged = merge_partial_aggregates(partials, group_count, &operations, total_elapsed_us);
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
            | AggregateExpr::Sum(_) => Some(MergeOperation::Add),
            AggregateExpr::Min(_) => Some(MergeOperation::Min),
            AggregateExpr::Max(_) => Some(MergeOperation::Max),
            AggregateExpr::Avg(_) | AggregateExpr::Count { distinct: true, .. } => None,
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
        AggregateExpr::Sum(_) => "sum",
        AggregateExpr::Avg(_) => "avg",
        AggregateExpr::Min(_) => "min",
        AggregateExpr::Max(_) => "max",
    };
    if !name.eq_ignore_ascii_case(expected_name) || args.len() != 1 {
        return false;
    }
    let expected_expr = match aggregate {
        AggregateExpr::Count { expr, .. }
        | AggregateExpr::Sum(expr)
        | AggregateExpr::Avg(expr)
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
        | LogicalPlan::Limit { .. } => false,
    }
}

fn merge_partial_aggregates(
    partials: Vec<TaskResponse>,
    group_count: usize,
    operations: &[MergeOperation],
    elapsed_us: u64,
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
    use super::{
        ColumnInfo, MergeOperation, TaskRequest, TaskResponse, aggregate_merge_contract,
        decode_arrow_stream, encode_arrow_stream, general_distributed_eligible,
        merge_partial_aggregates, task_request_from_dispatch, top_n_merge_contract,
    };
    use arrow::array::{Int64Array, StringArray};
    use arrow::datatypes::{DataType, Field, Schema};
    use arrow::record_batch::RecordBatch;
    use std::sync::Arc;

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
        };

        let request = task_request_from_dispatch(&dispatch, &context);
        assert_eq!(request.query_id, task_id.query_id);
        assert_eq!(request.stage_id, 3);
        assert_eq!(request.attempt, 1);
        assert_eq!(request.partition_index, 2);
        assert_eq!(request.partition_count, 4);
        assert_eq!(request.execution_partition.unwrap().count, 4);
        assert!(request.fragment.is_some());
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
        let merged = merge_partial_aggregates(partials, 1, &[MergeOperation::Add], 2).unwrap();
        assert_eq!(
            merged.data,
            vec![
                vec![serde_json::json!("east"), serde_json::json!(5)],
                vec![serde_json::json!("west"), serde_json::json!(4)],
            ]
        );
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
