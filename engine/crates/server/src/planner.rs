use kaveon_core::{
    AggregateFunction, AggregateMode, AggregateSpec, BatchOperator, BatchSource, CatalogManager,
    DataFormat, EXECUTABLE_FRAGMENT_VERSION, ExchangeDescriptor, ExchangeId, ExchangeInput,
    ExchangeOutput, ExecutableFragment, Expr, FragmentNode, FragmentNodeId, FragmentOperator,
    JoinSpec, JoinType as FragmentJoinType, KaveonError, NamedExpr, Partitioning, Result, ScanSpec,
    ScanTable, SortSpec, StageFragment, StageGraph, StageId, TableReference,
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

const GROUPED_AGGREGATE_STATE_KEY_COLUMN: &str = "group_keys";

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

pub fn build_stage_graph(
    query_id: impl Into<String>,
    plan: &LogicalPlan,
    worker_count: usize,
) -> Result<StageGraph> {
    if worker_count == 0 {
        return Err(KaveonError::Execution(
            "stage planning requires at least one worker".into(),
        ));
    }
    let query_id = query_id.into();
    let mut builder = StageGraphBuilder {
        worker_count,
        stages: Vec::new(),
        exchanges: Vec::new(),
    };
    let root_stage = builder.build(plan)?;
    let graph = StageGraph {
        query_id,
        root_stage,
        stages: builder.stages,
        exchanges: builder.exchanges,
    };
    graph.validate()?;
    Ok(graph)
}

pub fn build_executable_fragments(
    query_id: impl Into<String>,
    plan: &LogicalPlan,
    catalog: &CatalogManager,
    worker_count: usize,
) -> Result<BTreeMap<StageId, ExecutableFragment>> {
    let graph = build_stage_graph(query_id, plan, worker_count)?;
    let mut builder = ExecutableFragmentBuilder {
        graph: &graph,
        catalog,
        fragments: BTreeMap::new(),
        next_stage: 0,
    };
    let root = builder.build(plan)?;
    if root != graph.root_stage || builder.fragments.len() != graph.stages.len() {
        return Err(KaveonError::Execution(
            "executable fragments diverged from the stage graph".into(),
        ));
    }
    for fragment in builder.fragments.values() {
        fragment.validate()?;
    }
    Ok(builder.fragments)
}

struct FragmentDraft {
    nodes: Vec<FragmentNode>,
    root: FragmentNodeId,
}

impl FragmentDraft {
    fn leaf(operator: FragmentOperator) -> Self {
        Self {
            nodes: vec![FragmentNode {
                id: FragmentNodeId(0),
                inputs: Vec::new(),
                operator,
            }],
            root: FragmentNodeId(0),
        }
    }

    fn push(&mut self, operator: FragmentOperator, inputs: Vec<FragmentNodeId>) {
        let id = FragmentNodeId(self.nodes.len() as u32);
        self.nodes.push(FragmentNode {
            id,
            inputs,
            operator,
        });
        self.root = id;
    }
}

struct ExecutableFragmentBuilder<'a> {
    graph: &'a StageGraph,
    catalog: &'a CatalogManager,
    fragments: BTreeMap<StageId, ExecutableFragment>,
    next_stage: u32,
}

