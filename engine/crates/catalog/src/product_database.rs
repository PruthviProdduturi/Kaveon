//! Transient relational execution for ADLS-backed product snapshots.
//!
//! The SQLite database exists only for one request. Durable state is supplied as
//! typed table snapshots and returned as typed snapshots for immutable Parquet
//! publication by the caller.

use std::sync::Arc;

use arrow::{
    array::{
        Array, ArrayRef, BinaryArray, BinaryBuilder, Float64Array, Float64Builder, Int64Array,
        Int64Builder, StringArray, StringBuilder,
    },
    datatypes::{DataType, Field, Schema},
    record_batch::RecordBatch,
};
use kaveon_core::{KaveonError, Result};
use parquet::arrow::{ArrowWriter, arrow_reader::ParquetRecordBatchReaderBuilder};
use rusqlite::hooks::{AuthAction, Authorization};
use rusqlite::{
    Connection, TransactionBehavior, params_from_iter,
    types::{Value, ValueRef},
};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum ProductValue {
    Null,
    Integer(i64),
    Real(f64),
    Text(String),
    Blob(Vec<u8>),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProductColumnType {
    Integer,
    Real,
    Text,
    Blob,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProductColumn {
    pub name: String,
    pub value_type: ProductColumnType,
    pub nullable: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ProductTable {
    pub name: String,
    pub columns: Vec<ProductColumn>,
    pub rows: Vec<Vec<ProductValue>>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ProductStatement {
    pub sql: String,
    #[serde(default)]
    pub params: Vec<ProductValue>,
    #[serde(default)]
    pub bindings: Vec<ResultBinding>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResultBinding {
    pub parameter_index: usize,
    pub statement_index: usize,
    pub column: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ProductStatementResult {
    pub columns: Vec<String>,
    pub rows: Vec<Vec<ProductValue>>,
    pub affected_rows: usize,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ProductRuntimeRequest {
    pub schema_sql: String,
    pub tables: Vec<ProductTable>,
    pub statements: Vec<ProductStatement>,
    #[serde(default)]
    pub read_only: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ProductRuntimeResult {
    pub statement_results: Vec<ProductStatementResult>,
    pub tables: Vec<ProductTable>,
}

pub struct ProductDatabase;

impl ProductDatabase {
    pub fn execute(request: ProductRuntimeRequest) -> Result<ProductRuntimeResult> {
        validate_request(&request)?;
        validate_schema_sql(&request.schema_sql)?;
        let mut connection = Connection::open_in_memory().map_err(sql_error)?;
        connection
            .execute_batch("PRAGMA foreign_keys = ON; PRAGMA temp_store = MEMORY;")
            .map_err(sql_error)?;
        connection
            .execute_batch(&request.schema_sql)
            .map_err(sql_error)?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(sql_error)?;
        transaction
            .execute_batch("PRAGMA defer_foreign_keys = ON;")
            .map_err(sql_error)?;
        for table in &request.tables {
            load_table(&transaction, table)?;
        }
        let violations: i64 = transaction
            .query_row("SELECT COUNT(*) FROM pragma_foreign_key_check", [], |row| {
                row.get(0)
            })
            .map_err(sql_error)?;
        if violations != 0 {
            return Err(database_error("loaded product tables violate foreign keys"));
        }
        let mut statement_results = Vec::with_capacity(request.statements.len());
        for (index, statement) in request.statements.iter().enumerate() {
            let mut statement = statement.clone();
            resolve_bindings(&mut statement, index, &statement_results)?;
            transaction.authorizer(Some(request_authorizer));
            let result = execute_statement(&transaction, &statement, request.read_only);
            transaction.authorizer(None::<fn(rusqlite::hooks::AuthContext<'_>) -> Authorization>);
            statement_results.push(result?);
        }
        let tables = request
            .tables
            .iter()
            .map(|table| snapshot_table(&transaction, table))
            .collect::<Result<Vec<_>>>()?;
        transaction.commit().map_err(sql_error)?;
        Ok(ProductRuntimeResult {
            statement_results,
            tables,
        })
    }
}

fn validate_request(request: &ProductRuntimeRequest) -> Result<()> {
    if request.schema_sql.len() > 1024 * 1024 || request.statements.len() > 1000 {
        return Err(database_error("product request exceeds configured limits"));
    }
    let rows = request
        .tables
        .iter()
        .map(|table| table.rows.len())
        .sum::<usize>();
    if rows > 100_000
        || serde_json::to_vec(request)
            .map_err(|_| database_error("product request serialization failed"))?
            .len()
            > 64 * 1024 * 1024
    {
        return Err(database_error("product request exceeds configured limits"));
    }
    if request
        .statements
        .iter()
        .any(|statement| statement.sql.len() > 1024 * 1024)
    {
        return Err(database_error("product statement exceeds configured limit"));
    }
    Ok(())
}

fn validate_schema_sql(schema_sql: &str) -> Result<()> {
    let normalized = schema_sql.to_ascii_lowercase();
    for forbidden in [
        "attach",
        "detach",
        "pragma",
        "vacuum",
        "load_extension",
        "readfile",
        "writefile",
        "select",
        "insert",
        "update",
        "delete",
        "begin",
        "commit",
        "rollback",
        "drop",
        "alter",
    ] {
        if normalized
            .split(|value: char| !value.is_ascii_alphanumeric() && value != '_')
            .any(|word| word == forbidden)
        {
            return Err(database_error("schema contains a disallowed operation"));
        }
    }
    if !normalized.split(';').all(|statement| {
        statement.trim().is_empty() || statement.trim_start().starts_with("create ")
    }) {
        return Err(database_error("schema must contain CREATE statements only"));
    }
    Ok(())
}

fn resolve_bindings(
    statement: &mut ProductStatement,
    current: usize,
    results: &[ProductStatementResult],
) -> Result<()> {
    for binding in &statement.bindings {
        if binding.statement_index >= current || binding.parameter_index >= statement.params.len() {
            return Err(database_error("invalid result binding"));
        }
        let result = results
            .get(binding.statement_index)
            .ok_or_else(|| database_error("invalid result binding"))?;
        if result.rows.len() != 1 {
            return Err(database_error("result binding requires exactly one row"));
        }
        let column = result
            .columns
            .iter()
            .position(|column| column == &binding.column)
            .ok_or_else(|| database_error("result binding column not found"))?;
        statement.params[binding.parameter_index] = result.rows[0][column].clone();
    }
    Ok(())
}

fn request_authorizer(context: rusqlite::hooks::AuthContext<'_>) -> Authorization {
    match context.action {
        AuthAction::Transaction { .. }
        | AuthAction::Savepoint { .. }
        | AuthAction::Pragma { .. }
        | AuthAction::Attach { .. }
        | AuthAction::Detach { .. }
        | AuthAction::AlterTable { .. }
        | AuthAction::CreateIndex { .. }
        | AuthAction::CreateTable { .. }
        | AuthAction::CreateTempIndex { .. }
        | AuthAction::CreateTempTable { .. }
        | AuthAction::CreateTempTrigger { .. }
        | AuthAction::CreateTempView { .. }
        | AuthAction::CreateTrigger { .. }
        | AuthAction::CreateView { .. }
        | AuthAction::DropIndex { .. }
        | AuthAction::DropTable { .. }
        | AuthAction::DropTempIndex { .. }
        | AuthAction::DropTempTable { .. }
        | AuthAction::DropTempTrigger { .. }
        | AuthAction::DropTempView { .. }
        | AuthAction::DropTrigger { .. }
        | AuthAction::DropView { .. }
        | AuthAction::CreateVtable { .. }
        | AuthAction::DropVtable { .. } => Authorization::Deny,
        AuthAction::Function { function_name }
            if function_name.eq_ignore_ascii_case("load_extension") =>
        {
            Authorization::Deny
        }
        _ => Authorization::Allow,
    }
}

pub fn encode_table_parquet(table: &ProductTable) -> Result<Vec<u8>> {
    validate_table(table)?;
    let fields: Vec<Field> = table
        .columns
        .iter()
        .map(|column| Field::new(&column.name, arrow_type(column.value_type), column.nullable))
        .collect();
    let schema = Arc::new(Schema::new(fields));
    let arrays = table
        .columns
        .iter()
        .enumerate()
        .map(|(index, column)| build_array(column, &table.rows, index))
        .collect::<Result<Vec<_>>>()?;
    let batch = RecordBatch::try_new(schema.clone(), arrays).map_err(arrow_error)?;
    let mut writer = ArrowWriter::try_new(Vec::new(), schema, None).map_err(parquet_error)?;
    writer.write(&batch).map_err(parquet_error)?;
    writer.into_inner().map_err(parquet_error)
}

pub fn decode_table_parquet(name: impl Into<String>, bytes: Vec<u8>) -> Result<ProductTable> {
    let builder = ParquetRecordBatchReaderBuilder::try_new(bytes::Bytes::from(bytes))
        .map_err(parquet_error)?;
    let schema = builder.schema().clone();
    let columns = schema
        .fields()
        .iter()
        .map(|field| {
            Ok(ProductColumn {
                name: field.name().clone(),
                value_type: product_type(field.data_type())?,
                nullable: field.is_nullable(),
            })
        })
        .collect::<Result<Vec<_>>>()?;
    let reader = builder.build().map_err(parquet_error)?;
    let mut rows = Vec::new();
    for batch in reader {
        let batch = batch.map_err(arrow_error)?;
        for row in 0..batch.num_rows() {
            rows.push(
                (0..batch.num_columns())
                    .map(|column| value_from_array(batch.column(column), row))
                    .collect::<Result<Vec<_>>>()?,
            );
        }
    }
    Ok(ProductTable {
        name: name.into(),
        columns,
        rows,
    })
}

fn load_table(transaction: &rusqlite::Transaction<'_>, table: &ProductTable) -> Result<()> {
    validate_table(table)?;
    if table.rows.is_empty() {
        return Ok(());
    }
    let names = table
        .columns
        .iter()
        .map(|column| quote_identifier(&column.name))
        .collect::<Result<Vec<_>>>()?
        .join(",");
    let values = (1..=table.columns.len())
        .map(|index| format!("?{index}"))
        .collect::<Vec<_>>()
        .join(",");
    let sql = format!(
        "INSERT INTO {} ({names}) VALUES ({values})",
        quote_identifier(&table.name)?
    );
    let mut statement = transaction.prepare(&sql).map_err(sql_error)?;
    for row in &table.rows {
        let values = row.iter().map(value_to_sql).collect::<Result<Vec<_>>>()?;
        statement
            .execute(params_from_iter(values.iter()))
            .map_err(sql_error)?;
    }
    Ok(())
}

fn execute_statement(
    transaction: &rusqlite::Transaction<'_>,
    input: &ProductStatement,
    read_only: bool,
) -> Result<ProductStatementResult> {
    let values = input
        .params
        .iter()
        .map(value_to_sql)
        .collect::<Result<Vec<_>>>()?;
    let mut statement = transaction.prepare(&input.sql).map_err(sql_error)?;
    if read_only && !statement.readonly() {
        return Err(database_error("read-only product request contains a write"));
    }
    if statement.column_count() == 0 {
        return Ok(ProductStatementResult {
            columns: Vec::new(),
            rows: Vec::new(),
            affected_rows: statement
                .execute(params_from_iter(values.iter()))
                .map_err(sql_error)?,
        });
    }
    let columns = statement
        .column_names()
        .iter()
        .map(ToString::to_string)
        .collect();
    let count = statement.column_count();
    let mut query = statement
        .query(params_from_iter(values.iter()))
        .map_err(sql_error)?;
    let mut rows = Vec::new();
    while let Some(row) = query.next().map_err(sql_error)? {
        rows.push(
            (0..count)
                .map(|index| value_from_sql(row.get_ref(index).map_err(sql_error)?))
                .collect(),
        );
    }
    Ok(ProductStatementResult {
        columns,
        rows: rows.into_iter().collect::<Result<Vec<_>>>()?,
        affected_rows: 0,
    })
}

fn snapshot_table(
    transaction: &rusqlite::Transaction<'_>,
    table: &ProductTable,
) -> Result<ProductTable> {
    validate_table(table)?;
    let names = table
        .columns
        .iter()
        .map(|column| quote_identifier(&column.name))
        .collect::<Result<Vec<_>>>()?
        .join(",");
    let sql = format!("SELECT {names} FROM {}", quote_identifier(&table.name)?);
    let mut statement = transaction.prepare(&sql).map_err(sql_error)?;
    let count = table.columns.len();
    let mut query = statement.query([]).map_err(sql_error)?;
    let mut rows = Vec::new();
    while let Some(row) = query.next().map_err(sql_error)? {
        let mut values = Vec::with_capacity(count);
        for index in 0..count {
            values.push(value_from_sql(row.get_ref(index).map_err(sql_error)?)?);
        }
        rows.push(values);
    }
    Ok(ProductTable {
        name: table.name.clone(),
        columns: table.columns.clone(),
        rows,
    })
}

fn validate_table(table: &ProductTable) -> Result<()> {
    quote_identifier(&table.name)?;
    if table.columns.is_empty() {
        return Err(database_error("product table must have columns"));
    }
    for column in &table.columns {
        quote_identifier(&column.name)?;
    }
    for row in &table.rows {
        if row.len() != table.columns.len() {
            return Err(database_error("product row does not match column count"));
        }
    }
    Ok(())
}

fn quote_identifier(value: &str) -> Result<String> {
    if value.is_empty()
        || !value.bytes().enumerate().all(|(index, byte)| {
            byte == b'_' || byte.is_ascii_alphanumeric() && (index != 0 || !byte.is_ascii_digit())
        })
    {
        return Err(database_error("invalid product identifier"));
    }
    Ok(format!("\"{value}\""))
}

fn value_to_sql(value: &ProductValue) -> Result<Value> {
    Ok(match value {
        ProductValue::Null => Value::Null,
        ProductValue::Integer(value) => Value::Integer(*value),
        ProductValue::Real(value) if value.is_finite() => Value::Real(*value),
        ProductValue::Real(_) => {
            return Err(database_error("non-finite real values are not supported"));
        }
        ProductValue::Text(value) => Value::Text(value.clone()),
        ProductValue::Blob(value) => Value::Blob(value.clone()),
    })
}
fn value_from_sql(value: ValueRef<'_>) -> Result<ProductValue> {
    Ok(match value {
        ValueRef::Null => ProductValue::Null,
        ValueRef::Integer(value) => ProductValue::Integer(value),
        ValueRef::Real(value) => ProductValue::Real(value),
        ValueRef::Text(value) => ProductValue::Text(
            String::from_utf8(value.to_vec()).map_err(|_| database_error("invalid UTF-8 text"))?,
        ),
        ValueRef::Blob(value) => ProductValue::Blob(value.to_vec()),
    })
}
fn arrow_type(value: ProductColumnType) -> DataType {
    match value {
        ProductColumnType::Integer => DataType::Int64,
        ProductColumnType::Real => DataType::Float64,
        ProductColumnType::Text => DataType::Utf8,
        ProductColumnType::Blob => DataType::Binary,
    }
}
fn product_type(value: &DataType) -> Result<ProductColumnType> {
    match value {
        DataType::Int64 => Ok(ProductColumnType::Integer),
        DataType::Float64 => Ok(ProductColumnType::Real),
        DataType::Utf8 => Ok(ProductColumnType::Text),
        DataType::Binary => Ok(ProductColumnType::Blob),
        _ => Err(database_error("unsupported Parquet product column type")),
    }
}
fn build_array(
    column: &ProductColumn,
    rows: &[Vec<ProductValue>],
    index: usize,
) -> Result<ArrayRef> {
    match column.value_type {
        ProductColumnType::Integer => {
            let mut builder = Int64Builder::new();
            for row in rows {
                match &row[index] {
                    ProductValue::Null => builder.append_null(),
                    ProductValue::Integer(value) => builder.append_value(*value),
                    _ => {
                        return Err(database_error(
                            "product value does not match integer column",
                        ));
                    }
                }
            }
            Ok(Arc::new(builder.finish()))
        }
        ProductColumnType::Real => {
            let mut builder = Float64Builder::new();
            for row in rows {
                match &row[index] {
                    ProductValue::Null => builder.append_null(),
                    ProductValue::Real(value) if value.is_finite() => builder.append_value(*value),
                    _ => return Err(database_error("product value does not match real column")),
                }
            }
            Ok(Arc::new(builder.finish()))
        }
        ProductColumnType::Text => {
            let mut builder = StringBuilder::new();
            for row in rows {
                match &row[index] {
                    ProductValue::Null => builder.append_null(),
                    ProductValue::Text(value) => builder.append_value(value),
                    _ => return Err(database_error("product value does not match text column")),
                }
            }
            Ok(Arc::new(builder.finish()))
        }
        ProductColumnType::Blob => {
            let mut builder = BinaryBuilder::new();
            for row in rows {
                match &row[index] {
                    ProductValue::Null => builder.append_null(),
                    ProductValue::Blob(value) => builder.append_value(value),
                    _ => return Err(database_error("product value does not match blob column")),
                }
            }
            Ok(Arc::new(builder.finish()))
        }
    }
}
fn value_from_array(array: &ArrayRef, row: usize) -> Result<ProductValue> {
    if array.is_null(row) {
        return Ok(ProductValue::Null);
    }
    match array.data_type() {
        DataType::Int64 => Ok(ProductValue::Integer(
            array
                .as_any()
                .downcast_ref::<Int64Array>()
                .ok_or_else(|| database_error("invalid integer array"))?
                .value(row),
        )),
        DataType::Float64 => Ok(ProductValue::Real(
            array
                .as_any()
                .downcast_ref::<Float64Array>()
                .ok_or_else(|| database_error("invalid real array"))?
                .value(row),
        )),
        DataType::Utf8 => Ok(ProductValue::Text(
            array
                .as_any()
                .downcast_ref::<StringArray>()
                .ok_or_else(|| database_error("invalid text array"))?
                .value(row)
                .to_owned(),
        )),
        DataType::Binary => Ok(ProductValue::Blob(
            array
                .as_any()
                .downcast_ref::<BinaryArray>()
                .ok_or_else(|| database_error("invalid blob array"))?
                .value(row)
                .to_vec(),
        )),
        _ => Err(database_error("unsupported Parquet product column type")),
    }
}
fn sql_error(_: rusqlite::Error) -> KaveonError {
    database_error("SQLite runtime failure")
}
fn arrow_error(_: arrow::error::ArrowError) -> KaveonError {
    database_error("Arrow product snapshot failure")
}
fn parquet_error(_: parquet::errors::ParquetError) -> KaveonError {
    database_error("Parquet product snapshot failure")
}
fn database_error(message: impl Into<String>) -> KaveonError {
    KaveonError::Execution(format!("product database: {}", message.into()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn table(name: &str, columns: Vec<ProductColumn>) -> ProductTable {
        ProductTable {
            name: name.into(),
            columns,
            rows: Vec::new(),
        }
    }

    fn column(name: &str, value_type: ProductColumnType, nullable: bool) -> ProductColumn {
        ProductColumn {
            name: name.into(),
            value_type,
            nullable,
        }
    }

    #[test]
    fn batch_is_atomic_and_resolves_a_prior_returning_value() {
        let result = ProductDatabase::execute(ProductRuntimeRequest {
            schema_sql: "CREATE TABLE parent(id INTEGER PRIMARY KEY, name TEXT NOT NULL UNIQUE); CREATE TABLE child(id INTEGER PRIMARY KEY, parent_id INTEGER NOT NULL REFERENCES parent(id));".into(),
            read_only: false,
            tables: vec![table("parent", vec![column("id", ProductColumnType::Integer, true), column("name", ProductColumnType::Text, false)]), table("child", vec![column("id", ProductColumnType::Integer, true), column("parent_id", ProductColumnType::Integer, false)])],
            statements: vec![
                ProductStatement { sql: "INSERT INTO parent(name) VALUES (?1) RETURNING id".into(), params: vec![ProductValue::Text("one".into())], bindings: vec![] },
                ProductStatement { sql: "INSERT INTO child(parent_id) VALUES (?1)".into(), params: vec![ProductValue::Null], bindings: vec![ResultBinding { parameter_index: 0, statement_index: 0, column: "id".into() }] },
            ],
        }).unwrap();
        assert_eq!(result.tables[0].rows.len(), 1);
        assert_eq!(
            result.tables[1].rows,
            vec![vec![ProductValue::Integer(1), ProductValue::Integer(1)]]
        );
        let failed = ProductDatabase::execute(ProductRuntimeRequest {
            schema_sql: "CREATE TABLE items(id INTEGER PRIMARY KEY, name TEXT NOT NULL UNIQUE);"
                .into(),
            tables: vec![table(
                "items",
                vec![
                    column("id", ProductColumnType::Integer, true),
                    column("name", ProductColumnType::Text, false),
                ],
            )],
            statements: vec![
                ProductStatement {
                    sql: "INSERT INTO items(name) VALUES ('duplicate')".into(),
                    params: vec![],
                    bindings: vec![],
                },
                ProductStatement {
                    sql: "INSERT INTO items(name) VALUES ('duplicate')".into(),
                    params: vec![],
                    bindings: vec![],
                },
            ],
            read_only: false,
        });
        assert!(failed.is_err());
    }

    #[test]
    fn parquet_round_trip_preserves_sqlite_value_kinds_and_nulls() {
        let table = ProductTable {
            name: "values".into(),
            columns: vec![
                column("integer_value", ProductColumnType::Integer, true),
                column("real_value", ProductColumnType::Real, true),
                column("text_value", ProductColumnType::Text, true),
                column("blob_value", ProductColumnType::Blob, true),
            ],
            rows: vec![
                vec![
                    ProductValue::Integer(7),
                    ProductValue::Real(1.5),
                    ProductValue::Text("text".into()),
                    ProductValue::Blob(vec![0, 1, 255]),
                ],
                vec![
                    ProductValue::Null,
                    ProductValue::Null,
                    ProductValue::Null,
                    ProductValue::Null,
                ],
            ],
        };
        let bytes = encode_table_parquet(&table).unwrap();
        assert_eq!(decode_table_parquet("values", bytes).unwrap(), table);
    }

    #[test]
    fn authorizer_rejects_schema_and_transaction_control_in_requests() {
        for sql in [
            "BEGIN",
            "PRAGMA foreign_keys = OFF",
            "ATTACH ':memory:' AS unsafe",
            "CREATE TABLE unsafe(id INTEGER)",
        ] {
            let result = ProductDatabase::execute(ProductRuntimeRequest {
                schema_sql: "CREATE TABLE safe(id INTEGER PRIMARY KEY);".into(),
                tables: vec![table(
                    "safe",
                    vec![column("id", ProductColumnType::Integer, false)],
                )],
                statements: vec![ProductStatement {
                    sql: sql.into(),
                    params: vec![],
                    bindings: vec![],
                }],
                read_only: false,
            });
            assert!(result.is_err(), "{sql}");
        }
    }
}
