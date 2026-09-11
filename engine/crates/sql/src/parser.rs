use kaveon_core::{KaveonError, Result};
use sqlparser::ast::Statement;
use sqlparser::ast::{
    AssignmentTarget, BinaryOperator, Expr, FromTable, SetExpr, TableFactor, TableWithJoins, Value,
};
use sqlparser::dialect::GenericDialect;
use sqlparser::parser::Parser;

pub fn parse_sql(sql: &str) -> Result<Vec<Statement>> {
    let dialect = GenericDialect {};
    Parser::parse_sql(&dialect, sql).map_err(|e| KaveonError::Sql(e.to_string()))
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
    let kind = product_kind(&dml.table)?;
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
            if values.rows.len() != 1 || values.rows[0].len() != 2 {
                return Err(sql_error("product INSERT accepts exactly one record"));
            }
            Ok(ProductDmlCommand::Create {
                kind,
                id: string_literal(&values.rows[0][0], "product ID")?,
                document_json: string_literal(&values.rows[0][1], "product document_json")?,
            })
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
            Ok(ProductDmlCommand::Update {
                kind,
                id,
                expected_revision,
                document_json,
            })
        }
        Statement::Delete(delete) => {
            let (id, expected_revision) = product_key_predicate(delete.selection.as_ref())?;
            Ok(ProductDmlCommand::Delete {
                kind,
                id,
                expected_revision,
            })
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
            "ROLLBACK savepoints and AND CHAIN are not supported",
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
        ];
        for sql in cases {
            assert!(
                parse_native_transactional(sql).is_err(),
                "unexpectedly accepted {sql}"
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
            "INSERT INTO product.datasets (id, document_json) VALUES ('d-1', '{}'), ('d-2', '{}')",
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
}