impl ExecutableFragmentBuilder<'_> {
    fn build(&mut self, plan: &LogicalPlan) -> Result<StageId> {
        match plan {
            LogicalPlan::Scan { table, columns, .. } => {
                let reference = TableReference::parse(table);
                let resolved = self.catalog.resolve_table(&reference)?;
                let scan = ScanSpec {
                    source_uri: resolved.full_path(),
                    format: resolved.table.format,
                    table: ScanTable {
                        catalog: resolved.catalog,
                        schema: resolved.schema,
                        table: resolved.table.name.clone(),
                    },
                    projection: columns.clone().unwrap_or_default(),
                    predicate: None,
                };
                Ok(self.add_fragment(FragmentDraft::leaf(FragmentOperator::Scan(scan))))
            }
            LogicalPlan::Filter { input, predicate } => {
                let stage = self.build(input)?;
                let mut draft = self.draft_mut(stage)?;
                if let Some(storage_predicate) = to_storage_predicate(predicate)
                    && let Some(FragmentNode {
                        operator: FragmentOperator::Scan(scan),
                        ..
                    }) = draft.nodes.first_mut()
                {
                    scan.predicate = Some(storage_predicate);
                }
                draft.push(
                    FragmentOperator::Filter {
                        predicate: predicate.clone(),
                    },
                    vec![draft.root],
                );
                Ok(stage)
            }
            LogicalPlan::Project { input, columns } => {
                let stage = self.build(input)?;
                let expressions = fragment_project_expressions(columns);
                let mut draft = self.draft_mut(stage)?;
                draft.push(FragmentOperator::Project { expressions }, vec![draft.root]);
                Ok(stage)
            }
            LogicalPlan::Aggregate {
                input,
                group_by,
                aggregates,
            } => {
                let source = self.build(input)?;
                let groups = named_expressions(group_by);
                let aggregates = aggregate_specs(aggregates);
                let target = StageId(self.next_stage);
                let exchange = self.exchange(source, target)?.clone();
                let mut source_draft = self.draft_mut(source)?;
                source_draft.push(
                    FragmentOperator::Aggregate {
                        mode: AggregateMode::Partial,
                        group_by: groups.clone(),
                        aggregates: aggregates.clone(),
                    },
                    vec![source_draft.root],
                );
                source_draft.push(
                    FragmentOperator::ExchangeOutput(ExchangeOutput {
                        exchange_id: exchange.id.clone(),
                        partitioning: match exchange.partitioning {
                            Partitioning::Hash {
                                partition_count, ..
                            } => Partitioning::Hash {
                                columns: vec![GROUPED_AGGREGATE_STATE_KEY_COLUMN.to_owned()],
                                partition_count,
                            },
                            partitioning => partitioning,
                        },
                    }),
                    vec![source_draft.root],
                );
                let mut target_draft =
                    FragmentDraft::leaf(FragmentOperator::ExchangeInput(ExchangeInput {
                        exchange_id: exchange.id,
                    }));
                target_draft.push(
                    FragmentOperator::Aggregate {
                        mode: AggregateMode::Final,
                        group_by: groups,
                        aggregates,
                    },
                    vec![target_draft.root],
                );
                Ok(self.add_fragment(target_draft))
            }
            LogicalPlan::Limit { input, count }
                if matches!(input.as_ref(), LogicalPlan::Sort { .. }) =>
            {
                let LogicalPlan::Sort {
                    input: sort_input,
                    order_by,
                } = input.as_ref()
                else {
                    unreachable!()
                };
                let source = self.build(sort_input)?;
                let keys = sort_specs(order_by);
                let target = StageId(self.next_stage);
                let exchange = self.exchange(source, target)?.clone();
                let mut draft = self.draft_mut(source)?;
                draft.push(
                    FragmentOperator::TopN {
                        keys: keys.clone(),
                        limit: *count,
                    },
                    vec![draft.root],
                );
                draft.push(
                    FragmentOperator::ExchangeOutput(ExchangeOutput {
                        exchange_id: exchange.id.clone(),
                        partitioning: exchange.partitioning,
                    }),
                    vec![draft.root],
                );
                let mut target_draft =
                    FragmentDraft::leaf(FragmentOperator::ExchangeInput(ExchangeInput {
                        exchange_id: exchange.id,
                    }));
                target_draft.push(
                    FragmentOperator::TopN {
                        keys,
                        limit: *count,
                    },
                    vec![target_draft.root],
                );
                Ok(self.add_fragment(target_draft))
            }
            LogicalPlan::Sort { input, order_by } => self.build_single_exchange(
                input,
                FragmentOperator::Sort {
                    keys: sort_specs(order_by),
                },
                None,
            ),
            LogicalPlan::Limit { input, count } => self.build_single_exchange(
                input,
                FragmentOperator::Limit { limit: *count },
                Some(FragmentOperator::Limit { limit: *count }),
            ),
            LogicalPlan::Join {
                left,
                right,
                join_type,
                condition,
            } => self.build_join(left, right, *join_type, condition.as_ref()),
        }
    }

    fn build_single_exchange(
        &mut self,
        input: &LogicalPlan,
        operator: FragmentOperator,
        partial_operator: Option<FragmentOperator>,
    ) -> Result<StageId> {
        let source = self.build(input)?;
        let target = StageId(self.next_stage);
        let exchange = self.exchange(source, target)?.clone();
        let mut source_draft = self.draft_mut(source)?;
        if let Some(partial_operator) = partial_operator {
            source_draft.push(partial_operator, vec![source_draft.root]);
        }
        source_draft.push(
            FragmentOperator::ExchangeOutput(ExchangeOutput {
                exchange_id: exchange.id.clone(),
                partitioning: exchange.partitioning,
            }),
            vec![source_draft.root],
        );
        let mut target_draft =
            FragmentDraft::leaf(FragmentOperator::ExchangeInput(ExchangeInput {
                exchange_id: exchange.id,
            }));
        target_draft.push(operator, vec![target_draft.root]);
        Ok(self.add_fragment(target_draft))
    }

    fn build_join(
        &mut self,
        left: &LogicalPlan,
        right: &LogicalPlan,
        join_type: JoinType,
        condition: Option<&Expr>,
    ) -> Result<StageId> {
        let left_stage = self.build(left)?;
        let right_stage = self.build(right)?;
        let target = StageId(self.next_stage);
        let left_exchange = self.exchange(left_stage, target)?.clone();
        let right_exchange = self.exchange(right_stage, target)?.clone();
        for (stage, exchange) in [
            (left_stage, left_exchange.clone()),
            (right_stage, right_exchange.clone()),
        ] {
            let mut draft = self.draft_mut(stage)?;
            draft.push(
                FragmentOperator::ExchangeOutput(ExchangeOutput {
                    exchange_id: exchange.id,
                    partitioning: exchange.partitioning,
                }),
                vec![draft.root],
            );
        }
        let mut target_draft =
            FragmentDraft::leaf(FragmentOperator::ExchangeInput(ExchangeInput {
                exchange_id: left_exchange.id,
            }));
        let right_input = FragmentNodeId(1);
        target_draft.nodes.push(FragmentNode {
            id: right_input,
            inputs: Vec::new(),
            operator: FragmentOperator::ExchangeInput(ExchangeInput {
                exchange_id: right_exchange.id,
            }),
        });
        let keys = join_keys(condition)?;
        let (left_keys, right_keys): (Vec<_>, Vec<_>) = keys.into_iter().unzip();
        target_draft.push(
            FragmentOperator::HashJoin(JoinSpec {
                join_type: fragment_join_type(join_type),
                left_keys: left_keys.into_iter().map(Expr::Column).collect(),
                right_keys: right_keys.into_iter().map(Expr::Column).collect(),
                residual: None,
                broadcast: join_type == JoinType::Cross,
            }),
            vec![FragmentNodeId(0), right_input],
        );
        Ok(self.add_fragment(target_draft))
    }

    fn add_fragment(&mut self, draft: FragmentDraft) -> StageId {
        let stage_id = StageId(self.next_stage);
        self.next_stage += 1;
        self.fragments.insert(
            stage_id,
            ExecutableFragment {
                version: EXECUTABLE_FRAGMENT_VERSION,
                stage_id,
                root: draft.root,
                nodes: draft.nodes,
            },
        );
        stage_id
    }

    fn draft_mut(&mut self, stage: StageId) -> Result<FragmentDraftGuard<'_>> {
        let fragment = self.fragments.get_mut(&stage).ok_or_else(|| {
            KaveonError::Execution(format!("missing executable fragment for stage {}", stage.0))
        })?;
        Ok(FragmentDraftGuard { fragment })
    }

    fn exchange(&self, source: StageId, target: StageId) -> Result<&ExchangeDescriptor> {
        self.graph
            .exchanges
            .iter()
            .find(|exchange| exchange.source_stage == source && exchange.target_stage == target)
            .ok_or_else(|| {
                KaveonError::Execution(format!(
                    "stage graph has no exchange from {} to {}",
                    source.0, target.0
                ))
            })
    }
}

