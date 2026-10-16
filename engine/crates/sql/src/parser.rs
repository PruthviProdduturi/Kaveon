use kaveon_core::{KaveonError, Result};
use sqlparser::ast::Statement;
use sqlparser::ast::{
    AssignmentTarget, BinaryOperator, Expr, FromTable, SetExpr, TableFactor, TableWithJoins, Value,
};
use sqlparser::dialect::{BigQueryDialect, DuckDbDialect};
use sqlparser::parser::Parser;

pub fn parse_sql(sql: &str) -> Result<Vec<Statement>> {
    // DuckDB's grammar is close to the generic SQL surface while enabling
    // standard aggregate FILTER (WHERE ...) clauses. Unsupported execution
    // features are still rejected by the logical planner with typed errors.
    let dialect = DuckDbDialect {};
    // Studio's completion inserts SQL Server-style bracketed identifiers
    // (`[year]`). sqlparser's generic dialect treats those brackets as an
    // array expression, which later reaches the executor as the unsupported
    // scalar function ARRAY. Quote identifier-shaped brackets before parsing;
    // leave numeric/list brackets untouched so ARRAY[...] remains available
    // for the syntax that explicitly supports it.
    let normalized = normalize_lateral_unnest(&normalize_bracket_identifiers(sql));
    // BigQuery's grammar retains a table-shaped UNNEST node when the function
    // is correlated. DuckDB parses the same spelling as a generic function,
    // which cannot carry ordinality metadata.
    if contains_lateral_unnest(sql)
        && let Ok(statements) = Parser::parse_sql(&BigQueryDialect {}, &normalized)
    {
        return Ok(statements);
    }
    match Parser::parse_sql(&dialect, &normalized) {
        Ok(statements) => Ok(statements),
        Err(primary) if normalized.to_ascii_uppercase().contains("WITH OFFSET") => {
            // sqlparser's DuckDB dialect does not expose BigQuery's
            // zero-based UNNEST offset form. Retry that narrow surface with
            // the dialect that owns the AST fields, leaving all other
            // statements on the normal DuckDB grammar and diagnostics.
            Parser::parse_sql(&BigQueryDialect {}, &normalized)
                .map_err(|_| KaveonError::Sql(primary.to_string()))
        }
        Err(error) => Err(KaveonError::Sql(error.to_string())),
    }
}

fn contains_lateral_unnest(sql: &str) -> bool {
    let upper = sql.to_ascii_uppercase();
    upper
        .split_whitespace()
        .collect::<Vec<_>>()
        .windows(2)
        .any(|window| window[0] == "LATERAL" && window[1].starts_with("UNNEST"))
}

/// The upstream parser represents `LATERAL UNNEST(...)` as a generic table
/// function, which has no `WITH ORDINALITY`/`WITH OFFSET` field. Removing the
/// redundant lateral keyword for this one table-shaped form lets it produce
/// the richer `TableFactor::UNNEST` node; the logical binder still recognizes
/// correlated expressions only when the factor is joined laterally.
fn normalize_lateral_unnest(sql: &str) -> String {
    let bytes = sql.as_bytes();
    let mut out = String::with_capacity(sql.len());
    let mut copied = 0usize;
    let mut i = 0usize;
    let mut quote: Option<u8> = None;
    while i < bytes.len() {
        let byte = bytes[i];
        if let Some(delimiter) = quote {
            if byte == delimiter {
                if i + 1 < bytes.len() && bytes[i + 1] == delimiter {
                    i += 2;
                    continue;
                }
                quote = None;
            }
            i += 1;
            continue;
        }
        if matches!(byte, b'\'' | b'"' | b'`') {
            quote = Some(byte);
            i += 1;
            continue;
        }
        if is_sql_word_at(bytes, i, b"LATERAL") {
            let mut next = i + b"LATERAL".len();
            while next < bytes.len() && bytes[next].is_ascii_whitespace() {
                next += 1;
            }
            if is_sql_word_at(bytes, next, b"UNNEST") {
                out.push_str(&sql[copied..i]);
                copied = i + b"LATERAL".len();
                i = copied;
                continue;
            }
        }
        i += 1;
    }
    if copied == 0 {
        sql.to_owned()
    } else {
        out.push_str(&sql[copied..]);
        out
    }
}

