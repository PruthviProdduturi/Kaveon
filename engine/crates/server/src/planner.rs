use kaveon_core::{
    AggregateFunction, AggregateMode, AggregateSpec, BatchOperator, BatchSource, CatalogManager,
    DataFormat, EXECUTABLE_FRAGMENT_VERSION, ExchangeDescriptor, ExchangeId, ExchangeInput,
    ExchangeOutput, ExecutableFragment, Expr, FragmentNode, FragmentNodeId, FragmentOperator,
    JoinSpec, JoinType as FragmentJoinType, KaveonError, NamedExpr, Partitioning, QueryMemoryPool,
    Result, ScanSpec, ScanTable, SortSpec, StageFragment, StageGraph, StageId, TableReference,
};
use kaveon_exec::aggregate::{AggExpr, AggFunc};
use kaveon_exec::filter::FilterOperator;
use kaveon_exec::join::JoinType as PhysicalJoinType;
use kaveon_exec::limit::LimitOperator;
use kaveon_exec::offset::OffsetOperator;
use kaveon_exec::project::ProjectOperator;
use kaveon_exec::scan::ScanOperator;
use kaveon_exec::semijoin::SemiJoinOperator;
use kaveon_exec::setop::{SetOpMode, SetOpOperator};
use kaveon_exec::sort::SortExpr;
use kaveon_exec::union::UnionOperator;
use kaveon_exec::window::WindowOperator;
use kaveon_optim::rules::to_storage_predicate;
use kaveon_sql::logical_plan::{AggregateExpr, JoinDistribution, JoinType, LogicalPlan};
const AGGREGATE_FUNCTIONS: &[&str] = &["COUNT", "SUM", "AVG", "MIN", "MAX"];
use kaveon_storage::{
    AdlsParquetReader, DeltaTableReader, ObjectDeltaReader, ObjectParquetReader, ParquetReader,
    ScanPartition,
};
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
    plan_query_inner(plan, catalog, None, None)
}

pub fn plan_query_with_memory(
    plan: &LogicalPlan,
    catalog: &CatalogManager,
    memory: &QueryMemoryPool,
) -> Result<PlannedQuery> {
    plan_query_inner(plan, catalog, None, Some(memory))
}

pub fn plan_partitioned_query(
    plan: &LogicalPlan,
    catalog: &CatalogManager,
    partition: ScanPartition,
) -> Result<PlannedQuery> {
    plan_query_inner(plan, catalog, Some(partition), None)
}

pub fn plan_partitioned_query_with_memory(
    plan: &LogicalPlan,
    catalog: &CatalogManager,
    partition: ScanPartition,
    memory: &QueryMemoryPool,
) -> Result<PlannedQuery> {
    plan_query_inner(plan, catalog, Some(partition), Some(memory))
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
        | LogicalPlan::Limit { input, .. }
        | LogicalPlan::Offset { input, .. }
        | LogicalPlan::Distinct { input, .. }
        | LogicalPlan::Window { input, .. } => qualify_tables(input, catalog, schema),
        LogicalPlan::Union { inputs, .. } => {
            for input in inputs.iter_mut() {
                qualify_tables(input, catalog, schema);
            }
        }
        LogicalPlan::Intersect { left, right }
        | LogicalPlan::Except { left, right }
        | LogicalPlan::SemiJoin { left, right, .. }
        | LogicalPlan::AntiJoin { left, right, .. } => {
            qualify_tables(left, catalog, schema);
            qualify_tables(right, catalog, schema);
        }
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
    build_executable_fragments_with_delta_versions(
        query_id,
        plan,
        catalog,
        worker_count,
        &BTreeMap::new(),
    )
}