struct FragmentDraftGuard<'a> {
    fragment: &'a mut ExecutableFragment,
}

impl std::ops::Deref for FragmentDraftGuard<'_> {
    type Target = ExecutableFragment;

    fn deref(&self) -> &Self::Target {
        self.fragment
    }
}

impl std::ops::DerefMut for FragmentDraftGuard<'_> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.fragment
    }
}

impl FragmentDraftGuard<'_> {
    fn push(&mut self, operator: FragmentOperator, inputs: Vec<FragmentNodeId>) {
        let id = FragmentNodeId(self.fragment.nodes.len() as u32);
        self.fragment.nodes.push(FragmentNode {
            id,
            inputs,
            operator,
        });
        self.fragment.root = id;
    }
}

fn named_expressions(expressions: &[Expr]) -> Vec<NamedExpr> {
    expressions
        .iter()
        .enumerate()
        .map(|(index, expression)| NamedExpr {
            name: match expression {
                Expr::Alias { name, .. } => name.clone(),
                Expr::Column(column) => unqualify(column),
                _ => format!("expr_{index}"),
            },
            expression: expression.clone(),
        })
        .collect()
}

fn fragment_project_expressions(expressions: &[Expr]) -> Vec<NamedExpr> {
    named_expressions(expressions)
        .into_iter()
        .map(|mut named| {
            named.expression = match named.expression {
                Expr::Function { name, args } => {
                    Expr::Column(fragment_aggregate_output_name(&name, &args))
                }
                Expr::Alias { expr, name } => match *expr {
                    Expr::Function {
                        name: function,
                        args,
                    } => Expr::Alias {
                        expr: Box::new(Expr::Column(fragment_aggregate_output_name(
                            &function, &args,
                        ))),
                        name,
                    },
                    expression => Expr::Alias {
                        expr: Box::new(expression),
                        name,
                    },
                },
                expression => expression,
            };
            named
        })
        .collect()
}

