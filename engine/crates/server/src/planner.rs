use kaveon_core::{BatchOperator, CatalogManager, Expr, KaveonError, Result, TableReference};
use kaveon_exec::aggregate::{AggExpr, AggFunc, HashAggregate};
use kaveon_exec::filter::FilterOperator;
use kaveon_exec::limit::LimitOperator;
use kaveon_exec::project::ProjectOperator;
use kaveon_exec::scan::ScanOperator;
use kaveon_sql::logical_plan::{AggregateExpr, LogicalPlan};
use kaveon_storage::ParquetReader;

pub fn plan_to_operator(
    plan: &LogicalPlan,
    catalog: &CatalogManager,
) -> Result<Box<dyn BatchOperator>> {
    match plan {
        LogicalPlan::Scan { table, columns } => {
            let reference = TableReference::parse(table);
            let resolved = catalog.resolve_table(&reference)?;
            let path = resolved.full_path();

            let mut reader = ParquetReader::new(&path);
            if let Some(cols) = columns {
                reader = reader.with_columns(cols.clone());
            }
            let source = reader
                .read()
                .map_err(|e| KaveonError::Execution(format!("failed to open '{}': {e}", path)))?;
            let scan = ScanOperator::new(Box::new(source), columns.as_deref())?;
            Ok(Box::new(scan))
        }

        LogicalPlan::Filter { input, predicate } => {
            let source = plan_to_operator(input, catalog)?;
            Ok(Box::new(FilterOperator::new(source, predicate.clone())))
        }

        LogicalPlan::Project { input, columns } => {
            let source = plan_to_operator(input, catalog)?;

            let has_star = columns.iter().any(|e| matches!(e, Expr::Star));
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

            Ok(Box::new(HashAggregate::new(source, group_cols, agg_exprs)?))
        }

        LogicalPlan::Sort { input, .. } => plan_to_operator(input, catalog),

        LogicalPlan::Limit { input, count } => {
            let source = plan_to_operator(input, catalog)?;
            Ok(Box::new(LimitOperator::new(source, *count)))
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
