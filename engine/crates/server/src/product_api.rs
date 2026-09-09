//! Private product metadata bridge. Application routes retain user authorization;
//! these routes require the separate catalog-service credential on every call.
use std::sync::Arc;

use axum::{extract::{DefaultBodyLimit, State}, http::{HeaderMap, StatusCode}, response::{IntoResponse, Response}, routing::post, Json, Router};
use kaveon_catalog::{product_database::{ProductRuntimeResult, ProductStatement, ProductTable, ProductValue, ResultBinding}, product_repository::{ProductRepository, RepositoryError}};
use serde::Deserialize;
use serde_json::{json, Value};

use crate::AppState;

pub fn configured_repository() -> anyhow::Result<Option<Arc<ProductRepository>>> {
    let Some(uri) = std::env::var("KAVEON_PRODUCT_CATALOG_URI").ok().filter(|v| !v.is_empty()) else { return Ok(None); };
    anyhow::ensure!(uri.starts_with("abfss://"), "product catalog requires ADLS Gen2");
    let location = kaveon_storage::ObjectLocation::parse(&uri)?;
    let repository = ProductRepository::new(kaveon_storage::AdlsConditionalCommit::new(location.store), location.path.to_string())
        .map_err(|_| anyhow::anyhow!("invalid product catalog configuration"))?;
    Ok(Some(Arc::new(repository)))
}

pub fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/v1/product/transaction", post(transaction))
        .route("/v1/product/bootstrap", post(bootstrap))
        .layer(DefaultBodyLimit::max(64 * 1024 * 1024))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct StatementRequest { sql: String, #[serde(default)] params: Vec<Value> }

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TransactionRequest {
    operation_id: String,
    #[serde(default)] read_only: bool,
    statements: Vec<StatementRequest>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct BootstrapRequest { operation_id: String, schema_sql: String, tables: Vec<ProductTable> }

fn authorize(state: &AppState, headers: &HeaderMap) -> Result<Arc<ProductRepository>, Response> {
    if let Err(response) = crate::api::mutation_actor(state, headers) { return Err(*response); }
    state.product_catalog.clone().ok_or_else(|| (StatusCode::SERVICE_UNAVAILABLE, Json(json!({"error":"product catalog is not configured"}))).into_response())
}

async fn transaction(State(state): State<Arc<AppState>>, headers: HeaderMap, Json(request): Json<TransactionRequest>) -> Response {
    let repository = match authorize(&state, &headers) { Ok(value) => value, Err(response) => return response };
    let statements: Result<Vec<_>, _> = request.statements.into_iter().map(convert_statement).collect();
    let statements = match statements { Ok(value) => value, Err(()) => return (StatusCode::BAD_REQUEST, Json(json!({"error":"invalid typed statement parameters"}))).into_response() };
    response(repository.execute(statements, request.operation_id, request.read_only).await)
}

async fn bootstrap(State(state): State<Arc<AppState>>, headers: HeaderMap, Json(request): Json<BootstrapRequest>) -> Response {
    let repository = match authorize(&state, &headers) { Ok(value) => value, Err(response) => return response };
    response(repository.initialize(request.schema_sql, request.tables, request.operation_id).await)
}

fn convert_statement(request: StatementRequest) -> Result<ProductStatement, ()> {
    let mut params = Vec::with_capacity(request.params.len());
    let mut bindings = Vec::new();
    for value in request.params {
        if let Some(object) = value.as_object() {
            if object.len() != 1 { return Err(()); }
            let binding = object.get("$result").and_then(Value::as_object).ok_or(())?;
            if binding.len() != 2 { return Err(()); }
            bindings.push(ResultBinding {
                parameter_index: params.len(),
                statement_index: usize::try_from(binding.get("statement").and_then(Value::as_u64).ok_or(())?).map_err(|_| ())?,
                column: binding.get("column").and_then(Value::as_str).ok_or(())?.to_owned(),
            });
            params.push(ProductValue::Null);
        } else {
            params.push(match value {
                Value::Null => ProductValue::Null,
                Value::Bool(value) => ProductValue::Integer(i64::from(value)),
                Value::Number(value) => if let Some(value) = value.as_i64() { ProductValue::Integer(value) } else { ProductValue::Real(value.as_f64().ok_or(())?) },
                Value::String(value) => ProductValue::Text(value),
                _ => return Err(()),
            });
        }
    }
    Ok(ProductStatement { sql: request.sql, params, bindings })
}

fn plain(value: ProductValue) -> Value {
    match value {
        ProductValue::Null => Value::Null,
        ProductValue::Integer(value) => json!(value),
        ProductValue::Real(value) => json!(value),
        ProductValue::Text(value) => json!(value),
        ProductValue::Blob(value) => json!({"$binary":value}),
    }
}

fn response(result: Result<ProductRuntimeResult, RepositoryError>) -> Response {
    match result {
        Ok(result) => {
            let results: Vec<_> = result.statement_results.into_iter().map(|statement| {
                let rows: Vec<Value> = statement.rows.into_iter().map(|row| {
                    Value::Object(statement.columns.iter().cloned().zip(row.into_iter().map(plain)).collect())
                }).collect();
                let count = if statement.columns.is_empty() { statement.affected_rows } else { rows.len() };
                json!({"rows": rows, "row_count": count})
            }).collect();
            Json(json!({"results":results})).into_response()
        }
        Err(error) => {
            let (status, code) = match error {
                RepositoryError::Conflict => (StatusCode::CONFLICT, "transaction_conflict"),
                RepositoryError::Indeterminate => (StatusCode::SERVICE_UNAVAILABLE, "commit_outcome_unknown"),
                RepositoryError::Invalid => (StatusCode::BAD_REQUEST, "invalid_transaction"),
                RepositoryError::Missing => (StatusCode::SERVICE_UNAVAILABLE, "product_catalog_not_initialized"),
                RepositoryError::Storage => (StatusCode::SERVICE_UNAVAILABLE, "product_storage_unavailable"),
            };
            (status, Json(json!({"error":code}))).into_response()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn converts_bindings_without_interpolating_values_into_sql() {
        let statement = convert_statement(StatementRequest { sql: "INSERT INTO children VALUES (?, ?)".into(), params: vec![json!({"$result":{"statement":0,"column":"id"}}), json!("' OR 1=1 --")] }).unwrap();
        assert_eq!(statement.bindings[0].parameter_index, 0);
        assert_eq!(statement.params[1], ProductValue::Text("' OR 1=1 --".into()));
        assert_eq!(statement.sql, "INSERT INTO children VALUES (?, ?)");
    }
}