fn fragment_aggregate_output_name(function: &str, arguments: &[Expr]) -> String {
    let suffix = arguments.first().map_or("star".to_owned(), |argument| {
        if matches!(argument, Expr::Star) {
            "star".to_owned()
        } else {
            expression_column(argument).unwrap_or_else(|_| "expr".to_owned())
        }
    });
    format!("{}_{}", function.to_ascii_lowercase(), suffix)
}

fn aggregate_specs(aggregates: &[AggregateExpr]) -> Vec<AggregateSpec> {
    aggregates
        .iter()
        .map(|aggregate| {
            let (function, argument, name) = match aggregate {
                AggregateExpr::Count { expr, distinct } => (
                    if *distinct {
                        AggregateFunction::CountDistinct
                    } else {
                        AggregateFunction::Count
                    },
                    (!matches!(expr, Expr::Star)).then(|| expr.clone()),
                    "count",
                ),
                AggregateExpr::Sum(expr) => (AggregateFunction::Sum, Some(expr.clone()), "sum"),
                AggregateExpr::Min(expr) => (AggregateFunction::Min, Some(expr.clone()), "min"),
                AggregateExpr::Max(expr) => (AggregateFunction::Max, Some(expr.clone()), "max"),
                AggregateExpr::Avg(expr) => (AggregateFunction::Avg, Some(expr.clone()), "avg"),
            };
            let suffix = argument.as_ref().map_or("star".into(), |expression| {
                expression_column(expression).unwrap_or_else(|_| "expr".into())
            });
            AggregateSpec {
                function,
                argument,
                output: format!("{name}_{suffix}"),
            }
        })
        .collect()
}

fn sort_specs(order_by: &[(Expr, bool)]) -> Vec<SortSpec> {
    order_by
        .iter()
        .map(|(expression, ascending)| SortSpec {
            expression: expression.clone(),
            ascending: *ascending,
            nulls_first: !ascending,
        })
        .collect()
}

fn fragment_join_type(join_type: JoinType) -> FragmentJoinType {
    match join_type {
        JoinType::Inner => FragmentJoinType::Inner,
        JoinType::Left => FragmentJoinType::Left,
        JoinType::Right => FragmentJoinType::Right,
        JoinType::Full => FragmentJoinType::Full,
        JoinType::Cross => FragmentJoinType::Cross,
    }
}

struct StageGraphBuilder {
    worker_count: usize,
    stages: Vec<StageFragment>,
    exchanges: Vec<ExchangeDescriptor>,
}

