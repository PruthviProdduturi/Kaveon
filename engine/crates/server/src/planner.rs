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
use kaveon_optim::rules::to_storage_predicate;
use kaveon_sql::logical_plan::{AggregateExpr, JoinType, LogicalPlan};
use kaveon_storage::{DeltaTableReader, ParquetReader, ScanPartition};
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
    plan_query_inner(plan, catalog, None)
}

pub fn plan_partitioned_query(
    plan: &LogicalPlan,
    catalog: &CatalogManager,
    partition: ScanPartition,
) -> Result<PlannedQuery> {
    plan_query_inner(plan, catalog, Some(partition))
}

pub fn qualify_tables(plan: &mut LogicalPlan, catalog: &str, schema: &str) {
    match plan {
        LogicalPlan::Scan { table, .. } => {
            *table = match TableReference::parse(table) {
                TableReference::Bare { table } => format!("{catalog}.{schema}.{table}"),
                TableReference::Partial { schema, table } => {
                    format!("{catalog}.{schema}.{table}")
                }
                TableReference::Full {
                    catalog,
                    schema,
                    table,
                } => format!("{catalog}.{schema}.{table}"),
            };
        }
        LogicalPlan::Join { left, right, .. } => {
            qualify_tables(left, catalog, schema);
            qualify_tables(right, catalog, schema);
        }
        LogicalPlan::Filter { input, .. }
        | LogicalPlan::Project { input, .. }
        | LogicalPlan::Aggregate { input, .. }
        | LogicalPlan::Sort { input, .. }
        | LogicalPlan::Limit { input, .. } => qualify_tables(input, catalog, schema),
    }
}

pub fn logical_plan_tree(plan: &LogicalPlan) -> kaveon_core::PlanNode {
    let mut next_id = 0;
    build_plan_tree(plan, &mut next_id, kaveon_core::PlanPhase::Logical)
}

pub fn optimized_plan_tree(plan: &LogicalPlan) -> kaveon_core::PlanNode {
    let mut next_id = 0;
    build_plan_tree(plan, &mut next_id, kaveon_core::PlanPhase::OptimizedLogical)
}

pub fn physical_plan_tree(plan: &LogicalPlan) -> kaveon_core::PlanNode {
    let mut next_id = 0;
    build_plan_tree(plan, &mut next_id, kaveon_core::PlanPhase::Physical)
}

