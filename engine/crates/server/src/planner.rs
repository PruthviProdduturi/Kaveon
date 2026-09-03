use kaveon_core::{
    BatchOperator, BatchSource, CatalogManager, DataFormat, Expr, KaveonError, Result,
    TableReference,
};
use kaveon_exec::aggregate::{AggExpr, AggFunc, HashAggregate};
use kaveon_exec::filter::FilterOperator;
use kaveon_exec::limit::LimitOperator;
use kaveon_exec::project::ProjectOperator;
use kaveon_exec::scan::ScanOperator;
use kaveon_sql::logical_plan::{AggregateExpr, LogicalPlan};
use kaveon_storage::{DeltaTableReader, ParquetReader};
use std::collections::BTreeMap;

pub struct PlannedQuery {
    pub operator: Box<dyn BatchOperator>,
    pub scan_metrics: Vec<kaveon_storage::ScanMetrics>,
}

pub fn plan_to_operator(
    plan: &LogicalPlan,
    catalog: &CatalogManager,
) -> Result<Box<dyn BatchOperator>> {
    Ok(plan_query(plan, catalog)?.operator)
}

pub fn plan_query(plan: &LogicalPlan, catalog: &CatalogManager) -> Result<PlannedQuery> {
    plan_query_inner(plan, catalog)
}

pub fn logical_plan_tree(plan: &LogicalPlan) -> kaveon_core::PlanNode {
    let mut next_id = 0;
    build_logical_plan_tree(plan, &mut next_id)
}

fn build_logical_plan_tree(plan: &LogicalPlan, next_id: &mut u32) -> kaveon_core::PlanNode {
    let id = *next_id;
    *next_id = next_id.saturating_add(1);
    let (operator, attributes, input) = match plan {
        LogicalPlan::Scan { table, columns } => {
            let mut attributes = BTreeMap::from([("table".to_owned(), table.clone())]);
            if let Some(columns) = columns {
                attributes.insert("columns".to_owned(), columns.join(", "));
            }
            ("Scan", attributes, None)
        }
        LogicalPlan::Filter { input, predicate } => (
            "Filter",
            BTreeMap::from([("predicate".to_owned(), format!("{predicate:?}"))]),
            Some(input.as_ref()),
        ),
        LogicalPlan::Project { input, columns } => (
            "Project",
            BTreeMap::from([("expressions".to_owned(), format!("{columns:?}"))]),
            Some(input.as_ref()),
        ),
        LogicalPlan::Aggregate {
            input,
            group_by,
            aggregates,
        } => (
            "Aggregate",
            BTreeMap::from([
                ("group_by".to_owned(), format!("{group_by:?}")),
                ("aggregates".to_owned(), format!("{aggregates:?}")),
            ]),
            Some(input.as_ref()),
        ),
        LogicalPlan::Sort { input, order_by } => (
            "Sort",
            BTreeMap::from([("order_by".to_owned(), format!("{order_by:?}"))]),
            Some(input.as_ref()),
        ),
        LogicalPlan::Limit { input, count } => (
            "Limit",
            BTreeMap::from([("rows".to_owned(), count.to_string())]),
            Some(input.as_ref()),
        ),
    };
    let mut node = kaveon_core::PlanNode::new(id, kaveon_core::PlanPhase::Logical, operator);
    node.attributes = attributes;
    if let Some(input) = input {
        node.children.push(build_logical_plan_tree(input, next_id));
    }
    node
}

fn plan_query_inner(plan: &LogicalPlan, catalog: &CatalogManager) -> Result<PlannedQuery> {
    match plan {
        LogicalPlan::Scan { table, columns } => {
            let reference = TableReference::parse(table);
            let resolved = catalog.resolve_table(&reference)?;
            let path = resolved.full_path();

            let (source, scan_metrics): (Box<dyn BatchSource>, _) = match resolved.table.format {
                DataFormat::Parquet => {
                    let mut reader = ParquetReader::new(&path);
                    if let Some(cols) = columns {
                        reader = reader.with_columns(cols.clone());
                    }
                    let source = reader.read().map_err(|e| {
                        KaveonError::Execution(format!("failed to open '{path}': {e}"))
                    })?;
                    let metrics = source.metrics();
                    (Box::new(source), vec![metrics])
                }
                DataFormat::Delta => {
                    let mut reader = DeltaTableReader::new(&path);
                    if let Some(cols) = columns {
                        reader = reader.with_columns(cols.clone());
                    }
                    let source = reader.read().map_err(|e| {
                        KaveonError::Execution(format!("failed to open '{path}': {e}"))
                    })?;
                    let metrics = source.metrics();
                    (Box::new(source), vec![metrics])
                }
                DataFormat::Iceberg => {
                    return Err(KaveonError::Execution(
                        "local Iceberg scans are not implemented".into(),
                    ));
                }
            };
            let scan = ScanOperator::new(source, columns.as_deref())?;
            Ok(PlannedQuery {
                operator: Box::new(scan),
                scan_metrics,
            })
        }

        LogicalPlan::Filter { input, predicate } => {
            let planned = plan_query_inner(input, catalog)?;
            Ok(PlannedQuery {
                operator: Box::new(FilterOperator::new(planned.operator, predicate.clone())),
                scan_metrics: planned.scan_metrics,
            })
        }

        LogicalPlan::Project { input, columns } => {
            let planned = plan_query_inner(input, catalog)?;

            let has_star = columns.iter().any(|e| matches!(e, Expr::Star));
            if has_star {
                return Ok(planned);
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

            Ok(PlannedQuery {
                operator: Box::new(ProjectOperator::new(planned.operator, exprs)?),
                scan_metrics: planned.scan_metrics,
            })
        }

        LogicalPlan::Aggregate {
            input,
            group_by,
            aggregates,
        } => {
            let planned = plan_query_inner(input, catalog)?;

            let group_cols: Vec<String> = group_by
                .iter()
                .map(|e| match e {
                    Expr::Column(name) => Ok(name.clone()),
                    _ => Err(KaveonError::Execution(
                        "only column references supported in GROUP BY".into(),
                    )),
                })
                .collect::<Result<_>>()?;

            let agg_exprs: Vec<AggExpr> = aggregates
                .iter()
                .map(logical_agg_to_exec)
                .collect::<Result<_>>()?;

            Ok(PlannedQuery {
                operator: Box::new(HashAggregate::new(planned.operator, group_cols, agg_exprs)?),
                scan_metrics: planned.scan_metrics,
            })
        }

        LogicalPlan::Sort { input, .. } => plan_query_inner(input, catalog),

        LogicalPlan::Limit { input, count } => {
            let planned = plan_query_inner(input, catalog)?;
            Ok(PlannedQuery {
                operator: Box::new(LimitOperator::new(planned.operator, *count)),
                scan_metrics: planned.scan_metrics,
            })
        }
    }
}

fn logical_agg_to_exec(agg: &AggregateExpr) -> Result<AggExpr> {
    let (func, expr) = match agg {
        AggregateExpr::Count(e) => (AggFunc::Count, e),
        AggregateExpr::Sum(e) => (AggFunc::Sum, e),
        AggregateExpr::Avg(e) => (AggFunc::Avg, e),
        AggregateExpr::Min(e) => (AggFunc::Min, e),
        AggregateExpr::Max(e) => (AggFunc::Max, e),
    };

    let column = match expr {
        Expr::Column(name) => name.clone(),
        Expr::Star => "*".to_owned(),
        _ => {
            return Err(KaveonError::Execution(
                "only column references supported in aggregate functions".into(),
            ));
        }
    };

    Ok(AggExpr::new(func, column))
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