impl StageGraphBuilder {
    fn build(&mut self, plan: &LogicalPlan) -> Result<StageId> {
        match plan {
            LogicalPlan::Aggregate {
                input, group_by, ..
            } => {
                let source = self.build(input)?;
                let aggregate = physical_plan_tree(plan);
                self.wrap_stage(source, "PartialAggregate", aggregate.attributes.clone());
                let keys = group_by
                    .iter()
                    .map(expression_column)
                    .collect::<Result<Vec<_>>>()?;
                let partitioning = if keys.is_empty() {
                    Partitioning::Single
                } else {
                    Partitioning::Hash {
                        columns: keys,
                        partition_count: self.worker_count,
                    }
                };
                let task_count = match &partitioning {
                    Partitioning::Single => 1,
                    _ => self.worker_count,
                };
                let target =
                    self.add_exchange_stage("FinalAggregate", aggregate.attributes, task_count, 1);
                self.add_exchange(source, target, partitioning);
                Ok(target)
            }
            LogicalPlan::Limit { input, .. }
                if matches!(input.as_ref(), LogicalPlan::Sort { .. }) =>
            {
                let LogicalPlan::Sort {
                    input: sort_input, ..
                } = input.as_ref()
                else {
                    unreachable!()
                };
                let source = self.build(sort_input)?;
                let top_n = physical_plan_tree(plan);
                self.wrap_stage(source, "PartialTopN", top_n.attributes.clone());
                let target = self.add_exchange_stage("FinalTopN", top_n.attributes, 1, 1);
                self.add_exchange(source, target, Partitioning::Single);
                Ok(target)
            }
            LogicalPlan::Sort { input, .. } | LogicalPlan::Limit { input, .. } => {
                let source = self.build(input)?;
                let physical = physical_plan_tree(plan);
                let operator = if matches!(plan, LogicalPlan::Sort { .. }) {
                    "FinalSort"
                } else {
                    self.wrap_stage(source, "PartialLimit", physical.attributes.clone());
                    "FinalLimit"
                };
                let target = self.add_exchange_stage(operator, physical.attributes, 1, 1);
                self.add_exchange(source, target, Partitioning::Single);
                Ok(target)
            }
            LogicalPlan::Join {
                left,
                right,
                join_type,
                condition,
            } => {
                let left_stage = self.build(left)?;
                let right_stage = self.build(right)?;
                let join = physical_plan_tree(plan);
                let target = self.add_exchange_stage(
                    "PartitionedHashJoin",
                    join.attributes,
                    self.worker_count,
                    2,
                );
                if *join_type == JoinType::Cross {
                    self.add_exchange(
                        left_stage,
                        target,
                        Partitioning::RoundRobin {
                            partition_count: self.worker_count,
                        },
                    );
                    self.add_exchange(right_stage, target, Partitioning::Broadcast);
                } else {
                    let keys = join_keys(condition.as_ref())?;
                    if keys.is_empty() {
                        return Err(KaveonError::Execution(
                            "distributed non-cross joins require equality keys".into(),
                        ));
                    }
                    let (left_keys, right_keys): (Vec<_>, Vec<_>) = keys.into_iter().unzip();
                    self.add_exchange(
                        left_stage,
                        target,
                        Partitioning::Hash {
                            columns: left_keys,
                            partition_count: self.worker_count,
                        },
                    );
                    self.add_exchange(
                        right_stage,
                        target,
                        Partitioning::Hash {
                            columns: right_keys,
                            partition_count: self.worker_count,
                        },
                    );
                }
                Ok(target)
            }
            LogicalPlan::Filter { input, .. } | LogicalPlan::Project { input, .. } => {
                let stage = self.build(input)?;
                let physical = physical_plan_tree(plan);
                let operator = physical.operator;
                self.wrap_stage(stage, &operator, physical.attributes);
                Ok(stage)
            }
            LogicalPlan::Scan { .. } => Ok(self.add_stage(plan, self.worker_count)),
        }
    }

    fn add_stage(&mut self, plan: &LogicalPlan, task_count: usize) -> StageId {
        let id = StageId(self.stages.len() as u32);
        self.stages.push(StageFragment {
            id,
            task_count,
            plan: physical_plan_tree(plan),
        });
        id
    }

    fn wrap_stage(&mut self, stage: StageId, operator: &str, attributes: BTreeMap<String, String>) {
        if let Some(fragment) = self.stages.iter_mut().find(|fragment| fragment.id == stage) {
            let child = std::mem::replace(
                &mut fragment.plan,
                kaveon_core::PlanNode::new(0, kaveon_core::PlanPhase::Physical, operator),
            );
            fragment.plan.attributes = attributes;
            fragment.plan.children.push(child);
        }
    }

