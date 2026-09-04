use crate::AppState;
use crate::cluster::{NodeInfo, NodeRole};
use axum::body::Body;
use axum::extract::{Path, State};
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, post};
use axum::{Json, Router};
use kaveon_core::collect_batches;
use kaveon_sql::logical_plan::sql_to_logical_plan;
use kaveon_sql::logical_plan::{AggregateExpr, LogicalPlan};
use serde::{Deserialize, Serialize};
use std::cmp::Reverse;
use std::collections::HashMap;
use std::io::Cursor;
use std::sync::Arc;
use std::time::{Instant, SystemTime, UNIX_EPOCH};
use tower_http::cors::CorsLayer;
use uuid::Uuid;

use tokio::sync::RwLock;

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
        .route("/v1/statement", post(submit_statement))
        .route("/v1/task", post(execute_task))
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
    query: String,
    catalog: String,
    schema: String,
    partition_index: usize,
    partition_count: usize,
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
    let partition =
        match kaveon_storage::ScanPartition::new(req.partition_index, req.partition_count) {
            Ok(partition) => partition,
            Err(error) => {
                return (
                    StatusCode::BAD_REQUEST,
                    Json(serde_json::json!({ "error": error.to_string() })),
                )
                    .into_response();
            }
        };
    let started = Instant::now();
    let mut plan = match sql_to_logical_plan(req.query.trim().trim_end_matches(';')) {
        Ok(plan) => plan,
        Err(error) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({ "error": error.to_string() })),
            )
                .into_response();
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
    match result {
        Ok((schema, batches)) => match encode_arrow_stream(&schema, &batches) {
            Ok(bytes) => Response::builder()
                .status(StatusCode::OK)
                .header(header::CONTENT_TYPE, "application/vnd.apache.arrow.stream")
                .header("x-kaveon-task-elapsed-us", elapsed_us(started))
                .body(Body::from(bytes))
                .unwrap_or_else(|error| {
                    (
                        StatusCode::INTERNAL_SERVER_ERROR,
                        Json(serde_json::json!({ "error": error.to_string() })),
                    )
                        .into_response()
                }),
            Err(error) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({ "error": error })),
            )
                .into_response(),
        },
        Err(error) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": error.to_string() })),
        )
            .into_response(),
    }
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

async fn cancel_query(Path(query_id): Path<String>) -> impl IntoResponse {
    let mut store = QUERY_STORE.write().await;
    if store.queries.remove(&query_id).is_some() {
        StatusCode::NO_CONTENT.into_response()
    } else {
        (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({
                "error": format!("query '{query_id}' not found"),
                "code": "QUERY_NOT_FOUND"
            })),
        )
            .into_response()
    }
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

async fn execute_remote_task(
    client: &reqwest::Client,
    worker: &NodeInfo,
    request: &TaskRequest,
) -> Result<
    (
        arrow::datatypes::SchemaRef,
        Vec<arrow::record_batch::RecordBatch>,
        u64,
        usize,
    ),
    String,
> {
    let url = format!("{}/v1/task", worker.address.trim_end_matches('/'));
    let response = client
        .post(url)
        .json(request)
        .send()
        .await
        .map_err(|error| format!("worker '{}' is unavailable: {error}", worker.node_id))?;
    if !response.status().is_success() {
        let status = response.status();
        let message = response.text().await.unwrap_or_default();
        return Err(format!(
            "worker '{}' failed task with {status}: {message}",
            worker.node_id
        ));
    }
    let elapsed_us = response
        .headers()
        .get("x-kaveon-task-elapsed-us")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse().ok())
        .unwrap_or_default();
    let bytes = response.bytes().await.map_err(|error| {
        format!(
            "worker '{}' result could not be read: {error}",
            worker.node_id
        )
    })?;
    let output_bytes = bytes.len();
    let (schema, batches) = decode_arrow_stream(&bytes).map_err(|error| {
        format!(
            "worker '{}' returned an invalid Arrow stream: {error}",
            worker.node_id
        )
    })?;
    Ok((schema, batches, elapsed_us, output_bytes))
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
    let mut tasks = tokio::task::JoinSet::new();
    for partition_index in 0..partition_count {
        let client = client.clone();
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
                };
                let task_id = kaveon_core::TaskId {
                    query_id: request.query_id.clone(),
                    stage_id: kaveon_core::StageId(request.stage_id),
                    partition: partition_index,
                    attempt,
                }
                .to_string();
                match execute_remote_task(&client, &worker, &request).await {
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
                    Err(error) => failures.push(error),
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
        ColumnInfo, MergeOperation, TaskResponse, aggregate_merge_contract, decode_arrow_stream,
        encode_arrow_stream, merge_partial_aggregates,
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