fn is_sql_word_at(bytes: &[u8], offset: usize, word: &[u8]) -> bool {
    offset + word.len() <= bytes.len()
        && bytes[offset..offset + word.len()].eq_ignore_ascii_case(word)
        && (offset == 0 || !is_sql_word_byte(bytes[offset - 1]))
        && (offset + word.len() == bytes.len() || !is_sql_word_byte(bytes[offset + word.len()]))
}

fn is_sql_word_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_'
}

fn normalize_bracket_identifiers(sql: &str) -> String {
    let bytes = sql.as_bytes();
    let mut out = String::with_capacity(sql.len());
    // Everything before `copied` has been written out. The statement is copied
    // as string slices and only the bracket spans this rewrites are emitted
    // character by character.
    //
    // It used to rebuild the whole statement with `out.push(b as char)` per
    // byte. In Rust `u8 as char` is a Latin-1 conversion: byte 0xF0 becomes
    // U+00F0, which the String then re-encodes as 0xC3 0xB0. So every
    // multi-byte UTF-8 sequence in any statement came out as one character per
    // byte -- in identifiers and string literals alike -- and because product
    // records are written as SQL literals, the corruption was stored and
    // compounded on each rewrite. A chart named with an en dash reached the
    // screen as "New Chart ÃÂ¢ÃÂÃÂ ..." after three saves.
    //
    // Byte indexing stays valid because every delimiter scanned for here is
    // ASCII, and a UTF-8 continuation byte can never equal an ASCII byte, so
    // `copied` and the slice bounds always land on character boundaries.
    let mut copied = 0usize;
    let mut i = 0usize;
    let mut quote: Option<u8> = None;
    while i < bytes.len() {
        let b = bytes[i];
        if let Some(q) = quote {
            if b == q {
                if i + 1 < bytes.len() && bytes[i + 1] == q {
                    i += 2;
                    continue;
                }
                quote = None;
            }
            i += 1;
            continue;
        }
        if matches!(b, b'\'' | b'"' | b'`') {
            quote = Some(b);
            i += 1;
            continue;
        }
        if b == b'['
            && let Some(end_rel) = bytes[i + 1..].iter().position(|c| *c == b']')
        {
            let end = i + 1 + end_rel;
            let inner = &sql[i + 1..end];
            let mut chars = inner.chars();
            let identifier = chars
                .next()
                .is_some_and(|c| c == '_' || c.is_ascii_alphabetic())
                && chars.all(|c| c == '_' || c == '$' || c.is_ascii_alphanumeric());
            if identifier {
                out.push_str(&sql[copied..i]);
                out.push('"');
                out.push_str(inner);
                out.push('"');
                i = end + 1;
                copied = i;
                continue;
            }
        }
        i += 1;
    }
    out.push_str(&sql[copied..]);
    out
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NativeDmlKind {
    Insert,
    Update,
    Delete,
}

#[derive(Debug, Clone, PartialEq)]
pub struct NativeDmlStatement {
    pub kind: NativeDmlKind,
    pub table: String,
    pub ast: Statement,
}

#[derive(Debug, Clone, PartialEq)]
pub enum NativeTransactionalStatement {
    Begin,
    Commit,
    Rollback,
    Dml(Box<NativeDmlStatement>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProductDmlCommand {
    Create {
        kind: String,
        id: String,
        document_json: String,
    },
    Update {
        kind: String,
        id: String,
        expected_revision: u64,
        document_json: String,
    },
    Delete {
        kind: String,
        id: String,
        expected_revision: u64,
    },
}

/// Adapt only Kaveon's typed product-record SQL facade. Arbitrary row DML has
/// no storage contract and is rejected instead of being silently emulated.
pub fn adapt_product_dml(dml: &NativeDmlStatement) -> Result<ProductDmlCommand> {
    let mut commands = adapt_product_dml_batch(dml)?;
    if commands.len() != 1 {
        return Err(sql_error(
            "product INSERT accepts one record per statement; use the transaction batch API for multiple records",
        ));
    }
    Ok(commands.remove(0))
}

/// Adapt a product DML statement into one or more transactional commands.
/// Multi-row INSERT is intentionally expanded here so the transaction layer
/// can stage every row under one commit boundary without interpolating SQL.
pub fn adapt_product_dml_batch(dml: &NativeDmlStatement) -> Result<Vec<ProductDmlCommand>> {
    let kind = if dml.table.eq_ignore_ascii_case("kaveon.product.typed_rows")
        || dml.table.eq_ignore_ascii_case("product.typed_rows")
    {
        "typed_row".to_owned()
    } else {
        product_kind(&dml.table)?
    };
    match &dml.ast {
        Statement::Insert(insert) => {
            let columns: Vec<_> = insert
                .columns
                .iter()
                .map(|column| column.value.as_str())
                .collect();
            if columns.len() != 2
                || !columns[0].eq_ignore_ascii_case("id")
                || !columns[1].eq_ignore_ascii_case("document_json")
            {
                return Err(sql_error(
                    "product INSERT columns must be (id, document_json)",
                ));
            }
            let Some(SetExpr::Values(values)) =
                insert.source.as_deref().map(|query| query.body.as_ref())
            else {
                return Err(sql_error("product INSERT requires VALUES"));
            };
            if values.rows.is_empty() || values.rows.iter().any(|row| row.len() != 2) {
                return Err(sql_error(
                    "product INSERT rows must contain id and document_json",
                ));
            }
            values
                .rows
                .iter()
                .map(|row| {
                    Ok(ProductDmlCommand::Create {
                        kind: kind.clone(),
                        id: string_literal(&row[0], "product ID")?,
                        document_json: string_literal(&row[1], "product document_json")?,
                    })
                })
                .collect()
        }
        Statement::Update {
            assignments,
            selection,
            ..
        } => {
            if assignments.len() != 1 {
                return Err(sql_error("product UPDATE may set document_json only"));
            }
            let AssignmentTarget::ColumnName(column) = &assignments[0].target else {
                return Err(sql_error("product UPDATE may set document_json only"));
            };
            if column.0.len() != 1 || !column.0[0].value.eq_ignore_ascii_case("document_json") {
                return Err(sql_error("product UPDATE may set document_json only"));
            }
            let document_json = string_literal(&assignments[0].value, "product document_json")?;
            let (id, expected_revision) = product_key_predicate(selection.as_ref())?;
            Ok(vec![ProductDmlCommand::Update {
                kind,
                id,
                expected_revision,
                document_json,
            }])
        }
        Statement::Delete(delete) => {
            let (id, expected_revision) = product_key_predicate(delete.selection.as_ref())?;
            Ok(vec![ProductDmlCommand::Delete {
                kind,
                id,
                expected_revision,
            }])
        }
        _ => Err(sql_error("statement is not product DML")),
    }
}

fn product_kind(table: &str) -> Result<String> {
    let normalized = table.to_ascii_lowercase();
    let leaf = normalized
        .strip_prefix("kaveon.product.")
        .or_else(|| normalized.strip_prefix("product."));
    match leaf {
        Some("datasets") => Ok("dataset".into()),
        Some("charts") => Ok("chart".into()),
        Some("dashboards") => Ok("dashboard".into()),
        Some("saved_queries") => Ok("saved_query".into()),
        Some("user_themes") => Ok("user_theme".into()),
        Some("dlm_definitions") => Ok("dlm_definition".into()),
        Some("dlm_runs") => Ok("dlm_run".into()),
        Some("favorites") => Ok("favorite".into()),
        Some("sources") => Ok("source".into()),
        Some("user_recents") => Ok("user_recent".into()),
        Some("query_history") => Ok("query_history".into()),
        Some("activity") => Ok("activity".into()),
        Some("chat_sessions") => Ok("chat_session".into()),
        Some("chat_messages") => Ok("chat_message".into()),
        _ => Err(sql_error(
            "row DML is unsupported; target a supported kaveon.product table",
        )),
    }
}

fn product_key_predicate(selection: Option<&Expr>) -> Result<(String, u64)> {
    let Some(Expr::BinaryOp {
        left,
        op: BinaryOperator::And,
        right,
    }) = selection
    else {
        return Err(sql_error(
            "product UPDATE/DELETE requires WHERE id = <string> AND revision = <integer>",
        ));
    };
    let mut id = None;
    let mut revision = None;
    for expression in [left.as_ref(), right.as_ref()] {
        let Expr::BinaryOp {
            left,
            op: BinaryOperator::Eq,
            right,
        } = expression
        else {
            return Err(sql_error(
                "product UPDATE/DELETE requires equality predicates",
            ));
        };
        let Expr::Identifier(column) = left.as_ref() else {
            return Err(sql_error(
                "product key predicates require unqualified columns",
            ));
        };
        if column.value.eq_ignore_ascii_case("id") {
            id = Some(string_literal(right, "product ID")?);
        } else if column.value.eq_ignore_ascii_case("revision") {
            revision = Some(integer_literal(right, "expected revision")?);
        } else {
            return Err(sql_error(
                "product key predicates support id and revision only",
            ));
        }
    }
    match (id, revision) {
        (Some(id), Some(revision)) if revision > 0 => Ok((id, revision)),
        _ => Err(sql_error(
            "product UPDATE/DELETE requires one id and one positive revision",
        )),
    }
}

fn string_literal(expression: &Expr, label: &str) -> Result<String> {
    match expression {
        Expr::Value(Value::SingleQuotedString(value)) => Ok(value.clone()),
        _ => Err(sql_error(&format!("{label} must be a string literal"))),
    }
}

fn integer_literal(expression: &Expr, label: &str) -> Result<u64> {
    match expression {
        Expr::Value(Value::Number(value, false)) => value
            .parse()
            .map_err(|_| sql_error(&format!("{label} must be an unsigned integer literal"))),
        _ => Err(sql_error(&format!(
            "{label} must be an unsigned integer literal"
        ))),
    }
}

/// Parse the deliberately small native write contract.
///
/// This validates syntax only. It does not imply that the coordinator can
/// execute, isolate, commit, or recover the returned statement.
pub fn parse_native_transactional(sql: &str) -> Result<NativeTransactionalStatement> {
    let mut statements = parse_sql(sql)?;
    if statements.len() != 1 {
        return Err(sql_error(
            "native transactions accept exactly one statement per request",
        ));
    }
    let statement = statements.pop().expect("length checked");
    match &statement {
        Statement::StartTransaction {
            modes, modifier, ..
        } if modes.is_empty() && modifier.is_none() => Ok(NativeTransactionalStatement::Begin),
        Statement::StartTransaction { .. } => Err(sql_error(
            "BEGIN isolation, access-mode, and dialect modifiers are not supported",
        )),
        Statement::Commit { chain: false } => Ok(NativeTransactionalStatement::Commit),
        Statement::Commit { chain: true } => Err(sql_error("COMMIT AND CHAIN is not supported")),
        Statement::Rollback {
            chain: false,
            savepoint: None,
        } => Ok(NativeTransactionalStatement::Rollback),
        Statement::Rollback { .. } => Err(sql_error(
            "ROLLBACK TO SAVEPOINT and ROLLBACK AND CHAIN are not supported",
        )),
        Statement::Savepoint { .. } | Statement::ReleaseSavepoint { .. } => Err(sql_error(
            "SAVEPOINT and RELEASE SAVEPOINT are not supported",
        )),
        Statement::SetTransaction { .. } => Err(sql_error(
            "SET TRANSACTION isolation and access modes are not supported",
        )),
        Statement::Insert(insert) => {
            if insert.columns.is_empty() {
                return Err(sql_error("INSERT requires an explicit column list"));
            }
            if !matches!(
                insert.source.as_deref().map(|query| query.body.as_ref()),
                Some(SetExpr::Values(_))
            ) {
                return Err(sql_error("INSERT supports VALUES only"));
            }
            if insert.table_alias.is_some()
                || insert.or.is_some()
                || insert.ignore
                || insert.overwrite
                || insert.partitioned.is_some()
                || !insert.after_columns.is_empty()
                || insert.table
                || insert.on.is_some()
                || insert.returning.is_some()
                || insert.replace_into
                || insert.priority.is_some()
                || insert.insert_alias.is_some()
            {
                return Err(sql_error(
                    "INSERT aliases, conflict clauses, modifiers, partitions, and RETURNING are not supported",
                ));
            }
            let table = validated_table_name(&insert.table_name)?;
            Ok(NativeTransactionalStatement::Dml(Box::new(
                NativeDmlStatement {
                    kind: NativeDmlKind::Insert,
                    table,
                    ast: statement,
                },
            )))
        }
        Statement::Update {
            table,
            assignments,
            from,
            selection,
            returning,
            or,
        } => {
            let table_name = simple_table(table)?;
            if from.is_some() || returning.is_some() || or.is_some() {
                return Err(sql_error(
                    "UPDATE FROM, conflict clauses, and RETURNING are not supported",
                ));
            }
            if selection.is_none() {
                return Err(sql_error("UPDATE requires a WHERE predicate"));
            }
            if assignments.is_empty()
                || assignments.iter().any(|assignment| {
                    !matches!(&assignment.target, AssignmentTarget::ColumnName(name) if name.0.len() == 1)
                })
            {
                return Err(sql_error("UPDATE requires simple single-column assignments"));
            }
            Ok(NativeTransactionalStatement::Dml(Box::new(
                NativeDmlStatement {
                    kind: NativeDmlKind::Update,
                    table: table_name,
                    ast: statement,
                },
            )))
        }
        Statement::Delete(delete) => {
            let tables = match &delete.from {
                FromTable::WithFromKeyword(tables) => tables,
                FromTable::WithoutKeyword(_) => {
                    return Err(sql_error("DELETE requires the FROM keyword"));
                }
            };
            if tables.len() != 1
                || !delete.tables.is_empty()
                || delete.using.is_some()
                || delete.returning.is_some()
                || !delete.order_by.is_empty()
                || delete.limit.is_some()
            {
                return Err(sql_error(
                    "DELETE supports one table and no USING, RETURNING, ORDER BY, or LIMIT",
                ));
            }
            if delete.selection.is_none() {
                return Err(sql_error("DELETE requires a WHERE predicate"));
            }
            let table = simple_table(&tables[0])?;
            Ok(NativeTransactionalStatement::Dml(Box::new(
                NativeDmlStatement {
                    kind: NativeDmlKind::Delete,
                    table,
                    ast: statement,
                },
            )))
        }
        _ => Err(sql_error(
            "native transaction parser accepts INSERT, UPDATE, DELETE, BEGIN, COMMIT, or ROLLBACK only",
        )),
    }
}

fn simple_table(table: &TableWithJoins) -> Result<String> {
    if !table.joins.is_empty() {
        return Err(sql_error("write targets cannot contain joins"));
    }
    match &table.relation {
        TableFactor::Table {
            name,
            alias: None,
            args: None,
            with_hints,
            version: None,
            with_ordinality: false,
            partitions,
            json_path: None,
        } if with_hints.is_empty() && partitions.is_empty() => validated_table_name(name),
        _ => Err(sql_error("write target must be an unaliased base table")),
    }
}

fn validated_table_name(name: &sqlparser::ast::ObjectName) -> Result<String> {
    if !(1..=3).contains(&name.0.len()) {
        return Err(sql_error(
            "write target must be table, schema.table, or catalog.schema.table",
        ));
    }
    Ok(name.to_string())
}

fn sql_error(message: &str) -> KaveonError {
    KaveonError::Sql(message.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every statement passes through `normalize_bracket_identifiers`, and it
    /// used to rebuild the string with `u8 as char` -- a Latin-1 conversion --
    /// so every multi-byte UTF-8 sequence came out as one character per byte.
    /// Product records are written as SQL literals, so the corruption was
    /// stored and compounded on each rewrite.
    #[test]
    fn non_ascii_survives_the_bracket_pass_unchanged() {
        for sql in [
            "SELECT 'caf\u{e9}' AS x FROM t",
            "SELECT 'Probe \u{1f695} en\u{2013}dash' FROM t",
            "SELECT * FROM t WHERE country = 'K\u{f8}benhavn'",
            "SELECT \"na\u{ef}ve\" FROM t",
            "SELECT '\u{4f60}\u{597d}' FROM t",
            // A statement with no brackets at all must come back byte-identical.
            "SELECT a, b FROM t WHERE c = 'plain'",
        ] {
            assert_eq!(normalize_bracket_identifiers(sql), sql, "mangled: {sql}");
        }
    }

    /// The rewrite the function exists for still happens, and still only for
    /// identifier-shaped brackets.
    #[test]
    fn bracket_identifiers_are_quoted_and_other_brackets_are_not() {
        assert_eq!(
            normalize_bracket_identifiers("SELECT [year] FROM t"),
            "SELECT \"year\" FROM t"
        );
        assert_eq!(
            normalize_bracket_identifiers("SELECT [_a$1] FROM t"),
            "SELECT \"_a$1\" FROM t"
        );
        // Numeric and list brackets stay, so ARRAY[...] keeps working.
        assert_eq!(
            normalize_bracket_identifiers("SELECT ARRAY[1, 2] FROM t"),
            "SELECT ARRAY[1, 2] FROM t"
        );
        // A bracket inside a string literal is data, not an identifier.
        assert_eq!(
            normalize_bracket_identifiers("SELECT '[year]' FROM t"),
            "SELECT '[year]' FROM t"
        );
    }

    /// Both together: a rewrite in the same statement as non-ASCII text, which
    /// is where a slice-copying implementation could lose or duplicate a span.
    #[test]
    fn a_bracket_rewrite_and_non_ascii_text_coexist() {
        assert_eq!(
            normalize_bracket_identifiers(
                "SELECT [year] FROM t WHERE city = 'K\u{f8}benhavn' AND note = '\u{2013}'"
            ),
            "SELECT \"year\" FROM t WHERE city = 'K\u{f8}benhavn' AND note = '\u{2013}'"
        );
        assert_eq!(
            normalize_bracket_identifiers("SELECT '\u{1f695}', [year], '\u{2013}' FROM t"),
            "SELECT '\u{1f695}', \"year\", '\u{2013}' FROM t"
        );
    }

    /// An escaped quote inside a literal must not end the literal, or a later
    /// bracket would be rewritten inside what is still a string.
    #[test]
    fn a_doubled_quote_keeps_the_literal_open() {
        assert_eq!(
            normalize_bracket_identifiers("SELECT 'it''s [year]' FROM t"),
            "SELECT 'it''s [year]' FROM t"
        );
    }

    #[test]
    fn accepts_constrained_dml_and_transaction_controls() {
        let cases = [
            ("BEGIN", None),
            ("COMMIT", None),
            ("ROLLBACK", None),
            (
                "INSERT INTO kaveon.app.users (id, name) VALUES (1, 'Ada')",
                Some((NativeDmlKind::Insert, "kaveon.app.users")),
            ),
            (
                "UPDATE app.users SET name = 'Grace' WHERE id = 1",
                Some((NativeDmlKind::Update, "app.users")),
            ),
            (
                "DELETE FROM users WHERE id = 1",
                Some((NativeDmlKind::Delete, "users")),
            ),
        ];
        for (sql, expected) in cases {
            let parsed = parse_native_transactional(sql).unwrap();
            if let Some((kind, table)) = expected {
                let NativeTransactionalStatement::Dml(dml) = parsed else {
                    panic!("expected DML")
                };
                assert_eq!(dml.kind, kind);
                assert_eq!(dml.table, table);
            }
        }
    }

    #[test]
    fn bracketed_identifiers_are_not_parsed_as_arrays() {
        let statements = parse_sql("SELECT DISTINCT [year] FROM climate_energy.energy_annual")
            .expect("bracketed identifier should parse");
        let rendered = statements[0].to_string();
        assert!(rendered.contains("\"year\""));
        assert!(!rendered.contains("ARRAY"));
    }

    #[test]
    fn rejects_unbounded_or_extended_write_forms() {
        let cases = [
            "INSERT INTO users VALUES (1)",
            "INSERT INTO users (id) SELECT id FROM old_users",
            "INSERT INTO users (id) VALUES (1) RETURNING id",
            "UPDATE users SET name = 'Ada'",
            "UPDATE users SET name = old.name FROM old_users old WHERE users.id = old.id",
            "DELETE FROM users",
            "DELETE FROM users USING old_users WHERE users.id = old_users.id",
            "ROLLBACK TO SAVEPOINT before_write",
            "COMMIT AND CHAIN",
            "SAVEPOINT before_write",
            "RELEASE SAVEPOINT before_write",
            "SET TRANSACTION ISOLATION LEVEL SERIALIZABLE",
        ];
        for sql in cases {
            assert!(
                parse_native_transactional(sql).is_err(),
                "unexpectedly accepted {sql}"
            );
        }
    }

    #[test]
    fn transaction_control_rejections_explain_the_unsupported_feature() {
        let cases = [
            (
                "SAVEPOINT checkpoint",
                "SAVEPOINT and RELEASE SAVEPOINT are not supported",
            ),
            (
                "RELEASE SAVEPOINT checkpoint",
                "SAVEPOINT and RELEASE SAVEPOINT are not supported",
            ),
            (
                "SET TRANSACTION ISOLATION LEVEL SERIALIZABLE",
                "SET TRANSACTION isolation and access modes are not supported",
            ),
            (
                "BEGIN ISOLATION LEVEL REPEATABLE READ",
                "BEGIN isolation, access-mode, and dialect modifiers are not supported",
            ),
        ];
        for (sql, expected) in cases {
            assert!(
                parse_native_transactional(sql)
                    .unwrap_err()
                    .to_string()
                    .contains(expected),
                "{sql} did not report {expected}"
            );
        }
    }

    #[test]
    fn rejects_batches_and_non_transactional_sql() {
        for sql in [
            "BEGIN; DELETE FROM users WHERE id = 1",
            "SELECT * FROM users",
            "CREATE TABLE users (id BIGINT)",
        ] {
            assert!(
                parse_native_transactional(sql).is_err(),
                "unexpectedly accepted {sql}"
            );
        }
    }

    #[test]
    fn accepts_semicolon_terminated_single_transaction_statements() {
        for sql in [
            "BEGIN;",
            "COMMIT;",
            "ROLLBACK;",
            "INSERT INTO product.datasets (id, document_json) VALUES ('ds-1', '{}');",
            "UPDATE product.datasets SET document_json = '{}' WHERE id = 'ds-1' AND revision = 1;",
            "DELETE FROM product.datasets WHERE id = 'ds-1' AND revision = 1;",
        ] {
            assert!(
                parse_native_transactional(sql).is_ok(),
                "expected supported single statement: {sql}"
            );
        }
    }

    #[test]
    fn product_adapter_rejects_unbound_parameter_values() {
        let NativeTransactionalStatement::Dml(dml) = parse_native_transactional(
            "INSERT INTO product.datasets (id, document_json) VALUES ($1, $2)",
        )
        .unwrap() else {
            panic!("expected product DML")
        };
        assert!(adapt_product_dml(&dml).is_err());
    }

    #[test]
    fn adapts_only_exact_product_record_dml() {
        let parsed = parse_native_transactional(
            "INSERT INTO kaveon.product.datasets (id, document_json) VALUES ('ds-1', '{\"name\":\"Orders\"}')",
        ).unwrap();
        let NativeTransactionalStatement::Dml(dml) = parsed else {
            panic!("expected DML")
        };
        assert_eq!(
            adapt_product_dml(&dml).unwrap(),
            ProductDmlCommand::Create {
                kind: "dataset".into(),
                id: "ds-1".into(),
                document_json: "{\"name\":\"Orders\"}".into(),
            }
        );
        for sql in [
            "UPDATE product.charts SET document_json = '{}' WHERE id = 'c-1' AND revision = 2",
            "DELETE FROM product.dashboards WHERE id = 'd-1' AND revision = 3",
            "INSERT INTO product.dlm_definitions (id, document_json) VALUES ('ds-1', '{}')",
            "INSERT INTO product.dlm_runs (id, document_json) VALUES ('run-1', '{}')",
            "INSERT INTO kaveon.product.typed_rows (id, document_json) VALUES ('row-1', '{}')",
        ] {
            let NativeTransactionalStatement::Dml(dml) = parse_native_transactional(sql).unwrap()
            else {
                panic!("expected DML")
            };
            assert!(adapt_product_dml(&dml).is_ok());
        }
    }

    #[test]
    fn product_adapter_rejects_generic_rows_and_unsafe_shapes() {
        for sql in [
            "INSERT INTO app.users (id, document_json) VALUES ('u-1', '{}')",
            "INSERT INTO product.datasets (document_json, id) VALUES ('{}', 'd-1')",
            "UPDATE product.datasets SET document_json = '{}' WHERE id = 'd-1' AND owner = 'alice'",
            "DELETE FROM product.datasets WHERE revision = 1 AND revision = 2",
        ] {
            let NativeTransactionalStatement::Dml(dml) = parse_native_transactional(sql).unwrap()
            else {
                panic!("expected DML")
            };
            assert!(
                adapt_product_dml(&dml).is_err(),
                "unexpectedly adapted {sql}"
            );
        }
    }

    #[test]
    fn product_insert_batch_expands_rows_without_sql_interpolation() {
        let NativeTransactionalStatement::Dml(dml) = parse_native_transactional(
            "INSERT INTO product.datasets (id, document_json) VALUES ('one', '{}'), ('two', '{\"n\":2}')",
        )
        .unwrap() else {
            panic!("expected product DML");
        };
        let commands = adapt_product_dml_batch(&dml).unwrap();
        assert_eq!(commands.len(), 2);
        assert_eq!(
            commands[0],
            ProductDmlCommand::Create {
                kind: "dataset".into(),
                id: "one".into(),
                document_json: "{}".into(),
            }
        );
        assert_eq!(
            commands[1],
            ProductDmlCommand::Create {
                kind: "dataset".into(),
                id: "two".into(),
                document_json: "{\"n\":2}".into(),
            }
        );
        assert!(adapt_product_dml(&dml).is_err());
    }
}
