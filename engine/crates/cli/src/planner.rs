use kaveon_core::{
    BatchOperator, BatchSource, CatalogManager, DataFormat, Expr, KaveonError, Result,
    TableReference,
};
use kaveon_exec::aggregate::{AggExpr, AggFunc, HashAggregate};
use kaveon_exec::filter::FilterOperator;
use kaveon_exec::join::{HashJoin, JoinType as PhysicalJoinType};
use kaveon_exec::limit::LimitOperator;
use kaveon_exec::project::ProjectOperator;
use kaveon_exec::scan::ScanOperator;
use kaveon_exec::sort::{SortExpr, SortOperator};
use kaveon_exec::topn::TopNOperator;
use kaveon_sql::logical_plan::{AggregateExpr, JoinType, LogicalPlan};
use kaveon_storage::{DeltaTableReader, ParquetReader};

pub fn plan_to_operator(
    plan: &LogicalPlan,
    catalog: &CatalogManager,
) -> Result<Box<dyn BatchOperator>> {
    match plan {
        LogicalPlan::Scan { table, columns, .. } => {
            let reference = TableReference::parse(table);
            let resolved = catalog.resolve_table(&reference)?;
            let path = resolved.full_path();

            let source: Box<dyn BatchSource> = match resolved.table.format {
                DataFormat::Parquet => {
                    let mut reader = ParquetReader::new(&path);
                    if let Some(cols) = columns {
                        reader = reader.with_columns(cols.clone());
                    }
                    Box::new(reader.read().map_err(|e| {
                        KaveonError::Execution(format!("failed to open '{path}': {e}"))
                    })?)
                }
                DataFormat::Delta => {
                    let mut reader = DeltaTableReader::new(&path);
                    if let Some(cols) = columns {
                        reader = reader.with_columns(cols.clone());
                    }
                    Box::new(reader.read().map_err(|e| {
                        KaveonError::Execution(format!("failed to open '{path}': {e}"))
                    })?)
                }
                DataFormat::Iceberg => {
                    return Err(KaveonError::Execution(
                        "local Iceberg scans are not implemented".into(),
                    ));
                }
            };
            let scan = ScanOperator::new(source, columns.as_deref())?;
            Ok(Box::new(scan))
        }

        LogicalPlan::Join {
            left,
            right,
            join_type,
            condition,
        } => {
            let left_qualifier = relation_qualifier(left);
            let right_qualifier = relation_qualifier(right);
            let left = plan_to_operator(left, catalog)?;
            let right = plan_to_operator(right, catalog)?;
            Ok(Box::new(HashJoin::try_new_qualified(
                left,
                right,
                physical_join_type(*join_type),
                join_keys(condition.as_ref())?,
                left_qualifier.as_deref(),
                right_qualifier.as_deref(),
            )?))
        }

        LogicalPlan::Filter { input, predicate } => {
            let source = plan_to_operator(input, catalog)?;
            Ok(Box::new(FilterOperator::new(source, predicate.clone())))
        }

        LogicalPlan::Project { input, columns } => {
            let source = plan_to_operator(input, catalog)?;

            let has_star = columns.iter().any(|e| matches!(e, Expr::Star));
            if has_star && columns.len() == 1 {
                return Ok(source);
            }

            if has_star {
                return Ok(source);
            }

            let exprs: Vec<Expr> = columns
                .iter()
                .map(|e| match e {
                    Expr::Function { name, args } => {
                        let col = agg_output_name(name, args);
                        Expr::Column(col)
                    }
                    Expr::Alias { expr, name } => match expr.as_ref() {
                        Expr::Function { name: fname, args } => {
                            let col = agg_output_name(fname, args);
                            Expr::Alias {
                                expr: Box::new(Expr::Column(col)),
                                name: name.clone(),
                            }
                        }
                        _ => e.clone(),
                    },
                    _ => e.clone(),
                })
                .collect();

            Ok(Box::new(ProjectOperator::new(source, exprs)?))
        }

        LogicalPlan::Aggregate {
            input,
            group_by,
            aggregates,
        } => {
            let source = plan_to_operator(input, catalog)?;

            let group_cols: Vec<String> = group_by
                .iter()
                .map(|e| match e {
                    Expr::Column(name) => resolve_column_name(source.schema(), name),
                    _ => Err(KaveonError::Execution(
                        "only column references supported in GROUP BY".into(),
                    )),
                })
                .collect::<Result<_>>()?;

            let agg_exprs: Vec<AggExpr> = aggregates
                .iter()
                .map(|aggregate| logical_agg_to_exec(aggregate, source.schema()))
                .collect::<Result<_>>()?;

            Ok(Box::new(HashAggregate::new(source, group_cols, agg_exprs)?))
        }

        LogicalPlan::Sort { input, order_by } => {
            let source = plan_to_operator(input, catalog)?;
            let ordering = order_by
                .iter()
                .map(|(expr, ascending)| SortExpr::new(expr.clone(), *ascending))
                .collect();
            Ok(Box::new(SortOperator::new(source, ordering)?))
        }

        LogicalPlan::Limit { input, count } => {
            if let LogicalPlan::Sort {
                input: sort_input,
                order_by,
            } = input.as_ref()
            {
                let source = plan_to_operator(sort_input, catalog)?;
                let ordering = order_by
                    .iter()
                    .map(|(expr, ascending)| SortExpr::new(expr.clone(), *ascending))
                    .collect();
                return Ok(Box::new(TopNOperator::new(source, ordering, *count)?));
            }
            let source = plan_to_operator(input, catalog)?;
            Ok(Box::new(LimitOperator::new(source, *count)))
        }
    }
}