fn build_plan_tree(
    plan: &LogicalPlan,
    next_id: &mut u32,
    phase: kaveon_core::PlanPhase,
) -> kaveon_core::PlanNode {
    let id = *next_id;
    *next_id = next_id.saturating_add(1);
    if phase == kaveon_core::PlanPhase::Physical
        && let LogicalPlan::Limit { input, count } = plan
        && let LogicalPlan::Sort { input, order_by } = input.as_ref()
    {
        let mut node = kaveon_core::PlanNode::new(id, phase, "TopN");
        node.attributes = BTreeMap::from([
            ("rows".to_owned(), count.to_string()),
            ("order_by".to_owned(), format!("{order_by:?}")),
        ]);
        node.children.push(build_plan_tree(input, next_id, phase));
        return node;
    }
    let (operator, attributes, input) = match plan {
        LogicalPlan::Scan { table, columns, .. } => {
            let mut attributes = BTreeMap::from([("table".to_owned(), table.clone())]);
            if let Some(columns) = columns {
                attributes.insert("columns".to_owned(), columns.join(", "));
            }
            ("Scan", attributes, None)
        }
        LogicalPlan::Join {
            join_type,
            condition,
            ..
        } => (
            "Join",
            BTreeMap::from([
                ("type".to_owned(), format!("{join_type:?}")),
                ("condition".to_owned(), format!("{condition:?}")),
            ]),
            None,
        ),
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
    let mut node = kaveon_core::PlanNode::new(id, phase, operator);
    node.attributes = attributes;
    if let Some(input) = input {
        node.children.push(build_plan_tree(input, next_id, phase));
    } else if let LogicalPlan::Join { left, right, .. } = plan {
        node.children.push(build_plan_tree(left, next_id, phase));
        node.children.push(build_plan_tree(right, next_id, phase));
    }
    node
}

fn plan_query_inner(
    plan: &LogicalPlan,
    catalog: &CatalogManager,
    partition: Option<ScanPartition>,
) -> Result<PlannedQuery> {
    plan_query_with_predicate(plan, catalog, None, partition)
}

fn plan_query_with_predicate(
    plan: &LogicalPlan,
    catalog: &CatalogManager,
    storage_predicate: Option<&kaveon_core::StoragePredicate>,
    partition: Option<ScanPartition>,
) -> Result<PlannedQuery> {
    match plan {
        LogicalPlan::Scan { table, columns, .. } => {
            let reference = TableReference::parse(table);
            let resolved = catalog.resolve_table(&reference)?;
            let path = resolved.full_path();

            let (source, scan_metrics): (Box<dyn BatchSource>, _) = match resolved.table.format {
                DataFormat::Parquet => {
                    let mut reader = ParquetReader::new(&path);
                    if let Some(cols) = columns {
                        reader = reader.with_columns(cols.clone());
                    }
                    if let Some(predicate) = storage_predicate {
                        reader = reader.with_predicate(predicate.clone());
                    }
                    if let Some(partition) = partition {
                        reader = reader.with_partition(partition);
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
                    if let Some(partition) = partition {
                        reader = reader.with_partition(partition);
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

        LogicalPlan::Join {
            left,
            right,
            join_type,
            condition,
        } => {
            let left_qualifier = relation_qualifier(left);
            let right_qualifier = relation_qualifier(right);
            let left = plan_query_inner(left, catalog, partition)?;
            let right = plan_query_inner(right, catalog, partition)?;
            let keys = join_keys(condition.as_ref())?;
            let mut scan_metrics = left.scan_metrics;
            scan_metrics.extend(right.scan_metrics);
            Ok(PlannedQuery {
                operator: Box::new(HashJoin::try_new_qualified(
                    left.operator,
                    right.operator,
                    physical_join_type(*join_type),
                    keys,
                    left_qualifier.as_deref(),
                    right_qualifier.as_deref(),
                )?),
                scan_metrics,
            })
        }

        LogicalPlan::Filter { input, predicate } => {
            let pushed = to_storage_predicate(predicate);
            let planned = plan_query_with_predicate(input, catalog, pushed.as_ref(), partition)?;
            Ok(PlannedQuery {
                operator: Box::new(FilterOperator::new(planned.operator, predicate.clone())),
                scan_metrics: planned.scan_metrics,
            })
        }

        LogicalPlan::Project { input, columns } => {
            let planned = plan_query_inner(input, catalog, partition)?;

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
            let planned = plan_query_inner(input, catalog, partition)?;

            let group_cols: Vec<String> = group_by
                .iter()
                .map(|e| match e {
                    Expr::Column(name) => resolve_column_name(planned.operator.schema(), name),
                    _ => Err(KaveonError::Execution(
                        "only column references supported in GROUP BY".into(),
                    )),
                })
                .collect::<Result<_>>()?;

            let agg_exprs: Vec<AggExpr> = aggregates
                .iter()
                .map(|aggregate| logical_agg_to_exec(aggregate, planned.operator.schema()))
                .collect::<Result<_>>()?;

            Ok(PlannedQuery {
                operator: Box::new(HashAggregate::new(planned.operator, group_cols, agg_exprs)?),
                scan_metrics: planned.scan_metrics,
            })
        }

        LogicalPlan::Sort { input, order_by } => {
            let planned = plan_query_inner(input, catalog, partition)?;
            let sort_exprs = order_by
                .iter()
                .map(|(expr, ascending)| SortExpr::new(expr.clone(), *ascending))
                .collect();
            Ok(PlannedQuery {
                operator: Box::new(SortOperator::new(planned.operator, sort_exprs)?),
                scan_metrics: planned.scan_metrics,
            })
        }

        LogicalPlan::Limit { input, count } => {
            if let LogicalPlan::Sort {
                input: sort_input,
                order_by,
            } = input.as_ref()
            {
                let planned = plan_query_inner(sort_input, catalog, partition)?;
                let sort_exprs = order_by
                    .iter()
                    .map(|(expr, ascending)| SortExpr::new(expr.clone(), *ascending))
                    .collect();
                return Ok(PlannedQuery {
                    operator: Box::new(TopNOperator::new(planned.operator, sort_exprs, *count)?),
                    scan_metrics: planned.scan_metrics,
                });
            }
            let planned = plan_query_inner(input, catalog, partition)?;
            Ok(PlannedQuery {
                operator: Box::new(LimitOperator::new(planned.operator, *count)),
                scan_metrics: planned.scan_metrics,
            })
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

#[cfg(test)]
mod tests {
    use std::fs::{self, File};
    use std::sync::Arc;

    use arrow::array::Int64Array;
    use arrow::datatypes::{DataType, Field, Schema};
    use arrow::record_batch::RecordBatch;
    use kaveon_core::{
        AccessPattern, CatalogProvider, DataFormat, MemoryCatalog, StorageType, TableMeta,
        collect_batches,
    };
    use parquet::arrow::ArrowWriter;
    use parquet::file::properties::WriterProperties;

    use super::*;

    struct Fixture {
        directory: std::path::PathBuf,
        catalog: CatalogManager,
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.directory);
        }
    }

    fn fixture() -> Fixture {
        let directory =
            std::env::temp_dir().join(format!("kaveon-server-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&directory).unwrap();
        let schema = Arc::new(Schema::new(vec![Field::new("id", DataType::Int64, false)]));
        let batch = RecordBatch::try_new(
            Arc::clone(&schema),
            vec![Arc::new(Int64Array::from(vec![1, 2, 100, 101]))],
        )
        .unwrap();
        let properties = WriterProperties::builder()
            .set_max_row_group_size(2)
            .build();
        let mut writer = ArrowWriter::try_new(
            File::create(directory.join("items.parquet")).unwrap(),
            Arc::clone(&schema),
            Some(properties),
        )
        .unwrap();
        writer.write(&batch).unwrap();
        writer.close().unwrap();

        let mut memory = MemoryCatalog::new(
            "test",
            StorageType::Local {
                base_path: directory.clone(),
            },
        )
        .with_schema("default");
        memory
            .register_table(
                "default",
                TableMeta {
                    name: "items".into(),
                    arrow_schema: schema,
                    location: "items.parquet".into(),
                    access: AccessPattern::Shortcut,
                    format: DataFormat::Parquet,
                },
            )
            .unwrap();
        let mut catalog = CatalogManager::new("test", "default");
        catalog.register_catalog(Box::new(memory));
        Fixture { directory, catalog }
    }

    #[test]
    fn plans_order_by_limit_as_top_n() {
        let fixture = fixture();
        let mut plan = kaveon_sql::logical_plan::sql_to_logical_plan(
            "SELECT id FROM items ORDER BY id DESC LIMIT 2",
        )
        .unwrap();
        qualify_tables(&mut plan, "test", "default");
        let mut planned = plan_query(&plan, &fixture.catalog).unwrap();
        let batches = collect_batches(&mut *planned.operator).unwrap();
        let values = batches[0]
            .column(0)
            .as_any()
            .downcast_ref::<Int64Array>()
            .unwrap();
        assert_eq!(values.values(), &[101, 100]);
    }

    #[test]
    fn pushes_filter_to_parquet_and_retains_row_filter() {
        let fixture = fixture();
        let mut plan =
            kaveon_sql::logical_plan::sql_to_logical_plan("SELECT id FROM items WHERE id > 50")
                .unwrap();
        qualify_tables(&mut plan, "test", "default");
        let plan = kaveon_optim::rules::push_filter_down(plan);
        let mut planned = plan_query(&plan, &fixture.catalog).unwrap();
        let metrics = planned.scan_metrics[0].clone();
        let batches = collect_batches(&mut *planned.operator).unwrap();
        assert_eq!(batches.iter().map(RecordBatch::num_rows).sum::<usize>(), 2);
        assert_eq!(metrics.snapshot().row_groups_pruned(), 1);
    }

    #[test]
    fn qualifies_only_unqualified_table_references() {
        let mut plan = LogicalPlan::Scan {
            table: "items".into(),
            alias: None,
            columns: None,
        };
        qualify_tables(&mut plan, "test", "default");
        assert!(matches!(
            plan,
            LogicalPlan::Scan { ref table, .. } if table == "test.default.items"
        ));
    }
}
