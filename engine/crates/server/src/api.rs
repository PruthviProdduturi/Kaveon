use crate::AppState;
use crate::cluster::{NodeInfo, NodeRole};
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::{delete, get, post};
use axum::{Json, Router};
use kaveon_core::collect_batches;
use kaveon_sql::logical_plan::sql_to_logical_plan;
use serde::{Deserialize, Serialize};
use std::cmp::Reverse;
use std::collections::HashMap;
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
    logical: Option<String>,
    physical: Option<String>,
}

const QUERY_HISTORY_LIMIT: usize = 100;

#[derive(Clone, Copy, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
enum QueryState {
    Running,
    Finished,
    Failed,
}

#[derive(Clone, Serialize)]
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
                physical: None,
            },
            scans: vec![],
        },
    );

    let analysis_start = Instant::now();
    let plan = match sql_to_logical_plan(&sql) {
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
    let analysis_us = elapsed_us(analysis_start);
    let logical_plan = format!("{plan:#?}");
    if let Some(record) = QUERY_STORE.write().await.queries.get_mut(&query_id) {
        record.timings.analysis_us = Some(analysis_us);
        record.plan.logical = Some(logical_plan.clone());
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
                    physical: None,
                },
                scans,
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
            physical: None,
        },
        scans,
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
    logical_plan: Option<String>,
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