fn logical_agg_to_exec(
    agg: &AggregateExpr,
    input_schema: &arrow::datatypes::SchemaRef,
) -> Result<AggExpr> {
    let (func, expr, distinct) = match agg {
        AggregateExpr::Count { expr, distinct } => (AggFunc::Count, expr, *distinct),
        AggregateExpr::Sum(e) => (AggFunc::Sum, e, false),
        AggregateExpr::Avg(e) => (AggFunc::Avg, e, false),
        AggregateExpr::Min(e) => (AggFunc::Min, e, false),
        AggregateExpr::Max(e) => (AggFunc::Max, e, false),
    };

    let column = match expr {
        Expr::Column(name) => resolve_column_name(input_schema, name)?,
        Expr::Star => "*".to_owned(),
        _ => {
            return Err(KaveonError::Execution(
                "only column references supported in aggregate functions".into(),
            ));
        }
    };

    let expression = AggExpr::new(func, column);
    Ok(if distinct {
        expression.distinct()
    } else {
        expression
    })
}

fn resolve_column_name(schema: &arrow::datatypes::SchemaRef, name: &str) -> Result<String> {
    if schema.index_of(name).is_ok() {
        return Ok(name.to_owned());
    }
    let unqualified = name.rsplit('.').next().unwrap_or(name);
    let count = schema
        .fields()
        .iter()
        .filter(|field| {
            field.name() == unqualified
                || field
                    .name()
                    .strip_suffix(unqualified)
                    .is_some_and(|prefix| prefix.ends_with('.'))
        })
        .count();
    match count {
        1 => Ok(unqualified.to_owned()),
        0 => Err(KaveonError::Execution(format!(
            "column '{name}' not found in input"
        ))),
        _ => Err(KaveonError::Execution(format!(
            "column '{name}' is ambiguous in input"
        ))),
    }
}

fn physical_join_type(join_type: JoinType) -> PhysicalJoinType {
    match join_type {
        JoinType::Inner => PhysicalJoinType::Inner,
        JoinType::Left => PhysicalJoinType::Left,
        JoinType::Right => PhysicalJoinType::Right,
        JoinType::Full => PhysicalJoinType::Full,
        JoinType::Cross => PhysicalJoinType::Cross,
    }
}

fn join_keys(condition: Option<&Expr>) -> Result<Vec<(String, String)>> {
    match condition {
        None => Ok(Vec::new()),
        Some(Expr::BinaryOp {
            left,
            op: kaveon_core::BinaryOp::Eq,
            right,
        }) => match (left.as_ref(), right.as_ref()) {
            (Expr::Column(left), Expr::Column(right)) => {
                Ok(vec![(unqualify(left), unqualify(right))])
            }
            _ => Err(KaveonError::Execution(
                "join equality keys must be column references".into(),
            )),
        },
        Some(Expr::And(left, right)) => {
            let mut keys = join_keys(Some(left))?;
            keys.extend(join_keys(Some(right))?);
            Ok(keys)
        }
        Some(_) => Err(KaveonError::Execution(
            "only equality join conditions are supported".into(),
        )),
    }
}

fn unqualify(column: &str) -> String {
    column.rsplit('.').next().unwrap_or(column).to_owned()
}

fn relation_qualifier(plan: &LogicalPlan) -> Option<String> {
    match plan {
        LogicalPlan::Scan { table, alias, .. } => Some(
            alias
                .clone()
                .unwrap_or_else(|| table.rsplit('.').next().unwrap_or(table).to_owned()),
        ),
        _ => None,
    }
}

fn agg_output_name(func_name: &str, args: &[Expr]) -> String {
    let arg_str = args
        .iter()
        .map(|a| match a {
            Expr::Column(c) => c.clone(),
            Expr::Star => "*".into(),
            _ => "expr".into(),
        })
        .collect::<Vec<_>>()
        .join(", ");
    format!("{}_{}", func_name.to_lowercase(), arg_str)
}
