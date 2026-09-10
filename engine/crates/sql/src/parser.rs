use kaveon_core::{KaveonError, Result};
use sqlparser::ast::Statement;
use sqlparser::ast::{AssignmentTarget, FromTable, SetExpr, TableFactor, TableWithJoins};
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
}