    fn add_exchange_stage(
        &mut self,
        operator: &str,
        attributes: BTreeMap<String, String>,
        task_count: usize,
        input_count: usize,
    ) -> StageId {
        let id = StageId(self.stages.len() as u32);
        let mut plan = kaveon_core::PlanNode::new(0, kaveon_core::PlanPhase::Physical, operator);
        plan.attributes = attributes;
        for input in 0..input_count {
            let mut exchange = kaveon_core::PlanNode::new(
                input as u32 + 1,
                kaveon_core::PlanPhase::Physical,
                "ExchangeInput",
            );
            exchange
                .attributes
                .insert("input".into(), input.to_string());
            plan.children.push(exchange);
        }
        self.stages.push(StageFragment {
            id,
            task_count,
            plan,
        });
        id
    }

    fn add_exchange(&mut self, source: StageId, target: StageId, partitioning: Partitioning) {
        let ordinal = self.exchanges.len();
        self.exchanges.push(ExchangeDescriptor {
            id: ExchangeId(format!("exchange-{}-{}-{ordinal}", source.0, target.0)),
            source_stage: source,
            target_stage: target,
            partitioning,
        });
    }
}

fn expression_column(expression: &Expr) -> Result<String> {
    match expression {
        Expr::Column(column) => Ok(unqualify(column)),
        _ => Err(KaveonError::Execution(
            "distributed partition keys must be column references".into(),
        )),
    }
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
        memory
            .register_table(
                "default",
                TableMeta {
                    name: "customers".into(),
                    arrow_schema: Arc::new(Schema::new(vec![Field::new(
                        "id",
                        DataType::Int64,
                        false,
                    )])),
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

    fn graph(sql: &str) -> StageGraph {
        let plan = kaveon_sql::logical_plan::sql_to_logical_plan(sql).unwrap();
        build_stage_graph("query-1", &plan, 4).unwrap()
    }

    fn executable_fragments(sql: &str) -> BTreeMap<StageId, ExecutableFragment> {
        let fixture = fixture();
        let mut plan = kaveon_sql::logical_plan::sql_to_logical_plan(sql).unwrap();
        qualify_tables(&mut plan, "test", "default");
        build_executable_fragments("query-1", &plan, &fixture.catalog, 4).unwrap()
    }

    #[test]
    fn translates_grouped_aggregate_into_partial_and_final_fragments() {
        let fragments = executable_fragments("SELECT id, AVG(id) FROM items GROUP BY id");
        assert_eq!(fragments.len(), 2);
        let partial = &fragments[&StageId(0)];
        assert!(partial.nodes.iter().any(|node| matches!(
            node.operator,
            FragmentOperator::Aggregate {
                mode: AggregateMode::Partial,
                ..
            }
        )));
        let output = partial.nodes.last().unwrap();
        let FragmentOperator::ExchangeOutput(output) = &output.operator else {
            panic!("partial aggregate must terminate in exchange output");
        };
        assert!(matches!(
            &output.partitioning,
            Partitioning::Hash { columns, .. }
                if columns == &[GROUPED_AGGREGATE_STATE_KEY_COLUMN]
        ));
        let final_fragment = &fragments[&StageId(1)];
        assert!(final_fragment.nodes.iter().any(|node| matches!(
            node.operator,
            FragmentOperator::Aggregate {
                mode: AggregateMode::Final,
                ..
            }
        )));
        assert!(matches!(
            &final_fragment.nodes[0].operator,
            FragmentOperator::ExchangeInput(input) if input.exchange_id == output.exchange_id
        ));
        assert!(matches!(
            &final_fragment.nodes.last().unwrap().operator,
            FragmentOperator::Project { expressions }
                if expressions[1].expression == Expr::Column("avg_id".into())
        ));
    }

    #[test]
    fn translates_top_n_with_matching_single_exchange() {
        let fragments = executable_fragments("SELECT id FROM items ORDER BY id DESC LIMIT 3");
        let partial = &fragments[&StageId(0)];
        assert!(
            partial
                .nodes
                .iter()
                .any(|node| matches!(node.operator, FragmentOperator::TopN { limit: 3, .. }))
        );
        assert!(matches!(
            fragments[&StageId(1)].nodes.last().unwrap().operator,
            FragmentOperator::TopN { limit: 3, .. }
        ));
    }

    #[test]
    fn translates_equi_and_cross_joins_with_graph_exchange_ids() {
        for (sql, expected_type, expected_broadcast) in [
            (
                "SELECT * FROM items i JOIN customers c ON i.id = c.id",
                FragmentJoinType::Inner,
                false,
            ),
            (
                "SELECT * FROM items CROSS JOIN customers",
                FragmentJoinType::Cross,
                true,
            ),
        ] {
            let fragments = executable_fragments(sql);
            let join = fragments[&StageId(2)].nodes.last().unwrap();
            assert!(matches!(
                &join.operator,
                FragmentOperator::HashJoin(spec)
                    if spec.join_type == expected_type && spec.broadcast == expected_broadcast
            ));
            assert_eq!(join.inputs, vec![FragmentNodeId(0), FragmentNodeId(1)]);
            for stage in [StageId(0), StageId(1)] {
                assert!(matches!(
                    fragments[&stage].nodes.last().unwrap().operator,
                    FragmentOperator::ExchangeOutput(_)
                ));
            }
        }
    }

    #[test]
    fn plans_grouped_aggregate_with_hash_exchange() {
        let graph = graph("SELECT region, SUM(total) FROM orders GROUP BY region");

        assert_eq!(graph.stages.len(), 2);
        assert_eq!(graph.exchanges.len(), 1);
        assert_eq!(graph.stages[0].plan.operator, "PartialAggregate");
        let root = &graph.stages[graph.root_stage.0 as usize];
        assert_eq!(root.plan.operator, "Project");
        assert_eq!(root.plan.children[0].operator, "FinalAggregate");
        assert_eq!(root.task_count, 4);
        assert_eq!(
            graph.exchanges[0].partitioning,
            Partitioning::Hash {
                columns: vec!["region".into()],
                partition_count: 4,
            }
        );
    }

    #[test]
    fn plans_global_aggregate_with_single_exchange() {
        let graph = graph("SELECT COUNT(*) FROM orders");

        assert_eq!(graph.stages.len(), 2);
        assert_eq!(graph.stages[graph.root_stage.0 as usize].task_count, 1);
        assert_eq!(graph.exchanges[0].partitioning, Partitioning::Single);
    }

    #[test]
    fn plans_sort_and_top_n_as_single_final_stages() {
        for sql in [
            "SELECT id FROM orders ORDER BY id DESC",
            "SELECT id FROM orders ORDER BY id DESC LIMIT 10",
        ] {
            let graph = graph(sql);
            assert_eq!(graph.stages.len(), 2);
            assert_eq!(graph.exchanges.len(), 1);
            assert_eq!(graph.exchanges[0].partitioning, Partitioning::Single);
            assert_eq!(graph.stages[graph.root_stage.0 as usize].task_count, 1);
            assert!(
                graph.stages[graph.root_stage.0 as usize]
                    .plan
                    .operator
                    .starts_with("Final")
            );
        }
    }

    #[test]
    fn plans_equi_join_with_colocated_hash_exchanges() {
        let graph =
            graph("SELECT * FROM orders o JOIN customers c ON o.customer_id = c.customer_id");

        assert_eq!(graph.stages.len(), 3);
        assert_eq!(graph.exchanges.len(), 2);
        assert_eq!(
            graph.stages[graph.root_stage.0 as usize].plan.operator,
            "PartitionedHashJoin"
        );
        assert_eq!(
            graph.exchanges[0].partitioning,
            Partitioning::Hash {
                columns: vec!["customer_id".into()],
                partition_count: 4,
            }
        );
        assert_eq!(
            graph.exchanges[1].partitioning,
            Partitioning::Hash {
                columns: vec!["customer_id".into()],
                partition_count: 4,
            }
        );
    }

    #[test]
    fn plans_cross_join_with_broadcast_build_side() {
        let graph = graph("SELECT * FROM orders CROSS JOIN customers");

        assert_eq!(graph.stages.len(), 3);
        assert_eq!(
            graph.exchanges[0].partitioning,
            Partitioning::RoundRobin { partition_count: 4 }
        );
        assert_eq!(graph.exchanges[1].partitioning, Partitioning::Broadcast);
    }

    #[test]
    fn stage_planning_rejects_empty_worker_sets() {
        let plan = kaveon_sql::logical_plan::sql_to_logical_plan("SELECT * FROM orders").unwrap();
        let error = build_stage_graph("query-1", &plan, 0).unwrap_err();
        assert!(error.to_string().contains("at least one worker"));
    }
}
