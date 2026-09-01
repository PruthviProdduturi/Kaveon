use crate::parser::parse_sql;
use kaveon_core::{KaveonError, Result};

#[derive(Debug)]
pub enum LogicalPlan {
    Scan {
        table: String,
        columns: Option<Vec<String>>,
    },
    Filter {
        input: Box<LogicalPlan>,
        predicate: String,
    },
    Project {
        input: Box<LogicalPlan>,
        columns: Vec<String>,
    },
    Aggregate {
        input: Box<LogicalPlan>,
        group_by: Vec<String>,
        aggregates: Vec<(String, String)>,
    },
    Sort {
        input: Box<LogicalPlan>,
        order_by: Vec<(String, bool)>,
    },
    Limit {
        input: Box<LogicalPlan>,
        count: usize,
    },
}

pub fn sql_to_logical_plan(sql: &str) -> Result<LogicalPlan> {
    let stmts = parse_sql(sql)?;
    if stmts.is_empty() {
        return Err(KaveonError::Sql("empty query".into()));
    }
    // TODO: full AST → logical plan translation
    Err(KaveonError::Sql("logical plan translation not yet implemented".into()))
}