pub fn build_executable_fragments_with_delta_versions(
    query_id: impl Into<String>,
    plan: &LogicalPlan,
    catalog: &CatalogManager,
    worker_count: usize,
    analyzed_delta_versions: &BTreeMap<String, u64>,
) -> Result<BTreeMap<StageId, ExecutableFragment>> {
    let graph = build_stage_graph(query_id, plan, worker_count)?;
    let mut builder = ExecutableFragmentBuilder {
        graph: &graph,
        catalog,
        fragments: BTreeMap::new(),
        next_stage: 0,
        // Keys are resolved source URIs from the same pinned catalog used by
        // this builder. A catalog replacement therefore cannot redirect a pin.
        delta_versions: analyzed_delta_versions.clone(),
        iceberg_snapshots: BTreeMap::new(),
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
    delta_versions: BTreeMap<String, u64>,
    iceberg_snapshots: BTreeMap<String, i64>,
}

impl ExecutableFragmentBuilder<'_> {
    fn build(&mut self, plan: &LogicalPlan) -> Result<StageId> {
        match plan {
            LogicalPlan::Scan { table, columns, .. } => {
                let reference = TableReference::parse(table);
                let resolved = self.catalog.resolve_table(&reference)?;
                let source_uri = resolved.full_path();
                let delta_version = if resolved.table.format == DataFormat::Delta {
                    if let Some(version) = self.delta_versions.get(&source_uri) {
                        Some(*version)
                    } else {
                        let version = if source_uri.starts_with("s3://")
                            || source_uri.starts_with("abfss://")
                        {
                            kaveon_storage::ObjectDeltaReader::from_uri(&source_uri)?
                                .snapshot()?
                                .version
                        } else {
                            kaveon_storage::DeltaTableReader::new(
                                source_uri.strip_prefix("file://").unwrap_or(&source_uri),
                            )
                            .snapshot_version()?
                        };
                        self.delta_versions.insert(source_uri.clone(), version);
                        Some(version)
                    }
                } else {
                    None
                };
                let scan = ScanSpec {
                    iceberg_snapshot_id: if resolved.table.format == DataFormat::Iceberg {
                        Some(if let Some(id) = self.iceberg_snapshots.get(&source_uri) {
                            *id
                        } else {
                            let id = kaveon_storage::IcebergReader::new(&source_uri)
                                .snapshot()?
                                .snapshot_id
                                .unwrap_or(-1);
                            self.iceberg_snapshots.insert(source_uri.clone(), id);
                            id
                        })
                    } else {
                        None
                    },
                    source_uri,
                    format: resolved.table.format,
                    delta_version,
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
                // HAVING: the filter sits on the aggregate's output, where
                // SUM(x) is a column, and nothing about it reaches the scan.
                let over_aggregate = matches!(input.as_ref(), LogicalPlan::Aggregate { .. });
                let predicate = if over_aggregate {
                    bind_aggregate_references(predicate.clone())
                } else {
                    predicate.clone()
                };
                let mut draft = self.draft_mut(stage)?;
                if !over_aggregate
                    && let Some(storage_predicate) = to_storage_predicate(&predicate)
                    && let Some(FragmentNode {
                        operator: FragmentOperator::Scan(scan),
                        ..
                    }) = draft.nodes.first_mut()
                {
                    scan.predicate = Some(storage_predicate);
                }
                draft.push(FragmentOperator::Filter { predicate }, vec![draft.root]);
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
            LogicalPlan::Limit { .. } if plan.top_n().is_some() => {
                // Every source partition keeps the rows the window can
                // reach — the skipped ones included — and the target
                // merges those and drops the skipped rows once.
                let top_n = plan.top_n().expect("matched above");
                let source = self.build(top_n.input)?;
                let keys = sort_specs(top_n.order_by);
                let limit = top_n.retained();
                let target = StageId(self.next_stage);
                let exchange = self.exchange(source, target)?.clone();
                let mut draft = self.draft_mut(source)?;
                draft.push(
                    FragmentOperator::TopN {
                        keys: keys.clone(),
                        limit,
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
                    FragmentOperator::TopN { keys, limit },
                    vec![target_draft.root],
                );
                if top_n.skip > 0 {
                    target_draft.push(
                        FragmentOperator::Offset { offset: top_n.skip },
                        vec![target_draft.root],
                    );
                }
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
            LogicalPlan::Offset { input, count } => {
                self.build_single_exchange(input, FragmentOperator::Offset { offset: *count }, None)
            }
            LogicalPlan::Distinct { input } => {
                // Each worker deduplicates its own partition before the
                // exchange; DISTINCT over a low-cardinality column then ships
                // a handful of rows instead of the whole column.
                self.build_single_exchange(
                    input,
                    FragmentOperator::Distinct,
                    Some(FragmentOperator::Distinct),
                )
            }
            LogicalPlan::Union { inputs, .. } => {
                let mut stages = Vec::new();
                for input in inputs {
                    stages.push(self.build(input)?);
                }
                let target = StageId(self.next_stage);
                let mut exchange_inputs = Vec::new();
                for stage in stages.iter() {
                    let exchange = self.exchange(*stage, target)?.clone();
                    let mut draft = self.draft_mut(*stage)?;
                    draft.push(
                        FragmentOperator::ExchangeOutput(ExchangeOutput {
                            exchange_id: exchange.id.clone(),
                            partitioning: exchange.partitioning,
                        }),
                        vec![draft.root],
                    );
                    exchange_inputs.push(exchange.id);
                }
                let mut target_draft =
                    FragmentDraft::leaf(FragmentOperator::ExchangeInput(ExchangeInput {
                        exchange_id: exchange_inputs[0].clone(),
                    }));
                let mut union_inputs = vec![target_draft.root];
                for exchange_id in &exchange_inputs[1..] {
                    let id = FragmentNodeId(target_draft.nodes.len() as u32);
                    target_draft.nodes.push(FragmentNode {
                        id,
                        inputs: Vec::new(),
                        operator: FragmentOperator::ExchangeInput(ExchangeInput {
                            exchange_id: exchange_id.clone(),
                        }),
                    });
                    union_inputs.push(id);
                }
                target_draft.push(FragmentOperator::Union, union_inputs);
                Ok(self.add_fragment(target_draft))
            }
            LogicalPlan::Window {
                input,
                window_exprs,
            } => self.build_single_exchange(
                input,
                FragmentOperator::Window {
                    window_exprs: window_exprs.clone(),
                },
                None,
            ),
            LogicalPlan::Intersect { left, right } => {
                let left_stage = self.build(left)?;
                let right_stage = self.build(right)?;
                let target = StageId(self.next_stage);
                let left_exchange = self.exchange(left_stage, target)?.clone();
                let right_exchange = self.exchange(right_stage, target)?.clone();
                for (stage, exchange) in [
                    (left_stage, left_exchange.clone()),
                    (right_stage, right_exchange.clone()),
                ] {
                    // Set semantics: each side deduplicates on its workers
                    // before the exchange, so the single final task receives
                    // distinct rows rather than every row of the input.
                    let mut draft = self.draft_mut(stage)?;
                    draft.push(FragmentOperator::Distinct, vec![draft.root]);
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
                target_draft.nodes.push(FragmentNode {
                    id: FragmentNodeId(1),
                    inputs: Vec::new(),
                    operator: FragmentOperator::ExchangeInput(ExchangeInput {
                        exchange_id: right_exchange.id,
                    }),
                });
                target_draft.push(
                    FragmentOperator::Intersect,
                    vec![FragmentNodeId(0), FragmentNodeId(1)],
                );
                Ok(self.add_fragment(target_draft))
            }
            LogicalPlan::Except { left, right } => {
                let left_stage = self.build(left)?;
                let right_stage = self.build(right)?;
                let target = StageId(self.next_stage);
                let left_exchange = self.exchange(left_stage, target)?.clone();
                let right_exchange = self.exchange(right_stage, target)?.clone();
                for (stage, exchange) in [
                    (left_stage, left_exchange.clone()),
                    (right_stage, right_exchange.clone()),
                ] {
                    // Set semantics: each side deduplicates on its workers
                    // before the exchange, so the single final task receives
                    // distinct rows rather than every row of the input.
                    let mut draft = self.draft_mut(stage)?;
                    draft.push(FragmentOperator::Distinct, vec![draft.root]);
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
                target_draft.nodes.push(FragmentNode {
                    id: FragmentNodeId(1),
                    inputs: Vec::new(),
                    operator: FragmentOperator::ExchangeInput(ExchangeInput {
                        exchange_id: right_exchange.id,
                    }),
                });
                target_draft.push(
                    FragmentOperator::Except,
                    vec![FragmentNodeId(0), FragmentNodeId(1)],
                );
                Ok(self.add_fragment(target_draft))
            }
            LogicalPlan::Join {
                left,
                right,
                join_type,
                condition,
                distribution,
            } => self.build_join(left, right, *join_type, condition.as_ref(), *distribution),
            LogicalPlan::SemiJoin {
                left,
                right,
                left_key,
                right_key,
            }
            | LogicalPlan::AntiJoin {
                left,
                right,
                left_key,
                right_key,
            } => {
                // The subquery side is one column of distinct-ish keys; it
                // broadcasts into every probe task like a small build side.
                let left_stage = self.build(left)?;
                let right_stage = self.build(right)?;
                let join_type = if matches!(plan, LogicalPlan::SemiJoin { .. }) {
                    FragmentJoinType::Semi
                } else {
                    FragmentJoinType::Anti
                };
                self.attach_broadcast_join(
                    left_stage,
                    right_stage,
                    JoinSpec {
                        join_type,
                        left_qualifier: None,
                        right_qualifier: None,
                        left_keys: vec![left_key.clone()],
                        right_keys: vec![right_key.clone()],
                        residual: None,
                        broadcast: true,
                    },
                )
            }
        }
    }

    /// Route `right_stage` into `left_stage` as a broadcast exchange and
    /// finish the left stage with `spec` joining its root to that input.
    fn attach_broadcast_join(
        &mut self,
        left_stage: StageId,
        right_stage: StageId,
        spec: JoinSpec,
    ) -> Result<StageId> {
        let right_exchange = self.exchange(right_stage, left_stage)?.clone();
        let right_root = self.draft_mut(right_stage)?.root;
        self.draft_mut(right_stage)?.push(
            FragmentOperator::ExchangeOutput(ExchangeOutput {
                exchange_id: right_exchange.id.clone(),
                partitioning: Partitioning::Broadcast,
            }),
            vec![right_root],
        );
        let mut draft = self.draft_mut(left_stage)?;
        let right_input = FragmentNodeId(draft.nodes.len() as u32);
        draft.nodes.push(FragmentNode {
            id: right_input,
            inputs: Vec::new(),
            operator: FragmentOperator::ExchangeInput(ExchangeInput {
                exchange_id: right_exchange.id,
            }),
        });
        draft.push(
            FragmentOperator::HashJoin(spec),
            vec![draft.root, right_input],
        );
        Ok(left_stage)
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
        distribution: JoinDistribution,
    ) -> Result<StageId> {
        let left_stage = self.build(left)?;
        let right_stage = self.build(right)?;
        if distribution == JoinDistribution::BroadcastRight {
            let keys = join_keys(condition)?;
            let (left_keys, right_keys): (Vec<_>, Vec<_>) = keys.into_iter().unzip();
            return self.attach_broadcast_join(
                left_stage,
                right_stage,
                JoinSpec {
                    join_type: fragment_join_type(join_type),
                    left_qualifier: relation_qualifier(left),
                    right_qualifier: relation_qualifier(right),
                    left_keys: left_keys.into_iter().map(Expr::Column).collect(),
                    right_keys: right_keys.into_iter().map(Expr::Column).collect(),
                    residual: None,
                    broadcast: true,
                },
            );
        }
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
                left_qualifier: relation_qualifier(left),
                right_qualifier: relation_qualifier(right),
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
    // Columns from two relations that share a name (`t.country`,
    // `u.country`) keep their qualifiers so both outputs stay addressable.
    let mut bare_names = BTreeMap::<String, usize>::new();
    for expression in expressions {
        if let Expr::Column(column) = expression {
            *bare_names.entry(unqualify(column)).or_default() += 1;
        }
    }
    expressions
        .iter()
        .enumerate()
        .map(|(index, expression)| NamedExpr {
            name: match expression {
                Expr::Alias { name, .. } => name.clone(),
                Expr::Column(column) => {
                    let bare = unqualify(column);
                    if bare_names[&bare] > 1 {
                        column.clone()
                    } else {
                        bare
                    }
                }
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
            named.expression = bind_aggregate_references(named.expression);
            named
        })
        .collect()
}

/// Above an aggregate, every aggregate call — at the top of an expression,
/// under an alias, or nested in arithmetic, a CASE or a HAVING comparison —
/// is the aggregate's output column, not a function to evaluate.
fn bind_aggregate_references(expression: Expr) -> Expr {
    let bind = |expr: Box<Expr>| Box::new(bind_aggregate_references(*expr));
    match expression {
        Expr::Function { name, args } if AGGREGATE_FUNCTIONS.contains(&name.as_str()) => {
            Expr::Column(fragment_aggregate_output_name(&name, &args))
        }
        Expr::Function { name, args } => Expr::Function {
            name,
            args: args.into_iter().map(bind_aggregate_references).collect(),
        },
        Expr::Alias { expr, name } => Expr::Alias {
            expr: bind(expr),
            name,
        },
        Expr::BinaryOp { left, op, right } => Expr::BinaryOp {
            left: bind(left),
            op,
            right: bind(right),
        },
        Expr::IsNull(expr) => Expr::IsNull(bind(expr)),
        Expr::IsNotNull(expr) => Expr::IsNotNull(bind(expr)),
        Expr::Not(expr) => Expr::Not(bind(expr)),
        Expr::And(left, right) => Expr::And(bind(left), bind(right)),
        Expr::Or(left, right) => Expr::Or(bind(left), bind(right)),
        Expr::Case {
            operand,
            when_then,
            else_expr,
        } => Expr::Case {
            operand: operand.map(bind),
            when_then: when_then
                .into_iter()
                .map(|(when, then)| {
                    (
                        bind_aggregate_references(when),
                        bind_aggregate_references(then),
                    )
                })
                .collect(),
            else_expr: else_expr.map(bind),
        },
        Expr::Like {
            expr,
            pattern,
            negated,
            case_insensitive,
        } => Expr::Like {
            expr: bind(expr),
            pattern: bind(pattern),
            negated,
            case_insensitive,
        },
        Expr::Between {
            expr,
            low,
            high,
            negated,
        } => Expr::Between {
            expr: bind(expr),
            low: bind(low),
            high: bind(high),
            negated,
        },
        Expr::InList {
            expr,
            list,
            negated,
        } => Expr::InList {
            expr: bind(expr),
            list: list.into_iter().map(bind_aggregate_references).collect(),
            negated,
        },
        Expr::Cast { expr, data_type } => Expr::Cast {
            expr: bind(expr),
            data_type,
        },
        Expr::Extract { field, expr } => Expr::Extract {
            field,
            expr: bind(expr),
        },
        other => other,
    }
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
                AggregateExpr::Sum { expr, .. } => {
                    (AggregateFunction::Sum, Some(expr.clone()), "sum")
                }
                AggregateExpr::Min(expr) => (AggregateFunction::Min, Some(expr.clone()), "min"),
                AggregateExpr::Max(expr) => (AggregateFunction::Max, Some(expr.clone()), "max"),
                AggregateExpr::Avg { expr, .. } => {
                    (AggregateFunction::Avg, Some(expr.clone()), "avg")
                }
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
            // `ORDER BY COUNT(*) DESC` above an aggregate orders by the
            // aggregate's output column.
            expression: bind_aggregate_references(expression.clone()),
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
            LogicalPlan::Limit { .. } if plan.top_n().is_some() => {
                let shape = plan.top_n().expect("matched above");
                let source = self.build(shape.input)?;
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
                distribution,
            } => {
                let left_stage = self.build(left)?;
                let right_stage = self.build(right)?;
                let join = physical_plan_tree(plan);
                if *distribution == JoinDistribution::BroadcastRight {
                    self.wrap_stage(left_stage, "BroadcastHashJoin", join.attributes);
                    self.add_exchange(right_stage, left_stage, Partitioning::Broadcast);
                    return Ok(left_stage);
                }
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
            LogicalPlan::Offset { input, .. } => {
                let source = self.build(input)?;
                let physical = physical_plan_tree(plan);
                let target = self.add_exchange_stage("FinalOffset", physical.attributes, 1, 1);
                self.add_exchange(source, target, Partitioning::Single);
                Ok(target)
            }
            LogicalPlan::Distinct { input } => {
                let source = self.build(input)?;
                let physical = physical_plan_tree(plan);
                // Equal rows hash alike, so DISTINCT over named columns
                // spreads across the workers; each partition then holds a
                // disjoint share of the distinct rows and never waits on a
                // single final task.
                match distinct_partition_columns(input) {
                    Some(columns) => {
                        let target = self.add_exchange_stage(
                            "PartitionedDistinct",
                            physical.attributes,
                            self.worker_count,
                            1,
                        );
                        self.add_exchange(
                            source,
                            target,
                            Partitioning::Hash {
                                columns,
                                partition_count: self.worker_count,
                            },
                        );
                        Ok(target)
                    }
                    None => {
                        let target =
                            self.add_exchange_stage("FinalDistinct", physical.attributes, 1, 1);
                        self.add_exchange(source, target, Partitioning::Single);
                        Ok(target)
                    }
                }
            }
            LogicalPlan::Union { inputs, .. } => {
                let mut stages = Vec::new();
                for input in inputs {
                    stages.push(self.build(input)?);
                }
                let physical = physical_plan_tree(plan);
                let target = self.add_exchange_stage("Union", physical.attributes, 1, stages.len());
                for stage in stages {
                    self.add_exchange(stage, target, Partitioning::Single);
                }
                Ok(target)
            }
            LogicalPlan::Window { input, .. } => {
                let source = self.build(input)?;
                let physical = physical_plan_tree(plan);
                let target = self.add_exchange_stage("FinalWindow", physical.attributes, 1, 1);
                self.add_exchange(source, target, Partitioning::Single);
                Ok(target)
            }
            LogicalPlan::Intersect { left, right } | LogicalPlan::Except { left, right } => {
                let left_stage = self.build(left)?;
                let right_stage = self.build(right)?;
                let physical = physical_plan_tree(plan);
                let operator = if matches!(plan, LogicalPlan::Intersect { .. }) {
                    "Intersect"
                } else {
                    "Except"
                };
                let target = self.add_exchange_stage(operator, physical.attributes, 1, 2);
                self.add_exchange(left_stage, target, Partitioning::Single);
                self.add_exchange(right_stage, target, Partitioning::Single);
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
            LogicalPlan::SemiJoin { left, right, .. }
            | LogicalPlan::AntiJoin { left, right, .. } => {
                let left_stage = self.build(left)?;
                let right_stage = self.build(right)?;
                let physical = physical_plan_tree(plan);
                self.wrap_stage(left_stage, "BroadcastSemiJoin", physical.attributes);
                self.add_exchange(right_stage, left_stage, Partitioning::Broadcast);
                Ok(left_stage)
            }
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
        && let Some(top_n) = plan.top_n()
    {
        let mut node = kaveon_core::PlanNode::new(id, phase, "TopN");
        node.attributes = BTreeMap::from([
            ("rows".to_owned(), top_n.fetch.to_string()),
            ("order_by".to_owned(), format!("{:?}", top_n.order_by)),
        ]);
        if top_n.skip > 0 {
            node.attributes
                .insert("skip".to_owned(), top_n.skip.to_string());
        }
        node.children
            .push(build_plan_tree(top_n.input, next_id, phase));
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
        LogicalPlan::Offset { input, count } => (
            "Offset",
            BTreeMap::from([("rows".to_owned(), count.to_string())]),
            Some(input.as_ref()),
        ),
        LogicalPlan::Distinct { input } => ("Distinct", BTreeMap::new(), Some(input.as_ref())),
        LogicalPlan::Window { input, .. } => ("Window", BTreeMap::new(), Some(input.as_ref())),
        LogicalPlan::Union { .. } => ("Union", BTreeMap::new(), None),
        LogicalPlan::Intersect { .. } => ("Intersect", BTreeMap::new(), None),
        LogicalPlan::Except { .. } => ("Except", BTreeMap::new(), None),
        LogicalPlan::SemiJoin { .. } => ("SemiJoin", BTreeMap::new(), None),
        LogicalPlan::AntiJoin { .. } => ("AntiJoin", BTreeMap::new(), None),
    };
    let mut node = kaveon_core::PlanNode::new(id, phase, operator);
    node.attributes = attributes;
    if let Some(input) = input {
        node.children.push(build_plan_tree(input, next_id, phase));
    } else if let LogicalPlan::Join { left, right, .. }
    | LogicalPlan::Intersect { left, right }
    | LogicalPlan::Except { left, right } = plan
    {
        node.children.push(build_plan_tree(left, next_id, phase));
        node.children.push(build_plan_tree(right, next_id, phase));
    } else if let LogicalPlan::SemiJoin { left, right, .. }
    | LogicalPlan::AntiJoin { left, right, .. } = plan
    {
        node.children.push(build_plan_tree(left, next_id, phase));
        node.children.push(build_plan_tree(right, next_id, phase));
    } else if let LogicalPlan::Union { inputs, .. } = plan {
        for input in inputs {
            node.children.push(build_plan_tree(input, next_id, phase));
        }
    }
    node
}

fn plan_query_inner(
    plan: &LogicalPlan,
    catalog: &CatalogManager,
    partition: Option<ScanPartition>,
    memory: Option<&QueryMemoryPool>,
) -> Result<PlannedQuery> {
    plan_query_with_predicate(plan, catalog, None, partition, memory)
}

fn plan_query_with_predicate(
    plan: &LogicalPlan,
    catalog: &CatalogManager,
    storage_predicate: Option<&kaveon_core::StoragePredicate>,
    partition: Option<ScanPartition>,
    memory: Option<&QueryMemoryPool>,
) -> Result<PlannedQuery> {
    match plan {
        LogicalPlan::Scan { table, columns, .. } => {
            let reference = TableReference::parse(table);
            let resolved = catalog.resolve_table(&reference)?;
            let path = resolved.full_path();

            let (source, scan_metrics): (Box<dyn BatchSource>, _) = match resolved.table.format {
                DataFormat::Parquet => {
                    if path.starts_with("s3://") {
                        let mut reader = ObjectParquetReader::from_uri(&path)?;
                        if let Some(cols) = columns {
                            reader = reader.with_columns(cols.clone());
                        }
                        if let Some(predicate) = storage_predicate {
                            reader = reader.with_predicate(predicate.clone());
                        }
                        if let Some(partition) = partition {
                            reader = reader.with_partition(partition);
                        }
                        let source = reader.read_blocking()?;
                        let metrics = source.metrics();
                        return Ok(PlannedQuery {
                            operator: Box::new(ScanOperator::new(
                                Box::new(source),
                                columns.as_deref(),
                            )?),
                            scan_metrics: vec![metrics],
                        });
                    }
                    if path.starts_with("abfss://") {
                        let mut reader = AdlsParquetReader::from_abfss_uri(&path)?;
                        if let Some(cols) = columns {
                            reader = reader.with_columns(cols.clone());
                        }
                        if let Some(predicate) = storage_predicate {
                            reader = reader.with_predicate(predicate.clone());
                        }
                        if let Some(partition) = partition {
                            reader = reader.with_partition(partition);
                        }
                        let source = reader.read_blocking().map_err(|error| {
                            KaveonError::Execution(format!("failed to open '{path}': {error}"))
                        })?;
                        let metrics = source.metrics();
                        return Ok(PlannedQuery {
                            operator: Box::new(ScanOperator::new(
                                Box::new(source),
                                columns.as_deref(),
                            )?),
                            scan_metrics: vec![metrics],
                        });
                    }
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
                    if path.starts_with("abfss://") || path.starts_with("s3://") {
                        let mut reader = ObjectDeltaReader::from_uri(&path)?;
                        if let Some(predicate) = storage_predicate {
                            reader = reader.with_predicate(predicate.clone());
                        }
                        if let Some(cols) = columns {
                            reader = reader.with_columns(cols.clone());
                        }
                        if let Some(partition) = partition {
                            reader = reader.with_partition(partition);
                        }
                        let source = reader.read_blocking()?;
                        let metrics = source.metrics();
                        return Ok(PlannedQuery {
                            operator: Box::new(ScanOperator::new(
                                Box::new(source),
                                columns.as_deref(),
                            )?),
                            scan_metrics: vec![metrics],
                        });
                    }
                    let mut reader = DeltaTableReader::new(&path);
                    if let Some(predicate) = storage_predicate {
                        reader = reader.with_predicate(predicate.clone());
                    }
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
                    let mut reader = kaveon_storage::IcebergReader::new(&path);
                    if let Some(columns) = columns {
                        reader = reader.with_columns(columns.clone());
                    }
                    if let Some(partition) = partition {
                        reader = reader.with_partition(partition);
                    }
                    let source = reader.read_blocking()?;
                    let metrics = source.metrics();
                    (Box::new(source), vec![metrics])
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
            distribution: _,
        } => {
            let left_qualifier = relation_qualifier(left);
            let right_qualifier = relation_qualifier(right);
            let left = plan_query_inner(left, catalog, partition, memory)?;
            let right = plan_query_inner(right, catalog, partition, memory)?;
            let keys = join_keys(condition.as_ref())?;
            let mut scan_metrics = left.scan_metrics;
            scan_metrics.extend(right.scan_metrics);
            Ok(PlannedQuery {
                operator: kaveon_exec::partitioned::hash_join(
                    left.operator,
                    right.operator,
                    physical_join_type(*join_type),
                    keys,
                    left_qualifier.as_deref(),
                    right_qualifier.as_deref(),
                    memory
                        .map(|memory| memory.operator("hash-join"))
                        .transpose()?,
                )?,
                scan_metrics,
            })
        }

        LogicalPlan::Filter { input, predicate } => {
            // HAVING: the filter sits on the aggregate's output, where
            // SUM(x) is a column, and nothing about it reaches the scan.
            let over_aggregate = matches!(input.as_ref(), LogicalPlan::Aggregate { .. });
            let predicate = if over_aggregate {
                bind_aggregate_references(predicate.clone())
            } else {
                predicate.clone()
            };
            let pushed = if over_aggregate {
                None
            } else {
                to_storage_predicate(&predicate)
            };
            let planned =
                plan_query_with_predicate(input, catalog, pushed.as_ref(), partition, memory)?;
            let mut operator = FilterOperator::new(planned.operator, predicate);
            if let Some(memory) = memory {
                operator = operator.with_memory(memory.operator("filter")?);
            }
            Ok(PlannedQuery {
                operator: Box::new(operator),
                scan_metrics: planned.scan_metrics,
            })
        }

        LogicalPlan::Project { input, columns } => {
            let planned = plan_query_inner(input, catalog, partition, memory)?;

            let has_star = columns.iter().any(|e| matches!(e, Expr::Star));
            if has_star {
                return Ok(planned);
            }

            let exprs: Vec<Expr> = columns
                .iter()
                .map(|e| match e {
                    Expr::Function { name, args }
                        if AGGREGATE_FUNCTIONS.contains(&name.as_str()) =>
                    {
                        let col = agg_output_name(name, args);
                        Expr::Column(col)
                    }
                    Expr::Alias { expr, name } => match expr.as_ref() {
                        Expr::Function { name: fname, args }
                            if AGGREGATE_FUNCTIONS.contains(&fname.as_str()) =>
                        {
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

            let mut operator = ProjectOperator::new(planned.operator, exprs)?;
            if let Some(memory) = memory {
                operator = operator.with_memory(memory.operator("project")?);
            }
            Ok(PlannedQuery {
                operator: Box::new(operator),
                scan_metrics: planned.scan_metrics,
            })
        }

        LogicalPlan::Aggregate {
            input,
            group_by,
            aggregates,
        } => {
            if group_by.is_empty()
                && !aggregates.is_empty()
                && aggregates.iter().all(|aggregate| {
                    matches!(
                        aggregate,
                        AggregateExpr::Count {
                            expr: Expr::Star,
                            distinct: false
                        }
                    )
                })
                && let LogicalPlan::Scan { table, columns, .. } = input.as_ref()
                && partition.is_none()
                && storage_predicate.is_none()
                && let Some(operator) = kaveon_exec::metadata_count::MetadataCount::try_new(
                    catalog,
                    table,
                    columns.as_deref(),
                    aggregates.len(),
                    memory
                        .map(|pool| pool.operator("metadata-count"))
                        .transpose()?,
                )?
            {
                return Ok(PlannedQuery {
                    operator: Box::new(operator),
                    scan_metrics: Vec::new(),
                });
            }
            let planned = plan_query_inner(input, catalog, partition, memory)?;

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

            let parallelism = kaveon_exec::local_parallel::query_parallelism(memory)?;
            // Parallel partials account through the query pool; without one
            // (embedded and test plans) the aggregate runs serially.
            let operator = if let Some(pool) = memory.filter(|_| parallelism > 1) {
                let pool = pool.clone();
                let probe = kaveon_exec::aggregate::HashAggregate::new(
                    Box::new(kaveon_exec::local_parallel::EmptyInput(
                        planned.operator.schema().clone(),
                    )),
                    group_cols.clone(),
                    agg_exprs.clone(),
                )?;
                let schema = probe.schema().clone();
                let partials = kaveon_exec::local_parallel::ParallelPartials::new(
                    planned.operator,
                    group_cols.clone(),
                    agg_exprs.clone(),
                    pool.clone(),
                    parallelism,
                )?;
                Box::new(kaveon_exec::local_parallel::LazyFinalAggregate::new(
                    schema,
                    Box::new(partials),
                    Box::new(move |input| {
                        crate::fragment_exec::compile_final_aggregate(
                            input,
                            group_cols,
                            agg_exprs,
                            Some(&pool),
                        )
                    }),
                )) as Box<dyn BatchOperator>
            } else {
                kaveon_exec::partitioned::hash_aggregate(
                    planned.operator,
                    group_cols,
                    agg_exprs,
                    memory
                        .map(|memory| memory.operator("hash-aggregate"))
                        .transpose()?,
                )?
            };
            Ok(PlannedQuery {
                operator,
                scan_metrics: planned.scan_metrics,
            })
        }

        LogicalPlan::Sort { input, order_by } => {
            let planned = plan_query_inner(input, catalog, partition, memory)?;
            let sort_exprs = order_by
                .iter()
                .map(|(expr, ascending)| SortExpr::new(expr.clone(), *ascending))
                .collect();
            Ok(PlannedQuery {
                operator: kaveon_exec::partitioned::sort_operator(
                    planned.operator,
                    sort_exprs,
                    memory.map(|memory| memory.operator("sort")).transpose()?,
                )?,
                scan_metrics: planned.scan_metrics,
            })
        }

        LogicalPlan::Limit { input, count } => {
            if let Some(top_n) = plan.top_n() {
                let planned = plan_query_inner(top_n.input, catalog, partition, memory)?;
                let sort_exprs = top_n
                    .order_by
                    .iter()
                    .map(|(expr, ascending)| SortExpr::new(expr.clone(), *ascending))
                    .collect();
                let retained = kaveon_exec::partitioned::top_n_operator(
                    planned.operator,
                    sort_exprs,
                    top_n.retained(),
                    memory.map(|memory| memory.operator("topn")).transpose()?,
                )?;
                return Ok(PlannedQuery {
                    operator: if top_n.skip > 0 {
                        Box::new(OffsetOperator::new(retained, top_n.skip))
                    } else {
                        retained
                    },
                    scan_metrics: planned.scan_metrics,
                });
            }
            let planned = plan_query_inner(input, catalog, partition, memory)?;
            Ok(PlannedQuery {
                operator: Box::new(LimitOperator::new(planned.operator, *count)),
                scan_metrics: planned.scan_metrics,
            })
        }

        LogicalPlan::Offset { input, count } => {
            let planned = plan_query_inner(input, catalog, partition, memory)?;
            Ok(PlannedQuery {
                operator: Box::new(OffsetOperator::new(planned.operator, *count)),
                scan_metrics: planned.scan_metrics,
            })
        }

        LogicalPlan::Distinct { input } => {
            let planned = plan_query_inner(input, catalog, partition, memory)?;
            Ok(PlannedQuery {
                operator: crate::fragment_exec::distinct_operator(planned.operator, memory)?,
                scan_metrics: planned.scan_metrics,
            })
        }

        LogicalPlan::Union { inputs, .. } => {
            let mut operators: Vec<Box<dyn BatchOperator>> = Vec::new();
            let mut scan_metrics = Vec::new();
            for input in inputs {
                let planned = plan_query_inner(input, catalog, partition, memory)?;
                operators.push(planned.operator);
                scan_metrics.extend(planned.scan_metrics);
            }
            Ok(PlannedQuery {
                operator: Box::new(UnionOperator::new(operators)),
                scan_metrics,
            })
        }

        LogicalPlan::Window {
            input,
            window_exprs,
        } => {
            let planned = plan_query_inner(input, catalog, partition, memory)?;
            let mut operator = WindowOperator::new(planned.operator, window_exprs.clone())?;
            if let Some(memory) = memory {
                operator = operator.with_memory(memory.operator("window")?);
            }
            Ok(PlannedQuery {
                operator: Box::new(operator),
                scan_metrics: planned.scan_metrics,
            })
        }

        LogicalPlan::Intersect { left, right } => {
            let left_planned = plan_query_inner(left, catalog, partition, memory)?;
            let right_planned = plan_query_inner(right, catalog, partition, memory)?;
            let mut scan_metrics = left_planned.scan_metrics;
            scan_metrics.extend(right_planned.scan_metrics);
            let mut operator = SetOpOperator::new(
                left_planned.operator,
                right_planned.operator,
                SetOpMode::Intersect,
            );
            if let Some(memory) = memory {
                operator = operator.with_memory(memory.operator("set-operation")?);
            }
            Ok(PlannedQuery {
                operator: Box::new(operator),
                scan_metrics,
            })
        }

        LogicalPlan::Except { left, right } => {
            let left_planned = plan_query_inner(left, catalog, partition, memory)?;
            let right_planned = plan_query_inner(right, catalog, partition, memory)?;
            let mut scan_metrics = left_planned.scan_metrics;
            scan_metrics.extend(right_planned.scan_metrics);
            let mut operator = SetOpOperator::new(
                left_planned.operator,
                right_planned.operator,
                SetOpMode::Except,
            );
            if let Some(memory) = memory {
                operator = operator.with_memory(memory.operator("set-operation")?);
            }
            Ok(PlannedQuery {
                operator: Box::new(operator),
                scan_metrics,
            })
        }
        LogicalPlan::SemiJoin {
            left,
            right,
            left_key,
            right_key,
        }
        | LogicalPlan::AntiJoin {
            left,
            right,
            left_key,
            right_key,
        } => {
            let left_planned = plan_query_inner(left, catalog, partition, memory)?;
            let right_planned = plan_query_inner(right, catalog, partition, memory)?;
            let mut scan_metrics = left_planned.scan_metrics;
            scan_metrics.extend(right_planned.scan_metrics);
            let mut operator = SemiJoinOperator::new(
                left_planned.operator,
                right_planned.operator,
                left_key.clone(),
                right_key.clone(),
                matches!(plan, LogicalPlan::AntiJoin { .. }),
            )?;
            if let Some(memory) = memory {
                operator = operator.with_memory(memory.operator("semi-join")?);
            }
            Ok(PlannedQuery {
                operator: Box::new(operator),
                scan_metrics,
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
        AggregateExpr::Sum { expr, distinct } => (AggFunc::Sum, expr, *distinct),
        AggregateExpr::Avg { expr, distinct } => (AggFunc::Avg, expr, *distinct),
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

/// The input field `name` denotes, by its own name: exact, else the one
/// field whose bare name matches (`c_name` reaches a join's
/// `customer.c_name`; `t.x` reaches a scan's `x`).
fn resolve_column_name(schema: &arrow::datatypes::SchemaRef, name: &str) -> Result<String> {
    kaveon_exec::expr_eval::resolve_column_index(schema, name)
        .map(|index| schema.field(index).name().clone())
        .map_err(|error| match error {
            KaveonError::Execution(message) if message.contains("ambiguous") => {
                KaveonError::Execution(format!("column '{name}' is ambiguous in input"))
            }
            _ => KaveonError::Execution(format!("column '{name}' not found in input")),
        })
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
            // Keys keep their qualifiers: `n1.n_nationkey` beside
            // `n2.n_nationkey` is only unambiguous with them, and each side
            // resolves its key exactly or by bare name.
            (Expr::Column(left), Expr::Column(right)) => Ok(vec![(left.clone(), right.clone())]),
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

/// The output names a DISTINCT can hash-partition on: every projected
/// expression of its input, when that input is a projection of columns and
/// aliases (what `SELECT DISTINCT a, b` and the COUNT(DISTINCT) rewrite
/// produce). Anything else keeps the single final task.
fn distinct_partition_columns(input: &LogicalPlan) -> Option<Vec<String>> {
    let LogicalPlan::Project { columns, .. } = input else {
        return None;
    };
    if columns.is_empty() || !columns.iter().all(|expression| {
        matches!(expression, Expr::Column(_))
            || matches!(expression, Expr::Alias { expr, .. } if matches!(**expr, Expr::Column(_)))
    }) {
        return None;
    }
    Some(
        named_expressions(columns)
            .into_iter()
            .map(|named| named.name)
            .collect(),
    )
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
        // A pushed-down filter keeps its input relation.
        LogicalPlan::Filter { input, .. } => relation_qualifier(input),
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
    use kaveon_core::predicate::ScalarValue;
    use kaveon_core::{
        AccessPattern, BinaryOp, CatalogProvider, DataFormat, MemoryCatalog, StorageType,
        TableMeta, collect_batches,
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
    fn column_names_resolve_to_the_field_they_denote() {
        let schema = Arc::new(Schema::new(vec![
            Field::new("customer.c_name", DataType::Utf8, true),
            Field::new("orders.o_custkey", DataType::Int64, true),
            Field::new("flat", DataType::Int64, true),
        ]));
        // A bare name over a join output is that output's qualified field;
        // a qualified name over a bare field is the bare field.
        assert_eq!(
            resolve_column_name(&schema, "c_name").unwrap(),
            "customer.c_name"
        );
        assert_eq!(
            resolve_column_name(&schema, "orders.o_custkey").unwrap(),
            "orders.o_custkey"
        );
        assert_eq!(resolve_column_name(&schema, "t.flat").unwrap(), "flat");
        assert!(
            resolve_column_name(&schema, "missing")
                .unwrap_err()
                .to_string()
                .contains("not found")
        );
        let twice = Arc::new(Schema::new(vec![
            Field::new("n1.n_name", DataType::Utf8, true),
            Field::new("n2.n_name", DataType::Utf8, true),
        ]));
        assert!(
            resolve_column_name(&twice, "n_name")
                .unwrap_err()
                .to_string()
                .contains("ambiguous")
        );
    }
    #[test]
    fn metadata_count_sql_is_exact_snapshot_pinned_and_filter_safe() {
        let mut fixture = fixture();
        let schema = fixture
            .catalog
            .resolve_table(&TableReference::parse("test.default.items"))
            .unwrap()
            .table
            .arrow_schema
            .clone();
        let mut writer = ArrowWriter::try_new(
            File::create(fixture.directory.join("empty.parquet")).unwrap(),
            schema.clone(),
            None,
        )
        .unwrap();
        writer
            .write(&RecordBatch::new_empty(schema.clone()))
            .unwrap();
        writer.close().unwrap();
        let delta = fixture.directory.join("delta");
        fs::create_dir_all(delta.join("_delta_log")).unwrap();
        for name in ["a.parquet", "b.parquet"] {
            fs::copy(fixture.directory.join("items.parquet"), delta.join(name)).unwrap();
        }
        let logical_schema = serde_json::json!({"type":"struct","fields":[{"name":"id","type":"long","nullable":true,"metadata":{}}]});
        let actions = [
            serde_json::json!({"protocol":{"minReaderVersion":1,"minWriterVersion":2}}),
            serde_json::json!({"metaData":{"id":"count-test","schemaString":logical_schema.to_string(),"partitionColumns":[],"configuration":{}}}),
            serde_json::json!({"add":{"path":"a.parquet"}}),
            serde_json::json!({"add":{"path":"b.parquet"}}),
        ];
        fs::write(
            delta.join("_delta_log/00000000000000000000.json"),
            actions
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join("\n"),
        )
        .unwrap();
        let mut catalog = MemoryCatalog::new(
            "counts",
            StorageType::Local {
                base_path: fixture.directory.clone(),
            },
        )
        .with_schema("default");
        for (name, location, format) in [
            ("empty", "empty.parquet", DataFormat::Parquet),
            ("delta", "delta", DataFormat::Delta),
        ] {
            catalog
                .register_table(
                    "default",
                    TableMeta {
                        name: name.into(),
                        arrow_schema: schema.clone(),
                        location: location.into(),
                        access: AccessPattern::Shortcut,
                        format,
                    },
                )
                .unwrap();
        }
        fixture.catalog.register_catalog(Box::new(catalog));
        let plan_sql = |sql| {
            let plan = kaveon_sql::logical_plan::sql_to_logical_plan(sql).unwrap();
            plan_query(&plan, &fixture.catalog)
        };
        let count = |mut planned: PlannedQuery| {
            let batches = collect_batches(&mut *planned.operator).unwrap();
            assert_eq!(batches.len(), 1);
            batches[0]
                .column(0)
                .as_any()
                .downcast_ref::<arrow::array::UInt64Array>()
                .unwrap()
                .value(0)
        };
        let plain = plan_sql("SELECT COUNT(*) AS total FROM test.default.items").unwrap();
        assert!(plain.scan_metrics.is_empty());
        assert_eq!(plain.operator.schema().field(0).name(), "total");
        assert_eq!(count(plain), 4);
        assert_eq!(
            count(plan_sql("SELECT COUNT(*) FROM counts.default.empty").unwrap()),
            0
        );
        for (sql, expected) in [
            ("SELECT COUNT(*) FROM test.default.items WHERE id > 100", 1),
            ("SELECT COUNT(id) FROM test.default.items", 4),
            ("SELECT COUNT(DISTINCT id) FROM test.default.items", 4),
        ] {
            let planned = plan_sql(sql).unwrap();
            assert!(
                !planned.scan_metrics.is_empty(),
                "{sql} must retain scan execution"
            );
            assert_eq!(count(planned), expected);
        }
        let pinned = plan_sql("SELECT COUNT(*) FROM counts.default.delta").unwrap();
        fs::write(
            delta.join("_delta_log/00000000000000000001.json"),
            r#"{"remove":{"path":"b.parquet"}}"#,
        )
        .unwrap();
        assert_eq!(count(pinned), 8);
        assert_eq!(
            count(plan_sql("SELECT COUNT(*) FROM counts.default.delta").unwrap()),
            4
        );
        fs::write(
            delta.join("_delta_log/00000000000000000002.json"),
            r#"{"remove":{"path":"a.parquet"}}"#,
        )
        .unwrap();
        assert_eq!(
            count(plan_sql("SELECT COUNT(*) FROM counts.default.delta").unwrap()),
            0
        );
        fs::write(delta.join("_delta_log/00000000000000000003.json"), r#"{"protocol":{"minReaderVersion":3,"minWriterVersion":7,"readerFeatures":["deletionVectors"]}}"#).unwrap();
        assert!(plan_sql("SELECT COUNT(*) FROM counts.default.delta").is_err());
    }

    #[test]
    fn grouped_statement_honours_the_query_parallelism_ceiling() {
        // The ceiling rides on the pool: the planner asks the pool, not the
        // process, so a statement that lowered its parallelism runs the
        // serial aggregate and produces the same groups.
        let fixture = fixture();
        let sql = "SELECT id, COUNT(*), SUM(id) FROM items GROUP BY id ORDER BY id";
        let mut rows_by_ceiling = Vec::new();
        for ceiling in [None, Some(1)] {
            let pool = QueryMemoryPool::new("parallelism-ceiling", 64 * 1024 * 1024).unwrap();
            if let Some(threads) = ceiling {
                kaveon_exec::local_parallel::set_query_parallelism(&pool, threads).unwrap();
                assert_eq!(
                    kaveon_exec::local_parallel::query_parallelism(Some(&pool)).unwrap(),
                    threads
                );
            }
            let mut plan = kaveon_sql::logical_plan::sql_to_logical_plan(sql).unwrap();
            qualify_tables(&mut plan, "test", "default");
            let mut planned = plan_query_with_memory(&plan, &fixture.catalog, &pool).unwrap();
            let batches = collect_batches(&mut *planned.operator).unwrap();
            let rows = batches
                .iter()
                .flat_map(|batch| {
                    (0..batch.num_rows()).map(move |row| {
                        (0..batch.num_columns())
                            .map(|column| {
                                arrow::util::display::array_value_to_string(
                                    batch.column(column),
                                    row,
                                )
                                .unwrap()
                            })
                            .collect::<Vec<_>>()
                    })
                })
                .collect::<Vec<_>>();
            assert_eq!(rows.len(), 4, "{ceiling:?}");
            rows_by_ceiling.push(rows);
        }
        assert_eq!(rows_by_ceiling[0], rows_by_ceiling[1]);
    }

    #[test]
    fn parallel_sql_aggregates_preserve_projection_bindings() {
        const CHILD: &str = "KAVEON_PARALLEL_SQL_REGRESSION_CHILD";
        if std::env::var_os(CHILD).is_none() {
            let result = std::process::Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "planner::tests::parallel_sql_aggregates_preserve_projection_bindings",
                    "--nocapture",
                ])
                .env(CHILD, "1")
                .env("KAVEON_LOCAL_PARALLELISM", "4")
                .output()
                .unwrap();
            assert!(
                result.status.success(),
                "{}\n{}",
                String::from_utf8_lossy(&result.stdout),
                String::from_utf8_lossy(&result.stderr)
            );
            return;
        }
        let fixture = fixture();
        let pool = QueryMemoryPool::new("parallel-sql-regression", 64 * 1024 * 1024).unwrap();
        for sql in [
            "SELECT COUNT(*), SUM(id), MIN(id), MAX(id), AVG(id) FROM items",
            "SELECT COUNT(DISTINCT id), SUM(DISTINCT id) FROM items",
            "SELECT id, COUNT(*), SUM(id) FROM items GROUP BY id ORDER BY id",
            "SELECT COUNT(*) AS total, SUM(id) AS amount FROM items WHERE id > 100",
            "SELECT COUNT(*), SUM(id) FROM items WHERE id > 1000",
            "SELECT COUNT(*), SUM(i.id) FROM items i JOIN customers c ON i.id=c.id",
        ] {
            let mut plan = kaveon_sql::logical_plan::sql_to_logical_plan(sql).unwrap();
            qualify_tables(&mut plan, "test", "default");
            let mut planned = plan_query_with_memory(&plan, &fixture.catalog, &pool).unwrap();
            let expected_schema = planned.operator.schema().clone();
            let batches = collect_batches(&mut *planned.operator).unwrap();
            assert!(!batches.is_empty(), "{sql}");
            for batch in &batches {
                assert_eq!(batch.schema(), expected_schema, "{sql}");
            }
            let first = &batches[0];
            if sql.contains("GROUP BY") {
                assert_eq!(batches.iter().map(RecordBatch::num_rows).sum::<usize>(), 4);
            } else {
                let count = first
                    .column(0)
                    .as_any()
                    .downcast_ref::<arrow::array::UInt64Array>()
                    .unwrap()
                    .value(0);
                assert_eq!(
                    count,
                    if sql.contains("> 1000") {
                        0
                    } else if sql.contains("> 100") {
                        1
                    } else {
                        4
                    },
                    "{sql}"
                );
                let sum = first
                    .column(1)
                    .as_any()
                    .downcast_ref::<Int64Array>()
                    .unwrap();
                if sql.contains("> 1000") {
                    assert!(arrow::array::Array::is_null(sum, 0));
                } else {
                    assert_eq!(
                        sum.value(0),
                        if sql.contains("> 100") { 101 } else { 204 },
                        "{sql}"
                    );
                }
            }
            drop(planned);
        }
    }

    #[test]
    fn top_n_keys_bind_aggregate_calls_to_their_outputs() {
        let fragments = executable_fragments(
            "SELECT id, COUNT(*) FROM items GROUP BY id ORDER BY COUNT(*) DESC LIMIT 3",
        );
        let top_n = fragments
            .values()
            .flat_map(|fragment| fragment.nodes.iter())
            .find_map(|node| match &node.operator {
                FragmentOperator::TopN { keys, .. } => Some(keys.clone()),
                _ => None,
            })
            .expect("ORDER BY ... LIMIT plans a TopN");
        // The SQL layer binds the repeated select item to its output name.
        assert_eq!(top_n[0].expression, Expr::Column("expr_1".into()));
        assert!(!top_n[0].ascending);
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
    fn having_and_nested_aggregate_references_bind_to_aggregate_outputs() {
        let fragments = executable_fragments(
            "SELECT id, SUM(id) * 2 AS doubled, CASE WHEN COUNT(*) > 1 THEN 'many' ELSE 'one' END AS n              FROM items GROUP BY id HAVING SUM(id) > 100 AND COUNT(*) >= 1",
        );
        let final_fragment = &fragments[&StageId(1)];
        let filter = final_fragment
            .nodes
            .iter()
            .find_map(|node| match &node.operator {
                FragmentOperator::Filter { predicate } => Some(predicate.clone()),
                _ => None,
            })
            .expect("HAVING becomes a filter on the final stage");
        // `SUM(id) * 2` computes with an aggregate, so the SQL layer lowers
        // SUM(id) to a named argument column; COUNT(*) has no argument to
        // lower and binds here to the fragment's own output name.
        assert_eq!(
            filter,
            Expr::And(
                Box::new(Expr::BinaryOp {
                    left: Box::new(Expr::Column("sum___kaveon_arg_0".into())),
                    op: BinaryOp::Gt,
                    right: Box::new(Expr::Literal(ScalarValue::Int64(100))),
                }),
                Box::new(Expr::BinaryOp {
                    left: Box::new(Expr::Column("count_star".into())),
                    op: BinaryOp::Ge,
                    right: Box::new(Expr::Literal(ScalarValue::Int64(1))),
                }),
            )
        );
        // The scan never receives a predicate built from aggregate outputs.
        let partial = &fragments[&StageId(0)];
        assert!(partial.nodes.iter().all(|node| match &node.operator {
            FragmentOperator::Scan(scan) => scan.predicate.is_none(),
            _ => true,
        }));
        let FragmentOperator::Project { expressions } =
            &final_fragment.nodes.last().unwrap().operator
        else {
            panic!("final stage ends in the projection");
        };
        assert_eq!(
            expressions[1].expression,
            Expr::Alias {
                expr: Box::new(Expr::BinaryOp {
                    left: Box::new(Expr::Column("sum___kaveon_arg_0".into())),
                    op: BinaryOp::Multiply,
                    right: Box::new(Expr::Literal(ScalarValue::Int64(2))),
                }),
                name: "doubled".into(),
            }
        );
        let Expr::Alias { expr, .. } = &expressions[2].expression else {
            panic!("CASE keeps its alias");
        };
        let Expr::Case { when_then, .. } = expr.as_ref() else {
            panic!("CASE survives binding");
        };
        assert!(matches!(
            &when_then[0].0,
            Expr::BinaryOp { left, .. } if **left == Expr::Column("count_star".into())
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
    fn translates_an_offset_window_into_a_top_n_that_retains_the_skipped_rows() {
        let fragments =
            executable_fragments("SELECT id FROM items ORDER BY id DESC OFFSET 1000 LIMIT 10");
        assert_eq!(fragments.len(), 2);
        assert!(
            fragments[&StageId(0)]
                .nodes
                .iter()
                .any(|node| matches!(node.operator, FragmentOperator::TopN { limit: 1010, .. }))
        );
        assert!(
            fragments[&StageId(1)]
                .nodes
                .iter()
                .all(|node| !matches!(node.operator, FragmentOperator::Sort { .. }))
        );
        let target = &fragments[&StageId(1)].nodes;
        assert!(matches!(
            target[target.len() - 2].operator,
            FragmentOperator::TopN { limit: 1010, .. }
        ));
        assert!(matches!(
            target.last().unwrap().operator,
            FragmentOperator::Offset { offset: 1000 }
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
    fn count_distinct_partitions_the_distinct_rows_across_workers() {
        let sql = "SELECT id, COUNT(DISTINCT id) FROM items GROUP BY id";
        let mut plan = kaveon_sql::logical_plan::sql_to_logical_plan(sql).unwrap();
        qualify_tables(&mut plan, "test", "default");
        let plan = kaveon_optim::rules::push_projection_down(plan);
        let graph = build_stage_graph("query-1", &plan, 4).unwrap();
        fn mentions(node: &kaveon_core::PlanNode, operator: &str) -> bool {
            node.operator == operator || node.children.iter().any(|child| mentions(child, operator))
        }
        let distinct = graph
            .stages
            .iter()
            .find(|stage| mentions(&stage.plan, "PartitionedDistinct"))
            .expect("distinct spreads across the workers");
        assert_eq!(distinct.task_count, 4);
        assert!(graph.exchanges.iter().any(|exchange| {
            exchange.target_stage == distinct.id
                && matches!(
                    &exchange.partitioning,
                    Partitioning::Hash { columns, partition_count: 4 } if columns == &["id".to_owned()]
                )
        }));
        let fixture = fixture();
        let fragments = build_executable_fragments("query-1", &plan, &fixture.catalog, 4).unwrap();
        let scan_stage = &fragments[&StageId(0)];
        assert!(
            scan_stage
                .nodes
                .iter()
                .any(|node| matches!(node.operator, FragmentOperator::Distinct))
        );
        assert!(matches!(
            &scan_stage.nodes.last().unwrap().operator,
            FragmentOperator::ExchangeOutput(output)
                if matches!(output.partitioning, Partitioning::Hash { .. })
        ));
        // DISTINCT over an unprojected input keeps the single final task.
        let mut plan =
            kaveon_sql::logical_plan::sql_to_logical_plan("SELECT DISTINCT * FROM items").unwrap();
        qualify_tables(&mut plan, "test", "default");
        let graph = build_stage_graph("query-2", &plan, 4).unwrap();
        assert!(
            graph
                .stages
                .iter()
                .any(|stage| mentions(&stage.plan, "FinalDistinct"))
        );
    }

    #[test]
    fn set_operations_consume_every_exchange_input() {
        for (sql, expected) in [
            (
                "SELECT COUNT(*) FROM items WHERE id > 1 UNION ALL SELECT COUNT(*) FROM customers",
                "Union",
            ),
            (
                "SELECT id FROM items INTERSECT SELECT id FROM customers",
                "Intersect",
            ),
            (
                "SELECT id FROM items EXCEPT SELECT id FROM customers",
                "Except",
            ),
        ] {
            let mut plan = kaveon_sql::logical_plan::sql_to_logical_plan(sql).unwrap();
            qualify_tables(&mut plan, "test", "default");
            let fixture = fixture();
            // Building validates every fragment: an operator that leaves its
            // exchange inputs unreachable fails here.
            let fragments =
                build_executable_fragments("query-1", &plan, &fixture.catalog, 4).unwrap();
            let (_, target) = fragments
                .iter()
                .find(|(_, fragment)| {
                    fragment
                        .nodes
                        .iter()
                        .any(|node| format!("{:?}", node.operator).starts_with(expected))
                })
                .expect("set operation stage");
            let operator = target
                .nodes
                .iter()
                .find(|node| format!("{:?}", node.operator).starts_with(expected))
                .unwrap();
            let exchange_inputs = target
                .nodes
                .iter()
                .filter(|node| matches!(node.operator, FragmentOperator::ExchangeInput(_)))
                .map(|node| node.id)
                .collect::<Vec<_>>();
            assert_eq!(operator.inputs, exchange_inputs, "{sql}");
            assert_eq!(exchange_inputs.len(), 2, "{sql}");
        }
    }

    #[test]
    fn semi_and_anti_joins_broadcast_the_subquery_into_the_probe_stage() {
        for (sql, expected_type) in [
            (
                "SELECT COUNT(*) FROM items WHERE id IN (SELECT id FROM customers WHERE id > 1)",
                FragmentJoinType::Semi,
            ),
            (
                "SELECT id FROM items WHERE id NOT IN (SELECT id FROM customers GROUP BY id)",
                FragmentJoinType::Anti,
            ),
        ] {
            let mut plan = kaveon_sql::logical_plan::sql_to_logical_plan(sql).unwrap();
            qualify_tables(&mut plan, "test", "default");
            let plan = kaveon_optim::rules::push_filter_down(plan);
            let plan = kaveon_optim::rules::push_projection_down(plan);
            let fixture = fixture();
            let graph = build_stage_graph("query-1", &plan, 4).unwrap();
            assert!(graph.exchanges.iter().any(|exchange| {
                exchange.target_stage == StageId(0)
                    && matches!(exchange.partitioning, Partitioning::Broadcast)
            }));
            let fragments =
                build_executable_fragments("query-1", &plan, &fixture.catalog, 4).unwrap();
            let probe = &fragments[&StageId(0)];
            let join = probe
                .nodes
                .iter()
                .find(|node| matches!(node.operator, FragmentOperator::HashJoin(_)))
                .expect("probe stage carries the semi join");
            let FragmentOperator::HashJoin(spec) = &join.operator else {
                unreachable!()
            };
            assert_eq!(spec.join_type, expected_type);
            assert!(spec.broadcast);
            assert_eq!(spec.left_keys, vec![Expr::Column("id".into())]);
            assert!(matches!(
                probe.nodes[join.inputs[1].0 as usize].operator,
                FragmentOperator::ExchangeInput(_)
            ));
            assert!(matches!(
                probe.nodes.first().unwrap().operator,
                FragmentOperator::Scan(_)
            ));
        }
    }

    #[test]
    fn proven_small_build_is_broadcast_into_probe_scan_stage() {
        let fixture = fixture();
        let mut plan = kaveon_sql::logical_plan::sql_to_logical_plan(
            "SELECT * FROM items i JOIN customers c ON i.id = c.id",
        )
        .unwrap();
        qualify_tables(&mut plan, "test", "default");
        let plan = kaveon_optim::statistics::optimize_with_statistics(plan, &mut |table| {
            Some(kaveon_optim::statistics::RelationStatistics {
                rows: if table.ends_with("customers") {
                    10
                } else {
                    10_000
                },
                columns: vec!["id".into(), "name".into()],
            })
        });

        let graph = build_stage_graph("broadcast-query", &plan, 4).unwrap();
        assert_eq!(graph.stages.len(), 2);
        assert_eq!(graph.root_stage, StageId(0));
        assert_eq!(graph.exchanges.len(), 1);
        assert_eq!(graph.exchanges[0].source_stage, StageId(1));
        assert_eq!(graph.exchanges[0].target_stage, StageId(0));
        assert_eq!(graph.exchanges[0].partitioning, Partitioning::Broadcast);
        assert_eq!(graph.stages[0].plan.operator, "BroadcastHashJoin");

        let fragments =
            build_executable_fragments("broadcast-query", &plan, &fixture.catalog, 4).unwrap();
        assert_eq!(fragments.len(), 2);
        let probe = &fragments[&StageId(0)];
        let join = probe.nodes.last().unwrap();
        assert!(matches!(
            &join.operator,
            FragmentOperator::HashJoin(spec) if spec.broadcast
        ));
        assert!(matches!(
            fragments[&StageId(1)].nodes.last().unwrap().operator,
            FragmentOperator::ExchangeOutput(ExchangeOutput {
                partitioning: Partitioning::Broadcast,
                ..
            })
        ));
    }

    #[test]
    fn analyzed_delta_version_pins_fragments_across_add_remove_commit() {
        let mut fixture = fixture();
        let delta = fixture.directory.join("delta-events");
        fs::create_dir_all(delta.join("_delta_log")).unwrap();
        fs::copy(
            fixture.directory.join("items.parquet"),
            delta.join("old.parquet"),
        )
        .unwrap();
        fs::copy(
            fixture.directory.join("items.parquet"),
            delta.join("new.parquet"),
        )
        .unwrap();
        fs::write(
            delta.join("_delta_log/00000000000000000000.json"),
            r#"{"add":{"path":"old.parquet"}}"#,
        )
        .unwrap();
        let analyzed =
            kaveon_storage::analyze_source(delta.to_str().unwrap(), DataFormat::Delta).unwrap();
        assert_eq!(analyzed.delta_version, Some(0));

        fs::write(
            delta.join("_delta_log/00000000000000000001.json"),
            "{\"remove\":{\"path\":\"old.parquet\"}}\n{\"add\":{\"path\":\"new.parquet\"}}",
        )
        .unwrap();

        let schema = fixture
            .catalog
            .resolve_table(&TableReference::parse("test.default.items"))
            .unwrap()
            .table
            .arrow_schema
            .clone();
        let mut lake = MemoryCatalog::new(
            "lake",
            StorageType::Local {
                base_path: fixture.directory.clone(),
            },
        )
        .with_schema("default");
        lake.register_table(
            "default",
            TableMeta {
                name: "events".into(),
                arrow_schema: schema,
                location: "delta-events".into(),
                access: AccessPattern::Shortcut,
                format: DataFormat::Delta,
            },
        )
        .unwrap();
        fixture.catalog.register_catalog(Box::new(lake));
        let mut plan = kaveon_sql::logical_plan::sql_to_logical_plan(
            "SELECT * FROM test.default.items i JOIN lake.default.events e ON i.id = e.id",
        )
        .unwrap();
        qualify_tables(&mut plan, "test", "default");
        let source = fixture
            .catalog
            .resolve_table(&TableReference::parse("lake.default.events"))
            .unwrap()
            .full_path();
        let pins = BTreeMap::from([(source.clone(), analyzed.delta_version.unwrap())]);
        let pinned = build_executable_fragments_with_delta_versions(
            "pinned-delta",
            &plan,
            &fixture.catalog,
            2,
            &pins,
        )
        .unwrap();
        let fresh = build_executable_fragments("fresh-delta", &plan, &fixture.catalog, 2).unwrap();
        let version = |fragments: &BTreeMap<StageId, ExecutableFragment>| {
            fragments
                .values()
                .flat_map(|fragment| &fragment.nodes)
                .find_map(|node| match &node.operator {
                    FragmentOperator::Scan(scan) if scan.source_uri == source => scan.delta_version,
                    _ => None,
                })
        };
        assert_eq!(version(&pinned), Some(0));
        assert_eq!(version(&fresh), Some(1));

        let unrelated = BTreeMap::from([("different-source".into(), 0)]);
        let replacement = build_executable_fragments_with_delta_versions(
            "replacement-delta",
            &plan,
            &fixture.catalog,
            2,
            &unrelated,
        )
        .unwrap();
        assert_eq!(version(&replacement), Some(1));
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
        // Each side hashes on its own key, named as the join names it; the
        // partitioner resolves it against the fragment's output by bare
        // name when the fragment is a scan.
        assert_eq!(
            graph.exchanges[0].partitioning,
            Partitioning::Hash {
                columns: vec!["o.customer_id".into()],
                partition_count: 4,
            }
        );
        assert_eq!(
            graph.exchanges[1].partitioning,
            Partitioning::Hash {
                columns: vec!["c.customer_id".into()],
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
