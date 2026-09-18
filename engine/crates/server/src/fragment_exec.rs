use std::collections::{BTreeMap, HashMap, VecDeque};
use std::path::PathBuf;
use std::sync::Arc;

use arrow::array::{ArrayRef, Float64Array, UInt64Array};
use arrow::datatypes::{DataType, Field, Schema, SchemaRef};
use arrow::record_batch::RecordBatch;
use kaveon_core::{
    AggregateFunction, AggregateMode, BatchOperator, CatalogManager, DataFormat, ExchangeId,
    ExecutableFragment, Expr, FragmentNode, FragmentNodeId, FragmentOperator, KaveonError,
    Partitioning, QueryMemoryPool, Result,
};
use kaveon_exec::aggregate::{
    AggExpr, AggFunc, AggregateState, FinalAggregateValue, GroupedAggregateState, HashAggregate,
    finalize_grouped_aggregate_states, grouped_aggregate_key_types,
};
#[cfg(test)]
use kaveon_exec::aggregate::{
    grouped_aggregate_states_to_batch, grouped_aggregate_states_to_typed_batch,
};
use kaveon_exec::distinct::DistinctOperator;
use kaveon_exec::exchange::{HashPartitionMetrics, HashPartitioner};
use kaveon_exec::filter::FilterOperator;
use kaveon_exec::incremental_aggregate::{IncrementalAggregateMerger, MergedGroups};
use kaveon_exec::join::JoinType;
use kaveon_exec::limit::LimitOperator;
use kaveon_exec::local_parallel::Sources;
use kaveon_exec::offset::OffsetOperator;
use kaveon_exec::project::ProjectOperator;
use kaveon_exec::scan::ScanOperator;
use kaveon_exec::setop::{SetOpMode, SetOpOperator};
use kaveon_exec::sort::SortExpr;
use kaveon_exec::union::UnionOperator;
use kaveon_exec::window::WindowOperator;
use kaveon_storage::{
    AdlsParquetReader, DeltaTableReader, ObjectDeltaReader, ObjectParquetReader, ParquetReader,
    ScanMetrics, ScanPartition,
};

pub struct ExchangeBatches {
    pub schema: SchemaRef,
    pub batches: Vec<RecordBatch>,
}

pub trait ExchangeInputProvider {
    fn read(&self, exchange_id: &ExchangeId) -> Result<ExchangeBatches>;

    /// Opens an exchange as a batch stream. Network-backed providers override
    /// this to decode disk-spooled producer payloads one at a time; the default
    /// preserves the in-memory test/embedded provider contract.
    fn open(&self, exchange_id: &ExchangeId) -> Result<Box<dyn BatchOperator>> {
        let input = self.read(exchange_id)?;
        Ok(Box::new(BatchInput::new(input.schema, input.batches)))
    }

    /// Opens an exchange as sources that can each be read on a thread of
    /// their own — one per producer payload — for an operator whose
    /// threads take every batch. None when the provider cannot hand its
    /// input across threads, and the operator reads `open` here.
    fn open_each(&self, exchange_id: &ExchangeId) -> Result<Option<Sources>> {
        let _ = exchange_id;
        Ok(None)
    }
}

pub struct FragmentExecution {
    pub result_schema: SchemaRef,
    pub result_batches: Vec<RecordBatch>,
    pub exchange_outputs: BTreeMap<ExchangeId, ExchangeOutputBatches>,
    pub scan_metrics: Vec<kaveon_storage::ScanMetrics>,
    pub scan_metrics_complete: bool,
    pub hash_partition_metrics: HashPartitionMetrics,
}

pub struct ExchangeOutputBatches {
    pub schema: SchemaRef,
    pub partitions: Vec<Vec<RecordBatch>>,
}

pub fn execute_fragment(
    fragment: &ExecutableFragment,
    catalog: &CatalogManager,
    exchanges: &dyn ExchangeInputProvider,
    scan_partition: ScanPartition,
) -> Result<FragmentExecution> {
    execute_fragment_with_memory(fragment, catalog, exchanges, scan_partition, None)
}

pub fn execute_fragment_with_memory(
    fragment: &ExecutableFragment,
    catalog: &CatalogManager,
    exchanges: &dyn ExchangeInputProvider,
    scan_partition: ScanPartition,
    memory: Option<&QueryMemoryPool>,
) -> Result<FragmentExecution> {
    // Exchange output collected in memory: the local and test path. The
    // worker task streams it instead (execute_fragment_streaming).
    let mut partitions: Vec<Vec<RecordBatch>> = Vec::new();
    let mut sink = |partition: usize, batch: &RecordBatch| {
        if partitions.len() <= partition {
            partitions.resize_with(partition + 1, Vec::new);
        }
        partitions[partition].push(batch.clone());
        Ok(())
    };
    let mut execution = execute_fragment_streaming(
        fragment,
        catalog,
        exchanges,
        scan_partition,
        memory,
        &mut sink,
    )?;
    for output in execution.exchange_outputs.values_mut() {
        let count = output.partitions.len();
        let mut collected = std::mem::take(&mut partitions);
        collected.resize_with(count, Vec::new);
        output.partitions = collected;
    }
    Ok(execution)
}

/// Where a streamed exchange output goes: one call per (output partition,
/// batch), in production order, from the executing thread.
pub type ExchangeSink<'a> = dyn FnMut(usize, &RecordBatch) -> Result<()> + 'a;

/// Where a root fragment's result goes when it is streamed: `open` once
/// with the schema before any batch — also for a result with no rows —
/// then `write` per batch in production order, from the executing thread.
pub trait RootSink {
    fn open(&mut self, schema: &SchemaRef) -> Result<()>;
    fn write(&mut self, batch: &RecordBatch) -> Result<()>;
}

/// Execute a fragment whose root is an exchange output, handing each
/// partitioned batch to `sink` as it is produced instead of holding the
/// task's whole output — a partial aggregate's output can be as large as
/// its input. The returned execution carries the output's schema and
/// partition count with empty partitions; a fragment whose root is not an
/// exchange output returns its result batches as before.
pub fn execute_fragment_streaming(
    fragment: &ExecutableFragment,
    catalog: &CatalogManager,
    exchanges: &dyn ExchangeInputProvider,
    scan_partition: ScanPartition,
    memory: Option<&QueryMemoryPool>,
    sink: &mut ExchangeSink<'_>,
) -> Result<FragmentExecution> {
    execute_fragment_streaming_root(
        fragment,
        catalog,
        exchanges,
        scan_partition,
        memory,
        sink,
        None,
    )
}

/// `execute_fragment_streaming`, with a root fragment's result handed to
/// `root` batch by batch as it is produced when one is given: the returned
/// execution then carries the result schema with no batches. A root sink
/// is never consulted for a fragment whose root is an exchange output.
pub fn execute_fragment_streaming_root(
    fragment: &ExecutableFragment,
    catalog: &CatalogManager,
    exchanges: &dyn ExchangeInputProvider,
    scan_partition: ScanPartition,
    memory: Option<&QueryMemoryPool>,
    sink: &mut ExchangeSink<'_>,
    root_sink: Option<&mut dyn RootSink>,
) -> Result<FragmentExecution> {
    fragment.validate()?;
    let nodes = fragment
        .nodes
        .iter()
        .map(|node| (node.id, node))
        .collect::<HashMap<_, _>>();
    let root = nodes[&fragment.root];
    let scan_count = fragment
        .nodes
        .iter()
        .filter(|node| matches!(node.operator, FragmentOperator::Scan(_)))
        .count();
    let mut scan_metrics = Vec::new();
    if let FragmentOperator::ExchangeOutput(output) = &root.operator {
        let mut operator = compile_node(
            root.inputs[0],
            &nodes,
            catalog,
            exchanges,
            scan_partition,
            memory,
            &mut scan_metrics,
        )?;
        let schema = Arc::clone(operator.schema());
        let (partition_count, hash_partition_metrics) =
            stream_partitions(&mut *operator, &schema, &output.partitioning, sink)?;
        let scan_metrics_complete = has_complete_scan_metrics(scan_count, scan_metrics.len());
        return Ok(FragmentExecution {
            result_schema: Arc::clone(&schema),
            result_batches: Vec::new(),
            exchange_outputs: BTreeMap::from([(
                output.exchange_id.clone(),
                ExchangeOutputBatches {
                    schema,
                    partitions: vec![Vec::new(); partition_count],
                },
            )]),
            scan_metrics,
            scan_metrics_complete,
            hash_partition_metrics,
        });
    }
    let mut operator = compile_node(
        fragment.root,
        &nodes,
        catalog,
        exchanges,
        scan_partition,
        memory,
        &mut scan_metrics,
    )?;
    let result_schema = Arc::clone(operator.schema());
    let scan_metrics_complete = has_complete_scan_metrics(scan_count, scan_metrics.len());
    let result_batches = match root_sink {
        Some(root_sink) => {
            root_sink.open(&result_schema)?;
            while let Some(batch) = operator.next_batch()? {
                root_sink.write(&batch)?;
            }
            Vec::new()
        }
        None => collect(&mut *operator)?,
    };
    Ok(FragmentExecution {
        result_schema,
        result_batches,
        exchange_outputs: BTreeMap::new(),
        scan_metrics,
        scan_metrics_complete,
        hash_partition_metrics: HashPartitionMetrics::default(),
    })
}

fn compile_node(
    id: FragmentNodeId,
    nodes: &HashMap<FragmentNodeId, &FragmentNode>,
    catalog: &CatalogManager,
    exchanges: &dyn ExchangeInputProvider,
    scan_partition: ScanPartition,
    memory: Option<&QueryMemoryPool>,
    scan_metrics: &mut Vec<kaveon_storage::ScanMetrics>,
) -> Result<Box<dyn BatchOperator>> {
    let node = nodes[&id];
    match &node.operator {
        FragmentOperator::Scan(scan) => {
            let source: Box<dyn kaveon_core::BatchSource> = match scan.format {
                DataFormat::Parquet => {
                    if scan.source_uri.starts_with("s3://") {
                        let mut reader = ObjectParquetReader::from_uri(&scan.source_uri)?
                            .with_partition(scan_partition);
                        if !scan.projection.is_empty() {
                            reader = reader.with_columns(scan.projection.clone());
                        }
                        if let Some(predicate) = &scan.predicate {
                            reader = reader.with_predicate(predicate.clone());
                        }
                        let metrics = ScanMetrics::default();
                        reader = reader.with_metrics(metrics.clone());
                        scan_metrics.push(metrics);
                        return Ok(Box::new(ScanOperator::new(
                            Box::new(reader.read_blocking()?),
                            None,
                        )?));
                    }
                    if scan.source_uri.starts_with("abfss://") {
                        let mut reader = AdlsParquetReader::from_abfss_uri(&scan.source_uri)?
                            .with_partition(scan_partition);
                        if !scan.projection.is_empty() {
                            reader = reader.with_columns(scan.projection.clone());
                        }
                        if let Some(predicate) = &scan.predicate {
                            reader = reader.with_predicate(predicate.clone());
                        }
                        let metrics = ScanMetrics::default();
                        reader = reader.with_metrics(metrics.clone());
                        scan_metrics.push(metrics);
                        return Ok(Box::new(ScanOperator::new(
                            Box::new(reader.read_blocking()?),
                            None,
                        )?));
                    }
                    let path = local_path(&scan.source_uri)?;
                    let mut reader = ParquetReader::new(path).with_partition(scan_partition);
                    if !scan.projection.is_empty() {
                        reader = reader.with_columns(scan.projection.clone());
                    }
                    if let Some(predicate) = &scan.predicate {
                        reader = reader.with_predicate(predicate.clone());
                    }
                    let metrics = ScanMetrics::default();
                    reader = reader.with_metrics(metrics.clone());
                    scan_metrics.push(metrics);
                    Box::new(reader.read()?)
                }
                DataFormat::Delta => {
                    if scan.source_uri.starts_with("abfss://")
                        || scan.source_uri.starts_with("s3://")
                    {
                        let mut reader = ObjectDeltaReader::from_uri(&scan.source_uri)?
                            .with_partition(scan_partition);
                        if let Some(predicate) = &scan.predicate {
                            reader = reader.with_predicate(predicate.clone());
                        }
                        if let Some(version) = scan.delta_version {
                            reader = reader.with_version(version);
                        }
                        if !scan.projection.is_empty() {
                            reader = reader.with_columns(scan.projection.clone());
                        }
                        return Ok(Box::new(ScanOperator::new(
                            Box::new(reader.read_blocking()?),
                            None,
                        )?));
                    }
                    let path = local_path(&scan.source_uri)?;
                    let mut reader = DeltaTableReader::new(path).with_partition(scan_partition);
                    if let Some(predicate) = &scan.predicate {
                        reader = reader.with_predicate(predicate.clone());
                    }
                    if let Some(version) = scan.delta_version {
                        reader = reader.with_version(version);
                    }
                    if !scan.projection.is_empty() {
                        reader = reader.with_columns(scan.projection.clone());
                    }
                    Box::new(reader.read()?)
                }
                DataFormat::Iceberg => {
                    let mut reader = kaveon_storage::IcebergReader::new(&scan.source_uri)
                        .with_partition(scan_partition);
                    if let Some(id) = scan.iceberg_snapshot_id {
                        reader = reader.with_snapshot_id(id);
                    }
                    if !scan.projection.is_empty() {
                        reader = reader.with_columns(scan.projection.clone());
                    }
                    Box::new(reader.read_blocking()?)
                }
            };
            Ok(Box::new(ScanOperator::new(source, None)?))
        }
        FragmentOperator::ExchangeInput(input) => exchanges.open(&input.exchange_id),
        FragmentOperator::Filter { predicate } => {
            let mut operator = FilterOperator::new(
                compile_input(
                    node,
                    0,
                    nodes,
                    catalog,
                    exchanges,
                    scan_partition,
                    memory,
                    scan_metrics,
                )?,
                predicate.clone(),
            );
            if let Some(memory) = memory {
                operator = operator.with_memory(memory.operator("fragment-filter")?);
            }
            Ok(Box::new(operator))
        }
        FragmentOperator::Project { expressions } => {
            let mut operator = ProjectOperator::new(
                compile_input(
                    node,
                    0,
                    nodes,
                    catalog,
                    exchanges,
                    scan_partition,
                    memory,
                    scan_metrics,
                )?,
                expressions
                    .iter()
                    .map(|named| Expr::Alias {
                        expr: Box::new(named.expression.clone()),
                        name: named.name.clone(),
                    })
                    .collect(),
            )?;
            if let Some(memory) = memory {
                operator = operator.with_memory(memory.operator("fragment-project")?);
            }
            Ok(Box::new(operator))
        }
        FragmentOperator::Aggregate {
            mode,
            group_by,
            aggregates,
        } => {
            let input = compile_input(
                node,
                0,
                nodes,
                catalog,
                exchanges,
                scan_partition,
                memory,
                scan_metrics,
            )?;
            // An aggregate over rows resolves its columns against its
            // input as the node-local planner does (`t.country` over the
            // scan of `events t` is the field `country`); a final merge
            // names its decoded keys by the spec.
            let resolve_against = match mode {
                AggregateMode::Single | AggregateMode::Partial => Some(input.schema().as_ref()),
                AggregateMode::Final => None,
            };
            let (group_by, aggregates) = aggregate_bindings(group_by, aggregates, resolve_against)?;
            match mode {
                AggregateMode::Single => kaveon_exec::partitioned::hash_aggregate(
                    input,
                    group_by,
                    aggregates,
                    memory
                        .map(|memory| memory.operator("fragment-hash-aggregate"))
                        .transpose()?,
                ),
                AggregateMode::Partial => {
                    let output_types = kaveon_exec::aggregate::aggregate_output_types(
                        &aggregates,
                        input.schema(),
                    )?;
                    // Keys cross the exchange as their logical values: a
                    // dictionary-encoded string column is exchanged as Utf8.
                    // A key resolves as every column reference does: by
                    // exact name, else by its bare name (`t.country` over
                    // the scan of `events t`).
                    let group_types = group_by
                        .iter()
                        .map(|name| {
                            let schema = input.schema();
                            let index = kaveon_exec::expr_eval::resolve_column_index(schema, name)?;
                            Ok(kaveon_exec::aggregate::exchanged_group_key_type(
                                schema.field(index).data_type(),
                            ))
                        })
                        .collect::<Result<Vec<_>>>()?;
                    // Several aggregator threads per task: rows hash to the
                    // thread that owns their group, so the task's memory is
                    // one aggregator's and its CPU is all of them. Each
                    // thread's aggregate is the spill-capable one when a
                    // spill root is configured, so parallelism and the disk
                    // bound compose instead of excluding each other.
                    let parallelism = kaveon_exec::local_parallel::query_parallelism(memory)?;
                    if parallelism > 1
                        && let Some(memory) = memory
                    {
                        return Ok(Box::new(
                            kaveon_exec::local_parallel::ParallelPartials::new(
                                input,
                                group_by,
                                aggregates,
                                memory.clone(),
                                parallelism,
                            )?,
                        ));
                    }
                    if let Some(memory) = memory
                        && let Some((spill, count)) =
                            kaveon_exec::partitioned::spill_from_environment(memory)?
                    {
                        return Ok(Box::new(
                            kaveon_exec::partitioned::PartitionedHashAggregate::new_partial(
                                input,
                                group_by,
                                aggregates,
                                memory.operator("fragment-partial-hash-aggregate")?,
                                spill,
                                count,
                            )?,
                        ));
                    }
                    match memory {
                        // One thread, no spill root: flush in rounds on the
                        // query budget rather than hold every group at once.
                        Some(memory) => Ok(Box::new(
                            kaveon_exec::partitioned::FlushingPartialAggregate::new(
                                input,
                                group_by,
                                aggregates,
                                memory.operator("fragment-partial-hash-aggregate")?,
                            )?,
                        )),
                        None => {
                            let aggregate = HashAggregate::new(input, group_by, aggregates)?;
                            let (batch, state_memory) =
                                aggregate.into_partial_batch(&group_types, &output_types)?;
                            drop(state_memory);
                            Ok(Box::new(BatchInput::with_memory(
                                batch.schema(),
                                vec![batch],
                                memory,
                            )?))
                        }
                    }
                }
                AggregateMode::Final => compile_final_aggregate_parallel(
                    final_sources(node, nodes, exchanges, input)?,
                    group_by,
                    aggregates,
                    memory,
                    None,
                ),
            }
        }
        FragmentOperator::Sort { keys } => kaveon_exec::partitioned::sort_operator(
            compile_input(
                node,
                0,
                nodes,
                catalog,
                exchanges,
                scan_partition,
                memory,
                scan_metrics,
            )?,
            sort_expressions(keys),
            memory
                .map(|memory| memory.operator("fragment-sort"))
                .transpose()?,
        ),
        FragmentOperator::TopN { keys, limit } => {
            // A TopN straight over a final aggregate (through at most a
            // projection) runs inside every merge thread: each thread keeps
            // its own top rows and the merged groups never exist as a
            // whole — `ORDER BY c DESC LIMIT 10` over eighteen million
            // URLs holds ten rows a thread, not the eighteen million.
            if let Some((final_node, project)) = final_under_top_n(node, nodes) {
                let FragmentOperator::Aggregate {
                    group_by,
                    aggregates,
                    ..
                } = &final_node.operator
                else {
                    unreachable!("final_under_top_n returns an aggregate");
                };
                let (group_by, aggregates) = aggregate_bindings(group_by, aggregates, None)?;
                let input = compile_input(
                    final_node,
                    0,
                    nodes,
                    catalog,
                    exchanges,
                    scan_partition,
                    memory,
                    scan_metrics,
                )?;
                let sort = sort_expressions(keys);
                let limit = *limit;
                let tail: FinalTail = Arc::new(move |operator, memory| {
                    let operator = match &project {
                        Some(expressions) => {
                            let mut project = ProjectOperator::new(operator, expressions.clone())?;
                            if let Some(memory) = memory {
                                project = project.with_memory(memory.operator("fragment-project")?);
                            }
                            Box::new(project) as Box<dyn BatchOperator>
                        }
                        None => operator,
                    };
                    kaveon_exec::partitioned::top_n_operator(
                        operator,
                        sort.clone(),
                        limit,
                        memory
                            .map(|memory| memory.operator("fragment-topn"))
                            .transpose()?,
                    )
                });
                let merged = compile_final_aggregate_parallel(
                    final_sources(final_node, nodes, exchanges, input)?,
                    group_by,
                    aggregates,
                    memory,
                    Some(Arc::clone(&tail)),
                )?;
                // The threads' top rows union to at most threads × limit
                // rows; this TopN settles them (and is a no-op over a
                // serial merge that already applied it).
                return kaveon_exec::partitioned::top_n_operator(
                    merged,
                    sort_expressions(keys),
                    limit,
                    memory
                        .map(|memory| memory.operator("fragment-topn"))
                        .transpose()?,
                );
            }
            kaveon_exec::partitioned::top_n_operator(
                compile_input(
                    node,
                    0,
                    nodes,
                    catalog,
                    exchanges,
                    scan_partition,
                    memory,
                    scan_metrics,
                )?,
                sort_expressions(keys),
                *limit,
                memory
                    .map(|memory| memory.operator("fragment-topn"))
                    .transpose()?,
            )
        }
        FragmentOperator::Limit { limit } => Ok(Box::new(LimitOperator::new(
            compile_input(
                node,
                0,
                nodes,
                catalog,
                exchanges,
                scan_partition,
                memory,
                scan_metrics,
            )?,
            *limit,
        ))),
        FragmentOperator::Offset { offset } => Ok(Box::new(OffsetOperator::new(
            compile_input(
                node,
                0,
                nodes,
                catalog,
                exchanges,
                scan_partition,
                memory,
                scan_metrics,
            )?,
            *offset,
        ))),
        FragmentOperator::Distinct => {
            let input = compile_input(
                node,
                0,
                nodes,
                catalog,
                exchanges,
                scan_partition,
                memory,
                scan_metrics,
            )?;
            Ok(distinct_operator(input, memory)?)
        }
        FragmentOperator::Union => {
            let mut operators: Vec<Box<dyn BatchOperator>> = Vec::new();
            for &input_id in &node.inputs {
                operators.push(compile_node(
                    input_id,
                    nodes,
                    catalog,
                    exchanges,
                    scan_partition,
                    memory,
                    scan_metrics,
                )?);
            }
            if operators.is_empty() {
                return Err(exec_err("Union requires at least one input"));
            }
            Ok(Box::new(UnionOperator::new(operators)))
        }
        FragmentOperator::HashJoin(join) => {
            // A residual is a semi or anti join's: evaluated over the pairs
            // sharing a key. Inner and outer fragment joins carry none.
            let semi = matches!(
                join.join_type,
                kaveon_core::JoinType::Semi | kaveon_core::JoinType::Anti
            );
            if join.residual.is_some() && !semi {
                return Err(exec_err(
                    "residual fragment join filters are only implemented for semi and anti joins",
                ));
            }
            if semi {
                let (Some(left_key), Some(right_key)) =
                    (join.left_keys.first(), join.right_keys.first())
                else {
                    return Err(exec_err("semi join requires one key per side"));
                };
                let left = compile_input(
                    node,
                    0,
                    nodes,
                    catalog,
                    exchanges,
                    scan_partition,
                    memory,
                    scan_metrics,
                )?;
                let right = compile_input(
                    node,
                    1,
                    nodes,
                    catalog,
                    exchanges,
                    scan_partition,
                    memory,
                    scan_metrics,
                )?;
                let mut operator = kaveon_exec::semijoin::SemiJoinOperator::new(
                    left,
                    right,
                    left_key.clone(),
                    right_key.clone(),
                    join.join_type == kaveon_core::JoinType::Anti,
                )?;
                if let Some(residual) = &join.residual {
                    operator = operator.with_residual(residual.clone())?;
                }
                if let Some(memory) = memory {
                    operator = operator.with_memory(memory.operator("fragment-semi-join")?);
                }
                return Ok(Box::new(operator));
            }
            let left_keys = join
                .left_keys
                .iter()
                .map(expression_column)
                .collect::<Result<Vec<_>>>()?;
            let right_keys = join
                .right_keys
                .iter()
                .map(expression_column)
                .collect::<Result<Vec<_>>>()?;
            let left = compile_input(
                node,
                0,
                nodes,
                catalog,
                exchanges,
                scan_partition,
                memory,
                scan_metrics,
            )?;
            let right = compile_input(
                node,
                1,
                nodes,
                catalog,
                exchanges,
                scan_partition,
                memory,
                scan_metrics,
            )?;
            let keys = left_keys.into_iter().zip(right_keys).collect();
            kaveon_exec::partitioned::hash_join(
                left,
                right,
                join_type(join.join_type)?,
                keys,
                join.left_qualifier.as_deref(),
                join.right_qualifier.as_deref(),
                memory
                    .map(|memory| memory.operator("fragment-hash-join"))
                    .transpose()?,
            )
        }
        FragmentOperator::Window { window_exprs } => {
            let input = compile_input(
                node,
                0,
                nodes,
                catalog,
                exchanges,
                scan_partition,
                memory,
                scan_metrics,
            )?;
            let mut operator = WindowOperator::new(input, window_exprs.clone())?;
            if let Some(memory) = memory {
                operator = operator.with_memory(memory.operator("fragment-window")?);
            }
            Ok(Box::new(operator))
        }
        FragmentOperator::Intersect => {
            let left = compile_input(
                node,
                0,
                nodes,
                catalog,
                exchanges,
                scan_partition,
                memory,
                scan_metrics,
            )?;
            let right = compile_input(
                node,
                1,
                nodes,
                catalog,
                exchanges,
                scan_partition,
                memory,
                scan_metrics,
            )?;
            let mut operator = SetOpOperator::new(left, right, SetOpMode::Intersect);
            if let Some(memory) = memory {
                operator = operator.with_memory(memory.operator("fragment-set-operation")?);
            }
            Ok(Box::new(operator))
        }
        FragmentOperator::Except => {
            let left = compile_input(
                node,
                0,
                nodes,
                catalog,
                exchanges,
                scan_partition,
                memory,
                scan_metrics,
            )?;
            let right = compile_input(
                node,
                1,
                nodes,
                catalog,
                exchanges,
                scan_partition,
                memory,
                scan_metrics,
            )?;
            let mut operator = SetOpOperator::new(left, right, SetOpMode::Except);
            if let Some(memory) = memory {
                operator = operator.with_memory(memory.operator("fragment-set-operation")?);
            }
            Ok(Box::new(operator))
        }
        FragmentOperator::ExchangeOutput(_) => Err(exec_err(
            "ExchangeOutput is supported only as the fragment root",
        )),
    }
}

fn has_complete_scan_metrics(scan_count: usize, metric_handle_count: usize) -> bool {
    scan_count == metric_handle_count
}

/// DISTINCT over every input column: on several threads when the node
/// runs more than one and a query budget accounts for them, each thread
/// holding the rows whose values hash to it; serial otherwise.
pub(crate) fn distinct_operator(
    input: Box<dyn BatchOperator>,
    memory: Option<&QueryMemoryPool>,
) -> Result<Box<dyn BatchOperator>> {
    let parallelism = kaveon_exec::local_parallel::query_parallelism(memory)?;
    if parallelism > 1
        && let Some(memory) = memory
        && !input.schema().fields().is_empty()
    {
        let columns = input
            .schema()
            .fields()
            .iter()
            .map(|field| field.name().clone())
            .collect::<Vec<_>>();
        return kaveon_exec::local_parallel::ParallelPartials::distinct(
            input,
            columns,
            memory.clone(),
            parallelism,
        );
    }
    let mut operator = DistinctOperator::new(input);
    if let Some(memory) = memory {
        operator = operator.with_memory(memory.operator("fragment-distinct")?);
    }
    Ok(Box::new(operator))
}

/// The serial final aggregate: the hybrid merge for a grouped aggregate
/// under a budget (spilling through the query's spill when it has one),
/// the in-memory merge otherwise. Hash partitioning cannot divide a
/// global aggregate — every partial has the same empty key — so its
/// partials stream into one merger.
pub(crate) fn compile_final_aggregate(
    input: Box<dyn BatchOperator>,
    group_by: Vec<String>,
    aggregates: Vec<AggExpr>,
    memory: Option<&QueryMemoryPool>,
) -> Result<Box<dyn BatchOperator>> {
    if !group_by.is_empty()
        && let Some(memory) = memory
    {
        let spill = kaveon_exec::partitioned::spill_from_environment(memory)?;
        return hybrid_final_aggregate(
            input, group_by, aggregates, memory, spill, None, None, None,
        );
    }
    compile_final_aggregate_in_memory(input, group_by, aggregates, memory)
}

/// The grouped final aggregate a TopN node sits on, through at most one
/// projection: the final's node and the projection's expressions.
fn final_under_top_n<'a>(
    top_n: &FragmentNode,
    nodes: &HashMap<FragmentNodeId, &'a FragmentNode>,
) -> Option<(&'a FragmentNode, Option<Vec<Expr>>)> {
    let below = nodes.get(top_n.inputs.first()?)?;
    let (candidate, project) = match &below.operator {
        FragmentOperator::Project { expressions } => (
            nodes.get(below.inputs.first()?)?,
            Some(
                expressions
                    .iter()
                    .map(|named| Expr::Alias {
                        expr: Box::new(named.expression.clone()),
                        name: named.name.clone(),
                    })
                    .collect::<Vec<_>>(),
            ),
        ),
        _ => (below, None),
    };
    match &candidate.operator {
        FragmentOperator::Aggregate {
            mode: AggregateMode::Final,
            group_by,
            ..
        } if !group_by.is_empty() => Some((candidate, project)),
        _ => None,
    }
}

/// The group columns and aggregate expressions of an aggregate node.
/// The group columns and aggregate expressions of a spec. With `input`,
/// every column name resolves against it — exactly, else by its bare
/// name — to the field the operator reads.
fn aggregate_bindings(
    group_by: &[kaveon_core::NamedExpr],
    aggregates: &[kaveon_core::AggregateSpec],
    input: Option<&arrow::datatypes::Schema>,
) -> Result<(Vec<String>, Vec<AggExpr>)> {
    let resolve = |name: String| -> Result<String> {
        match input {
            Some(schema) if name != "*" => {
                let index = kaveon_exec::expr_eval::resolve_column_index(schema, &name)?;
                Ok(schema.field(index).name().clone())
            }
            _ => Ok(name),
        }
    };
    let group_by = group_by
        .iter()
        .map(|named| expression_column(&named.expression).and_then(&resolve))
        .collect::<Result<_>>()?;
    let aggregates = aggregates
        .iter()
        .map(|aggregate| {
            let (function, distinct) = match aggregate.function {
                AggregateFunction::Count => (AggFunc::Count, false),
                AggregateFunction::CountDistinct => (AggFunc::Count, true),
                AggregateFunction::Sum => (AggFunc::Sum, false),
                AggregateFunction::Min => (AggFunc::Min, false),
                AggregateFunction::Max => (AggFunc::Max, false),
                AggregateFunction::Avg => (AggFunc::Avg, false),
            };
            let column = aggregate
                .argument
                .as_ref()
                .map(expression_column)
                .transpose()?
                .map(&resolve)
                .transpose()?
                .unwrap_or_else(|| "*".into());
            let expression = AggExpr::new(function, column).with_alias(&aggregate.output);
            Ok(if distinct {
                expression.distinct()
            } else {
                expression
            })
        })
        .collect::<Result<_>>()?;
    Ok((group_by, aggregates))
}

/// What runs over a final aggregate's output inside each merge thread —
/// and over the serial merge once.
type FinalTail = Arc<
    dyn Fn(Box<dyn BatchOperator>, Option<&QueryMemoryPool>) -> Result<Box<dyn BatchOperator>>
        + Send
        + Sync,
>;

/// A final aggregate's input as sources for its merge threads: the
/// exchange's payloads, one per producer, when the input is an exchange
/// the provider can hand across threads; the compiled input otherwise.
fn final_sources(
    final_node: &FragmentNode,
    nodes: &HashMap<FragmentNodeId, &FragmentNode>,
    exchanges: &dyn ExchangeInputProvider,
    input: Box<dyn BatchOperator>,
) -> Result<Sources> {
    if let Some(id) = final_node.inputs.first()
        && let Some(below) = nodes.get(id)
        && let FragmentOperator::ExchangeInput(exchange) = &below.operator
        && let Some(sources) = exchanges.open_each(&exchange.exchange_id)?
    {
        if sources.schema() != input.schema() {
            return Err(exec_err(
                "exchange sources do not match the exchange schema",
            ));
        }
        return Ok(sources);
    }
    Ok(Sources::Here(input))
}

/// The grouped final aggregate on several threads when the node has
/// them: every thread sees every batch and keeps the rows whose encoded
/// key hashes to it, the sources read on threads of their own when the
/// exchange can hand them over — no batch is decoded, hashed or copied
/// on the calling thread. Each thread emits its finalised rows as soon
/// as its own merge is done; the hybrid merge inside every thread holds
/// what the budget admits and spills the rest through the query's
/// spill, so no refusal starts the stage over. The tail runs inside
/// each thread, over groups that are complete there. A global aggregate
/// has one group and one thread.
pub(crate) fn compile_final_aggregate_parallel(
    sources: Sources,
    group_by: Vec<String>,
    aggregates: Vec<AggExpr>,
    memory: Option<&QueryMemoryPool>,
    tail: Option<FinalTail>,
) -> Result<Box<dyn BatchOperator>> {
    let apply_tail = |operator: Box<dyn BatchOperator>,
                      memory: Option<&QueryMemoryPool>|
     -> Result<Box<dyn BatchOperator>> {
        match &tail {
            Some(tail) => tail(operator, memory),
            None => Ok(operator),
        }
    };
    let Some(pool) = memory else {
        return apply_tail(
            compile_final_aggregate_in_memory(
                sources.into_operator()?,
                group_by,
                aggregates,
                None,
            )?,
            None,
        );
    };
    if group_by.is_empty() {
        return apply_tail(
            compile_final_aggregate(sources.into_operator()?, group_by, aggregates, memory)?,
            memory,
        );
    }
    let parallelism = kaveon_exec::local_parallel::query_parallelism(memory)?;
    // One thread over a source on the calling thread: the merge reads it
    // straight. Sources on threads of their own go through the pump even
    // for one merge thread, so a source's refusal is answered by a spill
    // rather than the task's failure.
    if parallelism <= 1 && matches!(sources, Sources::Here(_)) {
        let spill = kaveon_exec::partitioned::spill_from_environment(pool)?;
        return apply_tail(
            hybrid_final_aggregate(
                sources.into_operator()?,
                group_by,
                aggregates,
                pool,
                spill,
                None,
                None,
                None,
            )?,
            memory,
        );
    }
    let parallelism = parallelism.max(1);
    let final_schema = final_schema(sources.schema(), &group_by, &aggregates)?;
    // The per-thread output schema is the tail's, found on an empty
    // operator of the final's schema.
    let schema =
        Arc::clone(apply_tail(Box::new(BatchInput::new(final_schema, Vec::new())), None)?.schema());
    let thread_tail = tail.clone();
    // No thread emits before every thread has finished merging: a
    // thread's output would otherwise compete for the budget with its
    // siblings' growing tables.
    let rendezvous = kaveon_exec::local_parallel::Rendezvous::new(parallelism);
    let operator: kaveon_exec::local_parallel::ThreadOperator =
        Arc::new(move |source, pool, context| {
            let merged = hybrid_final_aggregate(
                source,
                group_by.clone(),
                aggregates.clone(),
                pool,
                context.spill.clone(),
                Some(rendezvous.ticket()),
                Some((context.index, context.workers)),
                context
                    .pressure
                    .as_ref()
                    .map(|pressure| pressure.responder(context.index)),
            )?;
            match &thread_tail {
                Some(tail) => tail(merged, Some(pool)),
                None => Ok(merged),
            }
        });
    Ok(Box::new(
        kaveon_exec::local_parallel::ParallelPartials::broadcast(
            sources,
            schema,
            operator,
            pool.clone(),
            parallelism,
        )?,
    ))
}

/// The finalised schema of a grouped-state input.
fn final_schema(
    input: &SchemaRef,
    group_by: &[String],
    aggregates: &[AggExpr],
) -> Result<SchemaRef> {
    let group_types = grouped_aggregate_key_types(input)?;
    if group_types.len() != group_by.len() {
        return Err(exec_err("final aggregate group types do not match plan"));
    }
    let output_types = final_output_types(input, aggregates)?;
    Ok(finalized_aggregate_batch(group_by, &group_types, aggregates, &output_types, &[])?.schema())
}

/// The hybrid merge over a grouped-state input, each unit of complete
/// groups it yields finalised as one batch.
#[allow(clippy::too_many_arguments)]
fn hybrid_final_aggregate(
    input: Box<dyn BatchOperator>,
    group_by: Vec<String>,
    aggregates: Vec<AggExpr>,
    memory: &QueryMemoryPool,
    spill: Option<(kaveon_exec::spill::SpillManager, usize)>,
    rendezvous: Option<kaveon_exec::local_parallel::RendezvousTicket>,
    selection: Option<(usize, usize)>,
    pressure: Option<kaveon_exec::local_parallel::PressureTicket>,
) -> Result<Box<dyn BatchOperator>> {
    let group_types = grouped_aggregate_key_types(input.schema())?;
    if group_types.len() != group_by.len() {
        return Err(exec_err("final aggregate group types do not match plan"));
    }
    let output_types = final_output_types(input.schema(), &aggregates)?;
    let schema =
        finalized_aggregate_batch(&group_by, &group_types, &aggregates, &output_types, &[])?
            .schema();
    let account = memory.operator("fragment-final-aggregate")?;
    let mut merge = kaveon_exec::final_merge::HybridFinalMerge::new(input, account.clone(), spill)?;
    if let Some(ticket) = rendezvous {
        merge = merge.with_rendezvous(ticket);
    }
    if let Some((index, workers)) = selection {
        merge = merge.with_selection(index, workers);
    }
    if let Some(ticket) = pressure {
        merge = merge.with_pressure(ticket);
    }
    Ok(Box::new(HybridFinalOutput {
        merge,
        schema,
        group_by,
        group_types,
        aggregates,
        output_types,
        memory: account,
        current: None,
        held: None,
        emitted: false,
    }))
}

/// Rows per batch a final unit is emitted in.
const FINAL_OUTPUT_ROWS: usize = 4_096;

/// The hybrid merge's units of complete groups as finalised batches, each
/// unit in batches of `FINAL_OUTPUT_ROWS` rows built from the unit's
/// groups while they stay whole: the operator over this (a TopN, the
/// exchange writer) sees bounded batches, and the unit's memory is
/// released as its last batch is out. An empty merge is one empty batch.
struct HybridFinalOutput {
    merge: kaveon_exec::final_merge::HybridFinalMerge,
    schema: SchemaRef,
    group_by: Vec<String>,
    group_types: Vec<DataType>,
    aggregates: Vec<AggExpr>,
    output_types: Vec<DataType>,
    memory: kaveon_core::OperatorMemoryAccount,
    /// The unit being emitted, with what it holds and how far it is out.
    current: Option<FinalUnit>,
    /// The batch last emitted, held until the next call as any operator's
    /// output is.
    held: Option<kaveon_core::MemoryReservation>,
    emitted: bool,
}

struct FinalUnit {
    groups: FinalGroups,
    _reservations: Vec<kaveon_core::MemoryReservation>,
    offset: usize,
}

enum FinalGroups {
    Columnar(Box<kaveon_exec::columnar_aggregate::ColumnarGroups>),
    Rows(Vec<kaveon_exec::aggregate::FinalizedAggregateGroup>),
}

impl FinalGroups {
    fn len(&self) -> usize {
        match self {
            Self::Columnar(groups) => groups.len(),
            Self::Rows(groups) => groups.len(),
        }
    }
}

impl HybridFinalOutput {
    fn batch(&self, unit: &FinalUnit, end: usize) -> Result<RecordBatch> {
        match &unit.groups {
            FinalGroups::Columnar(groups) => {
                let (keys, outputs) = groups.final_arrays(unit.offset..end, &self.output_types)?;
                columnar_final_batch(
                    &self.group_by,
                    &self.group_types,
                    &self.aggregates,
                    &self.output_types,
                    keys,
                    outputs,
                )
            }
            FinalGroups::Rows(groups) => finalized_aggregate_batch(
                &self.group_by,
                &self.group_types,
                &self.aggregates,
                &self.output_types,
                &groups[unit.offset..end],
            ),
        }
    }
}

impl BatchOperator for HybridFinalOutput {
    fn schema(&self) -> &SchemaRef {
        &self.schema
    }

    fn next_batch(&mut self) -> Result<Option<RecordBatch>> {
        self.held = None;
        loop {
            if let Some(unit) = &self.current
                && unit.offset < unit.groups.len()
            {
                let end = (unit.offset + FINAL_OUTPUT_ROWS).min(unit.groups.len());
                let batch = self.batch(unit, end)?;
                if batch.schema() != self.schema {
                    return Err(exec_err("final aggregate output schema changed"));
                }
                self.held = Some(self.memory.reserve(batch.get_array_memory_size() as u64)?);
                self.current.as_mut().expect("checked above").offset = end;
                self.emitted = true;
                return Ok(Some(batch));
            }
            // The unit is out; its groups go before the next comes in.
            self.current = None;
            let Some((merged, reservations)) = self.merge.next_groups()? else {
                if self.emitted {
                    return Ok(None);
                }
                self.emitted = true;
                return Ok(Some(RecordBatch::new_empty(Arc::clone(&self.schema))));
            };
            let groups = match merged {
                MergedGroups::Columnar(groups) => FinalGroups::Columnar(groups),
                MergedGroups::Rows(merged) => {
                    FinalGroups::Rows(finalize_grouped_aggregate_states(&merged)?)
                }
            };
            if groups.len() == 0 {
                continue;
            }
            self.current = Some(FinalUnit {
                groups,
                _reservations: reservations,
                offset: 0,
            });
        }
    }
}

fn compile_final_aggregate_in_memory(
    mut input: Box<dyn BatchOperator>,
    group_by: Vec<String>,
    aggregates: Vec<AggExpr>,
    memory: Option<&QueryMemoryPool>,
) -> Result<Box<dyn BatchOperator>> {
    let group_types = grouped_aggregate_key_types(input.schema())?;
    let output_types = final_output_types(input.schema(), &aggregates)?;
    if group_types.len() != group_by.len() {
        return Err(exec_err("final aggregate group types do not match plan"));
    }
    let account = memory
        .map(|memory| memory.operator("fragment-final-aggregate"))
        .transpose()?;
    kaveon_exec::expr_eval::with_expression_memory(account.as_ref(), || {
        let mut merger = IncrementalAggregateMerger::new(account.clone());
        while let Some(batch) = input.next_batch()? {
            if grouped_aggregate_key_types(&batch.schema())? != group_types {
                return Err(exec_err("final aggregate input key schema changed"));
            }
            if final_output_types(&batch.schema(), &aggregates)? != output_types {
                return Err(exec_err("final aggregate input output schema changed"));
            }
            merger.push_batch(&batch)?;
        }
        let (merged, reservations) = merger.finish_groups()?;
        let mut merged = match merged {
            MergedGroups::Rows(merged) => merged,
            MergedGroups::Columnar(groups) => {
                // Consuming the table: each column is dropped as its array
                // is built.
                let (keys, outputs) = groups.into_final_arrays(&output_types)?;
                let batch = columnar_final_batch(
                    &group_by,
                    &group_types,
                    &aggregates,
                    &output_types,
                    keys,
                    outputs,
                )?;
                drop(reservations);
                return Ok(Box::new(BatchInput::with_memory(
                    batch.schema(),
                    vec![batch],
                    memory,
                )?) as Box<dyn BatchOperator>);
            }
        };
        if merged.is_empty() && group_by.is_empty() {
            merged.push(GroupedAggregateState {
                group_keys: Vec::new(),
                states: aggregates
                    .iter()
                    .zip(&output_types)
                    .map(|(agg, ty)| AggregateState::new_typed(agg, ty))
                    .collect(),
            });
        }
        let finalized = finalize_grouped_aggregate_states(&merged)?;
        let batch = finalized_aggregate_batch(
            &group_by,
            &group_types,
            &aggregates,
            &output_types,
            &finalized,
        )?;
        drop(merged);
        drop(finalized);
        drop(reservations);
        Ok(Box::new(BatchInput::with_memory(
            batch.schema(),
            vec![batch],
            memory,
        )?) as Box<dyn BatchOperator>)
    })
}

fn final_output_types(schema: &SchemaRef, aggregates: &[AggExpr]) -> Result<Vec<DataType>> {
    let types = kaveon_exec::aggregate::grouped_aggregate_output_types(schema)?;
    if types.is_empty() {
        return Ok(aggregates
            .iter()
            .map(|a| {
                if matches!(a.func, AggFunc::Count) {
                    DataType::UInt64
                } else {
                    DataType::Float64
                }
            })
            .collect());
    }
    if types.len() != aggregates.len() {
        return Err(exec_err("final aggregate output type count mismatch"));
    }
    Ok(types)
}

/// The final batch from a columnar table's finalised arrays: key columns
/// as the exchange typed them, aggregate columns from the accumulators.
fn columnar_final_batch(
    group_by: &[String],
    group_types: &[DataType],
    aggregates: &[AggExpr],
    output_types: &[DataType],
    keys: Vec<ArrayRef>,
    outputs: Vec<ArrayRef>,
) -> Result<RecordBatch> {
    let mut fields = Vec::with_capacity(group_by.len() + aggregates.len());
    let mut columns = Vec::with_capacity(group_by.len() + aggregates.len());
    if keys.len() != group_by.len() || group_types.len() != group_by.len() {
        return Err(exec_err("missing final aggregate key type"));
    }
    if output_types.len() != aggregates.len() || outputs.len() != aggregates.len() {
        return Err(exec_err(
            "final aggregate state layout does not match its plan",
        ));
    }
    for ((name, data_type), column) in group_by.iter().zip(group_types).zip(keys) {
        if column.data_type() != data_type {
            return Err(exec_err("final aggregate key type does not match plan"));
        }
        fields.push(Field::new(name, data_type.clone(), true));
        columns.push(column);
    }
    for ((aggregate, data_type), column) in aggregates.iter().zip(output_types).zip(outputs) {
        fields.push(Field::new(
            aggregate_output_name(aggregate),
            data_type.clone(),
            true,
        ));
        columns.push(column);
    }
    Ok(RecordBatch::try_new(
        Arc::new(Schema::new(fields)),
        columns,
    )?)
}

fn finalized_aggregate_batch(
    group_by: &[String],
    group_types: &[DataType],
    aggregates: &[AggExpr],
    output_types: &[DataType],
    groups: &[kaveon_exec::aggregate::FinalizedAggregateGroup],
) -> Result<RecordBatch> {
    let mut fields = Vec::with_capacity(group_by.len() + aggregates.len());
    let mut columns = Vec::with_capacity(group_by.len() + aggregates.len());
    for (index, name) in group_by.iter().enumerate() {
        let data_type = group_types
            .get(index)
            .ok_or_else(|| exec_err("missing final aggregate key type"))?;
        fields.push(Field::new(name, data_type.clone(), true));
        columns.push(group_column(groups, index, data_type)?);
    }
    for (index, aggregate) in aggregates.iter().enumerate() {
        if matches!(
            output_types[index],
            DataType::Int32 | DataType::Int64 | DataType::Date32
        ) {
            let values = groups
                .iter()
                .map(|group| match group.values.get(index) {
                    Some(FinalAggregateValue::Integer(value)) => Ok(*value),
                    _ => Err(exec_err("integer final state type mismatch")),
                })
                .collect::<Result<Vec<_>>>()?;
            let column: ArrayRef =
                if matches!(output_types[index], DataType::Int32 | DataType::Date32) {
                    let days = arrow::array::Int32Array::from(
                        values
                            .into_iter()
                            .map(|v| {
                                v.map(|n| {
                                    i32::try_from(n)
                                        .map_err(|_| exec_err("integer aggregate overflow"))
                                })
                                .transpose()
                            })
                            .collect::<Result<Vec<_>>>()?,
                    );
                    if output_types[index] == DataType::Date32 {
                        arrow::compute::cast(&days, &DataType::Date32)?
                    } else {
                        Arc::new(days)
                    }
                } else {
                    Arc::new(arrow::array::Int64Array::from(
                        values
                            .into_iter()
                            .map(|v| {
                                v.map(|n| {
                                    i64::try_from(n).map_err(|_| exec_err("integer SUM overflow"))
                                })
                                .transpose()
                            })
                            .collect::<Result<Vec<_>>>()?,
                    ))
                };
            fields.push(Field::new(
                aggregate_output_name(aggregate),
                output_types[index].clone(),
                true,
            ));
            columns.push(column);
        } else if let DataType::Decimal128(precision, scale) = output_types[index] {
            let values = groups
                .iter()
                .map(|group| match group.values.get(index) {
                    Some(FinalAggregateValue::Decimal(value, actual)) if *actual == scale => {
                        Ok(*value)
                    }
                    _ => Err(exec_err("decimal final state type/scale mismatch")),
                })
                .collect::<Result<Vec<_>>>()?;
            let array = arrow::array::Decimal128Array::from(values)
                .with_precision_and_scale(precision, scale)?;
            array.validate_decimal_precision(precision)?;
            fields.push(Field::new(
                aggregate_output_name(aggregate),
                DataType::Decimal128(precision, scale),
                true,
            ));
            columns.push(Arc::new(array) as ArrayRef);
        } else if matches!(output_types[index], DataType::Utf8 | DataType::LargeUtf8) {
            // MIN/MAX over text: the finalized value is the text itself.
            let values = groups
                .iter()
                .map(|group| match group.values.get(index) {
                    Some(FinalAggregateValue::Utf8(value)) => Ok(value.clone()),
                    _ => Err(exec_err("text final state type mismatch")),
                })
                .collect::<Result<Vec<_>>>()?;
            let column: ArrayRef = if output_types[index] == DataType::LargeUtf8 {
                Arc::new(arrow::array::LargeStringArray::from(values))
            } else {
                Arc::new(arrow::array::StringArray::from(values))
            };
            fields.push(Field::new(
                aggregate_output_name(aggregate),
                output_types[index].clone(),
                true,
            ));
            columns.push(column);
        } else if output_types[index] == DataType::UInt64
            && !matches!(aggregate.func, AggFunc::Count)
        {
            let values = groups
                .iter()
                .map(|group| match group.values.get(index) {
                    Some(FinalAggregateValue::Integer(value)) => value
                        .map(|n| {
                            u64::try_from(n).map_err(|_| exec_err("UInt64 aggregate overflow"))
                        })
                        .transpose(),
                    _ => Err(exec_err("UInt64 final state mismatch")),
                })
                .collect::<Result<Vec<_>>>()?;
            fields.push(Field::new(
                aggregate_output_name(aggregate),
                DataType::UInt64,
                true,
            ));
            columns.push(Arc::new(UInt64Array::from(values)));
        } else if matches!(aggregate.func, AggFunc::Count) {
            let values = groups
                .iter()
                .map(|group| match group.values.get(index) {
                    Some(FinalAggregateValue::Count(value)) => Ok(*value),
                    _ => Err(exec_err(
                        "final aggregate state layout does not match its plan",
                    )),
                })
                .collect::<Result<Vec<_>>>()?;
            fields.push(Field::new(
                aggregate_output_name(aggregate),
                DataType::UInt64,
                true,
            ));
            columns.push(Arc::new(UInt64Array::from(values)) as ArrayRef);
        } else {
            let values = groups
                .iter()
                .map(|group| match group.values.get(index) {
                    Some(FinalAggregateValue::Numeric(value)) => Ok(*value),
                    _ => Err(exec_err(
                        "final aggregate state layout does not match its plan",
                    )),
                })
                .collect::<Result<Vec<_>>>()?;
            fields.push(Field::new(
                aggregate_output_name(aggregate),
                DataType::Float64,
                true,
            ));
            columns.push(Arc::new(Float64Array::from(values)) as ArrayRef);
        }
    }
    Ok(RecordBatch::try_new(
        Arc::new(Schema::new(fields)),
        columns,
    )?)
}

fn aggregate_output_name(aggregate: &AggExpr) -> String {
    aggregate.output_name()
}

fn group_column(
    groups: &[kaveon_exec::aggregate::FinalizedAggregateGroup],
    index: usize,
    data_type: &DataType,
) -> Result<ArrayRef> {
    let keys = groups
        .iter()
        .map(|group| {
            group
                .group_keys
                .get(index)
                .cloned()
                .ok_or_else(|| exec_err("missing final group key"))
        })
        .collect::<Result<Vec<_>>>()?;
    kaveon_exec::aggregate::aggregate_key_column(&keys, data_type)
}

#[allow(clippy::too_many_arguments)] // Shared fragment compilation context includes measured scan handles.
fn compile_input(
    node: &FragmentNode,
    index: usize,
    nodes: &HashMap<FragmentNodeId, &FragmentNode>,
    catalog: &CatalogManager,
    exchanges: &dyn ExchangeInputProvider,
    scan_partition: ScanPartition,
    memory: Option<&QueryMemoryPool>,
    scan_metrics: &mut Vec<kaveon_storage::ScanMetrics>,
) -> Result<Box<dyn BatchOperator>> {
    compile_node(
        node.inputs[index],
        nodes,
        catalog,
        exchanges,
        scan_partition,
        memory,
        scan_metrics,
    )
}

fn expression_column(expression: &Expr) -> Result<String> {
    match expression {
        Expr::Column(column) => Ok(column.clone()),
        Expr::Star => Ok("*".into()),
        _ => Err(exec_err("fragment key must be a column reference")),
    }
}

fn sort_expressions(keys: &[kaveon_core::SortSpec]) -> Vec<SortExpr> {
    keys.iter()
        .map(|key| SortExpr::new(key.expression.clone(), key.ascending))
        .collect()
}

fn join_type(join_type: kaveon_core::JoinType) -> Result<JoinType> {
    match join_type {
        kaveon_core::JoinType::Inner => Ok(JoinType::Inner),
        kaveon_core::JoinType::Left => Ok(JoinType::Left),
        kaveon_core::JoinType::Right => Ok(JoinType::Right),
        kaveon_core::JoinType::Full => Ok(JoinType::Full),
        kaveon_core::JoinType::Cross => Ok(JoinType::Cross),
        kaveon_core::JoinType::Semi | kaveon_core::JoinType::Anti => {
            Err(exec_err("semi and anti fragment joins are not implemented"))
        }
    }
}

fn local_path(uri: &str) -> Result<PathBuf> {
    let path = uri.strip_prefix("file://").unwrap_or(uri);
    if path.is_empty() {
        return Err(exec_err("fragment scan source path cannot be empty"));
    }
    Ok(PathBuf::from(path))
}

fn collect(operator: &mut dyn BatchOperator) -> Result<Vec<RecordBatch>> {
    let mut batches = Vec::new();
    while let Some(batch) = operator.next_batch()? {
        batches.push(batch);
    }
    Ok(batches)
}

/// Drive `operator` to its end, routing every batch to its output
/// partition through `sink`. Returns the partition count and the hash
/// partitioning cost.
fn stream_partitions(
    operator: &mut dyn BatchOperator,
    schema: &SchemaRef,
    partitioning: &Partitioning,
    sink: &mut ExchangeSink<'_>,
) -> Result<(usize, HashPartitionMetrics)> {
    let mut metrics = HashPartitionMetrics::default();
    match partitioning {
        Partitioning::Single | Partitioning::Broadcast => {
            while let Some(batch) = operator.next_batch()? {
                sink(0, &batch)?;
            }
            Ok((1, metrics))
        }
        Partitioning::RoundRobin { partition_count } => {
            let mut index = 0usize;
            while let Some(batch) = operator.next_batch()? {
                sink(index % partition_count, &batch)?;
                index += 1;
            }
            Ok((*partition_count, metrics))
        }
        Partitioning::Hash {
            columns,
            partition_count,
        } => {
            let partitioner = HashPartitioner::try_new(schema, columns, *partition_count)?;
            while let Some(batch) = operator.next_batch()? {
                let (partitioned, batch_metrics) = partitioner.partition_profiled(&batch)?;
                metrics.hash_us = metrics.hash_us.saturating_add(batch_metrics.hash_us);
                metrics.copy_us = metrics.copy_us.saturating_add(batch_metrics.copy_us);
                metrics.copy_allocations = metrics
                    .copy_allocations
                    .saturating_add(batch_metrics.copy_allocations);
                metrics.copied_bytes = metrics
                    .copied_bytes
                    .saturating_add(batch_metrics.copied_bytes);
                for (partition, batch) in partitioned.into_iter().enumerate() {
                    if batch.num_rows() > 0 {
                        sink(partition, &batch)?;
                    }
                }
            }
            Ok((*partition_count, metrics))
        }
    }
}

struct BatchInput {
    schema: SchemaRef,
    batches: VecDeque<RecordBatch>,
    _memory: Vec<kaveon_core::MemoryReservation>,
}

impl BatchInput {
    fn new(schema: SchemaRef, batches: Vec<RecordBatch>) -> Self {
        Self {
            schema,
            batches: batches.into(),
            _memory: Vec::new(),
        }
    }

    fn with_memory(
        schema: SchemaRef,
        batches: Vec<RecordBatch>,
        memory: Option<&QueryMemoryPool>,
    ) -> Result<Self> {
        let mut input = Self::new(schema, batches);
        if let Some(memory) = memory {
            let account = memory.operator("fragment-retained-batches")?;
            for batch in &input.batches {
                input
                    ._memory
                    .push(account.reserve(batch.get_array_memory_size() as u64)?);
            }
        }
        Ok(input)
    }
}

impl BatchOperator for BatchInput {
    fn schema(&self) -> &SchemaRef {
        &self.schema
    }

    fn next_batch(&mut self) -> Result<Option<RecordBatch>> {
        Ok(self.batches.pop_front())
    }
}

fn exec_err(message: impl Into<String>) -> KaveonError {
    KaveonError::Execution(message.into())
}

#[cfg(test)]
mod tests {
    use kaveon_exec::aggregate::AggregateValue;
    use std::fs::{self, File};
    use std::sync::Arc;

    use arrow::array::{Array, Int64Array, UInt64Array};
    use arrow::datatypes::{DataType, Field, Schema};
    use kaveon_core::{
        AggregateSpec, BinaryOp, EXECUTABLE_FRAGMENT_VERSION, ExchangeInput, ExchangeOutput,
        FragmentNode, JoinSpec, ScalarValue, ScanSpec, ScanTable, SortSpec, StageId,
    };
    use parquet::arrow::ArrowWriter;
    use parquet::file::properties::WriterProperties;

    use super::*;

    struct Inputs {
        values: HashMap<ExchangeId, RecordBatch>,
    }

    struct BatchInputs {
        values: HashMap<ExchangeId, Vec<RecordBatch>>,
    }

    impl ExchangeInputProvider for BatchInputs {
        fn read(&self, exchange_id: &ExchangeId) -> Result<ExchangeBatches> {
            let batches = self
                .values
                .get(exchange_id)
                .ok_or_else(|| exec_err("missing test exchange"))?
                .clone();
            let schema = batches
                .first()
                .map(RecordBatch::schema)
                .ok_or_else(|| exec_err("test exchange requires a schema batch"))?;
            Ok(ExchangeBatches { schema, batches })
        }
    }

    impl ExchangeInputProvider for Inputs {
        fn read(&self, exchange_id: &ExchangeId) -> Result<ExchangeBatches> {
            let batch = self
                .values
                .get(exchange_id)
                .ok_or_else(|| exec_err("missing test exchange"))?;
            Ok(ExchangeBatches {
                schema: batch.schema(),
                batches: vec![batch.clone()],
            })
        }
    }

    fn input_batch() -> RecordBatch {
        let schema = Arc::new(Schema::new(vec![
            Field::new("key", DataType::Int64, false),
            Field::new("value", DataType::Int64, false),
        ]));
        RecordBatch::try_new(
            schema,
            vec![
                Arc::new(Int64Array::from(vec![1, 2, 1, 3])),
                Arc::new(Int64Array::from(vec![10, 40, 30, 20])),
            ],
        )
        .unwrap()
    }

    fn inputs() -> Inputs {
        Inputs {
            values: HashMap::from([(ExchangeId("input".into()), input_batch())]),
        }
    }

    fn first_partition() -> ScanPartition {
        ScanPartition::new(0, 1).unwrap()
    }

    fn node(id: u32, inputs: Vec<u32>, operator: FragmentOperator) -> FragmentNode {
        FragmentNode {
            id: FragmentNodeId(id),
            inputs: inputs.into_iter().map(FragmentNodeId).collect(),
            operator,
        }
    }

    #[test]
    fn executes_exchange_filter_top_n_and_hash_output() {
        let fragment = ExecutableFragment {
            version: EXECUTABLE_FRAGMENT_VERSION,
            stage_id: StageId(1),
            root: FragmentNodeId(4),
            nodes: vec![
                node(
                    1,
                    vec![],
                    FragmentOperator::ExchangeInput(ExchangeInput {
                        exchange_id: ExchangeId("input".into()),
                    }),
                ),
                node(
                    2,
                    vec![1],
                    FragmentOperator::Filter {
                        predicate: Expr::BinaryOp {
                            left: Box::new(Expr::Column("value".into())),
                            op: BinaryOp::Gt,
                            right: Box::new(Expr::Literal(ScalarValue::Int64(15))),
                        },
                    },
                ),
                node(
                    3,
                    vec![2],
                    FragmentOperator::TopN {
                        keys: vec![SortSpec {
                            expression: Expr::Column("value".into()),
                            ascending: false,
                            nulls_first: false,
                        }],
                        limit: 2,
                    },
                ),
                node(
                    4,
                    vec![3],
                    FragmentOperator::ExchangeOutput(ExchangeOutput {
                        exchange_id: ExchangeId("output".into()),
                        partitioning: Partitioning::Hash {
                            columns: vec!["key".into()],
                            partition_count: 2,
                        },
                    }),
                ),
            ],
        };

        let execution = execute_fragment(
            &fragment,
            &CatalogManager::new("test", "default"),
            &inputs(),
            first_partition(),
        )
        .unwrap();
        let output = &execution.exchange_outputs[&ExchangeId("output".into())];
        assert_eq!(output.schema, input_batch().schema());
        assert_eq!(output.partitions.len(), 2);
        assert_eq!(
            output
                .partitions
                .iter()
                .flatten()
                .map(RecordBatch::num_rows)
                .sum::<usize>(),
            2
        );
    }

    #[test]
    fn preserves_schema_for_empty_root_result() {
        let fragment = ExecutableFragment {
            version: EXECUTABLE_FRAGMENT_VERSION,
            stage_id: StageId(4),
            root: FragmentNodeId(2),
            nodes: vec![
                node(
                    1,
                    vec![],
                    FragmentOperator::ExchangeInput(ExchangeInput {
                        exchange_id: ExchangeId("input".into()),
                    }),
                ),
                node(
                    2,
                    vec![1],
                    FragmentOperator::Filter {
                        predicate: Expr::BinaryOp {
                            left: Box::new(Expr::Column("value".into())),
                            op: BinaryOp::Gt,
                            right: Box::new(Expr::Literal(ScalarValue::Int64(100))),
                        },
                    },
                ),
            ],
        };

        let execution = execute_fragment(
            &fragment,
            &CatalogManager::new("test", "default"),
            &inputs(),
            first_partition(),
        )
        .unwrap();
        assert_eq!(execution.result_schema, input_batch().schema());
        assert!(execution.result_batches.is_empty());
    }

    #[test]
    fn preserves_schema_for_empty_exchange_partitions() {
        let fragment = ExecutableFragment {
            version: EXECUTABLE_FRAGMENT_VERSION,
            stage_id: StageId(5),
            root: FragmentNodeId(3),
            nodes: vec![
                node(
                    1,
                    vec![],
                    FragmentOperator::ExchangeInput(ExchangeInput {
                        exchange_id: ExchangeId("input".into()),
                    }),
                ),
                node(
                    2,
                    vec![1],
                    FragmentOperator::Filter {
                        predicate: Expr::BinaryOp {
                            left: Box::new(Expr::Column("value".into())),
                            op: BinaryOp::Gt,
                            right: Box::new(Expr::Literal(ScalarValue::Int64(100))),
                        },
                    },
                ),
                node(
                    3,
                    vec![2],
                    FragmentOperator::ExchangeOutput(ExchangeOutput {
                        exchange_id: ExchangeId("empty".into()),
                        partitioning: Partitioning::Hash {
                            columns: vec!["key".into()],
                            partition_count: 2,
                        },
                    }),
                ),
            ],
        };

        let execution = execute_fragment(
            &fragment,
            &CatalogManager::new("test", "default"),
            &inputs(),
            first_partition(),
        )
        .unwrap();
        let output = &execution.exchange_outputs[&ExchangeId("empty".into())];
        assert_eq!(output.schema, input_batch().schema());
        assert_eq!(output.partitions.len(), 2);
        assert!(output.partitions.iter().all(Vec::is_empty));
    }

    #[test]
    fn executes_single_grouped_aggregate() {
        let fragment = ExecutableFragment {
            version: EXECUTABLE_FRAGMENT_VERSION,
            stage_id: StageId(2),
            root: FragmentNodeId(2),
            nodes: vec![
                node(
                    1,
                    vec![],
                    FragmentOperator::ExchangeInput(ExchangeInput {
                        exchange_id: ExchangeId("input".into()),
                    }),
                ),
                node(
                    2,
                    vec![1],
                    FragmentOperator::Aggregate {
                        mode: AggregateMode::Single,
                        group_by: vec![kaveon_core::NamedExpr {
                            name: "key".into(),
                            expression: Expr::Column("key".into()),
                        }],
                        aggregates: vec![AggregateSpec {
                            function: AggregateFunction::CountDistinct,
                            argument: Some(Expr::Column("value".into())),
                            output: "unique_values".into(),
                        }],
                    },
                ),
            ],
        };

        let execution = execute_fragment(
            &fragment,
            &CatalogManager::new("test", "default"),
            &inputs(),
            first_partition(),
        )
        .unwrap();
        assert_eq!(execution.result_batches[0].num_rows(), 3);
        let counts = execution.result_batches[0]
            .column(1)
            .as_any()
            .downcast_ref::<UInt64Array>()
            .unwrap();
        assert_eq!(counts.values().iter().sum::<u64>(), 4);
    }

    #[test]
    fn partial_and_final_aggregates_preserve_weighted_and_exact_state() {
        use arrow::array::StringArray;

        let schema = Arc::new(Schema::new(vec![
            Field::new("key", DataType::Utf8, true),
            Field::new("value", DataType::Int64, true),
        ]));
        let first = RecordBatch::try_new(
            schema.clone(),
            vec![
                Arc::new(StringArray::from(vec![
                    Some("west"),
                    Some("west"),
                    Some("west"),
                    Some("east"),
                    None,
                ])),
                Arc::new(Int64Array::from(vec![
                    Some(10),
                    Some(20),
                    None,
                    Some(5),
                    Some(7),
                ])),
            ],
        )
        .unwrap();
        let second = RecordBatch::try_new(
            schema,
            vec![
                Arc::new(StringArray::from(vec![
                    Some("west"),
                    Some("west"),
                    Some("east"),
                    None,
                ])),
                Arc::new(Int64Array::from(vec![Some(100), Some(20), Some(15), None])),
            ],
        )
        .unwrap();
        let aggregates = vec![
            AggregateSpec {
                function: AggregateFunction::Count,
                argument: Some(Expr::Column("value".into())),
                output: "count_value".into(),
            },
            AggregateSpec {
                function: AggregateFunction::Sum,
                argument: Some(Expr::Column("value".into())),
                output: "sum_value".into(),
            },
            AggregateSpec {
                function: AggregateFunction::Min,
                argument: Some(Expr::Column("value".into())),
                output: "min_value".into(),
            },
            AggregateSpec {
                function: AggregateFunction::Max,
                argument: Some(Expr::Column("value".into())),
                output: "max_value".into(),
            },
            AggregateSpec {
                function: AggregateFunction::Avg,
                argument: Some(Expr::Column("value".into())),
                output: "avg_value".into(),
            },
            AggregateSpec {
                function: AggregateFunction::CountDistinct,
                argument: Some(Expr::Column("value".into())),
                output: "distinct_value".into(),
            },
        ];
        let fragment = |mode, exchange: &str| ExecutableFragment {
            version: EXECUTABLE_FRAGMENT_VERSION,
            stage_id: StageId(20),
            root: FragmentNodeId(2),
            nodes: vec![
                node(
                    1,
                    vec![],
                    FragmentOperator::ExchangeInput(ExchangeInput {
                        exchange_id: ExchangeId(exchange.into()),
                    }),
                ),
                node(
                    2,
                    vec![1],
                    FragmentOperator::Aggregate {
                        mode,
                        group_by: vec![kaveon_core::NamedExpr {
                            name: "key".into(),
                            expression: Expr::Column("key".into()),
                        }],
                        aggregates: aggregates.clone(),
                    },
                ),
            ],
        };
        let partial = |batch| {
            execute_fragment(
                &fragment(AggregateMode::Partial, "raw"),
                &CatalogManager::new("test", "default"),
                &BatchInputs {
                    values: HashMap::from([(ExchangeId("raw".into()), vec![batch])]),
                },
                first_partition(),
            )
            .unwrap()
            .result_batches
            .remove(0)
        };
        let partials = vec![partial(first), partial(second)];
        let result = execute_fragment(
            &fragment(AggregateMode::Final, "states"),
            &CatalogManager::new("test", "default"),
            &BatchInputs {
                values: HashMap::from([(ExchangeId("states".into()), partials)]),
            },
            first_partition(),
        )
        .unwrap()
        .result_batches
        .remove(0);
        let key = result
            .column(0)
            .as_any()
            .downcast_ref::<StringArray>()
            .unwrap();
        let west = (0..result.num_rows())
            .find(|row| !key.is_null(*row) && key.value(*row) == "west")
            .unwrap();
        assert_eq!(
            result
                .column(1)
                .as_any()
                .downcast_ref::<UInt64Array>()
                .unwrap()
                .value(west),
            4
        );
        assert_eq!(
            result
                .column(2)
                .as_any()
                .downcast_ref::<Int64Array>()
                .unwrap()
                .value(west),
            150
        );
        assert_eq!(
            result
                .column(3)
                .as_any()
                .downcast_ref::<Int64Array>()
                .unwrap()
                .value(west),
            10
        );
        assert_eq!(
            result
                .column(4)
                .as_any()
                .downcast_ref::<Int64Array>()
                .unwrap()
                .value(west),
            100
        );
        assert_eq!(
            result
                .column(5)
                .as_any()
                .downcast_ref::<Float64Array>()
                .unwrap()
                .value(west),
            37.5
        );
        assert_eq!(
            result
                .column(6)
                .as_any()
                .downcast_ref::<UInt64Array>()
                .unwrap()
                .value(west),
            3
        );
        assert!((0..result.num_rows()).any(|row| key.is_null(row)));
    }

    #[test]
    fn final_empty_global_aggregate_uses_sql_identity_values() {
        let empty_state = grouped_aggregate_states_to_batch(&[]).unwrap();
        let fragment = ExecutableFragment {
            version: EXECUTABLE_FRAGMENT_VERSION,
            stage_id: StageId(21),
            root: FragmentNodeId(2),
            nodes: vec![
                node(
                    1,
                    vec![],
                    FragmentOperator::ExchangeInput(ExchangeInput {
                        exchange_id: ExchangeId("states".into()),
                    }),
                ),
                node(
                    2,
                    vec![1],
                    FragmentOperator::Aggregate {
                        mode: AggregateMode::Final,
                        group_by: vec![],
                        aggregates: vec![
                            AggregateSpec {
                                function: AggregateFunction::Count,
                                argument: None,
                                output: "rows".into(),
                            },
                            AggregateSpec {
                                function: AggregateFunction::Sum,
                                argument: Some(Expr::Column("value".into())),
                                output: "total".into(),
                            },
                        ],
                    },
                ),
            ],
        };
        let result = execute_fragment(
            &fragment,
            &CatalogManager::new("test", "default"),
            &BatchInputs {
                values: HashMap::from([(ExchangeId("states".into()), vec![empty_state])]),
            },
            first_partition(),
        )
        .unwrap()
        .result_batches
        .remove(0);
        assert_eq!(
            result
                .column(0)
                .as_any()
                .downcast_ref::<UInt64Array>()
                .unwrap()
                .value(0),
            0
        );
        assert!(result.column(1).is_null(0));
    }

    #[test]
    fn hybrid_final_aggregate_keeps_typed_null_keys_through_a_spill_and_bounds_merge_memory() {
        // 120 000 keys (one of them NULL) arriving twice, under a budget
        // that holds a fraction of them: the merge spills its table on
        // every refusal and merges each sub-partition back, so every key
        // — the NULL included, typed as the exchange typed it — counts
        // two, and the peak stays under the budget.
        let mut batches = Vec::new();
        for _ in 0..2 {
            for start in (0..120_000).step_by(4_000) {
                let groups = (start..start + 4_000)
                    .map(|key| GroupedAggregateState {
                        group_keys: vec![if key == 0 {
                            AggregateValue::Null
                        } else {
                            AggregateValue::Int64(key)
                        }],
                        states: vec![AggregateState::Count(1)],
                    })
                    .collect::<Vec<_>>();
                batches.push(
                    grouped_aggregate_states_to_typed_batch(&groups, &[DataType::Int64]).unwrap(),
                );
            }
        }
        let schema = batches[0].schema();
        let budget = 2 * 1024 * 1024;
        let pool = QueryMemoryPool::new("final-spill", budget).unwrap();
        let expressions = vec![AggExpr::new(AggFunc::Count, "*").with_alias("count")];
        let spill = kaveon_exec::spill::SpillManager::new(
            std::env::temp_dir().join("kaveon-final-spill-tests"),
            64 * 1024 * 1024,
        )
        .unwrap();
        let mut operator = hybrid_final_aggregate(
            Box::new(BatchInput::new(schema, batches)),
            vec!["key".into()],
            expressions,
            &pool,
            Some((spill.clone(), 16)),
            None,
            None,
            None,
        )
        .unwrap();
        let mut rows = 0;
        let mut nulls = 0;
        let mut units = 0;
        while let Some(batch) = operator.next_batch().unwrap() {
            units += 1;
            assert_eq!(batch.schema().field(0).data_type(), &DataType::Int64);
            rows += batch.num_rows();
            nulls += batch.column(0).null_count();
            let counts = batch
                .column(1)
                .as_any()
                .downcast_ref::<UInt64Array>()
                .unwrap();
            assert!(counts.values().iter().all(|count| *count == 2));
        }
        assert_eq!((rows, nulls), (120_000, 1));
        assert!(units > 1, "the budget made the merge spill");
        drop(operator);
        assert_eq!(pool.snapshot().current_bytes, 0);
        assert!(pool.snapshot().peak_bytes <= budget);
        let snapshot = spill.snapshot();
        assert!(snapshot.runs_written > 0);
        assert_eq!(snapshot.compactions, 0);
        assert_eq!(snapshot.current_bytes, 0);
    }

    #[test]
    fn parallel_final_with_a_top_n_tail_matches_the_serial_merge_and_spills_on_refusal() {
        // 400 000 keys in partial rows arriving twice: the final merges on
        // several threads with the TopN inside each, and the union settles
        // to the same top rows as a serial merge and sort. Under a budget
        // the merge cannot hold, each thread spills its table and merges
        // its sub-partitions back; the TopN inside the thread sees every
        // group once, complete.
        let mut batches = Vec::new();
        for round in 0..2i64 {
            for start in (0..400_000).step_by(8_192) {
                let groups = (start..(start + 8_192).min(400_000))
                    .map(|key| GroupedAggregateState {
                        group_keys: vec![AggregateValue::Int64(key)],
                        states: vec![AggregateState::Count((key % 7 + round) as u64 + 1)],
                    })
                    .collect::<Vec<_>>();
                batches.push(
                    grouped_aggregate_states_to_typed_batch(&groups, &[DataType::Int64]).unwrap(),
                );
            }
        }
        let schema = batches[0].schema();
        let expressions = vec![AggExpr::new(AggFunc::Count, "*").with_alias("count")];
        let sort = vec![SortExpr::new(Expr::Column("count".into()), false)];
        let expected = {
            let merged = compile_final_aggregate_in_memory(
                Box::new(BatchInput::new(schema.clone(), batches.clone())),
                vec!["key".into()],
                expressions.clone(),
                None,
            )
            .unwrap();
            let mut top =
                kaveon_exec::partitioned::top_n_operator(merged, sort.clone(), 3, None).unwrap();
            top.next_batch().unwrap().unwrap()
        };
        let counts = |batch: &RecordBatch| {
            batch
                .column(1)
                .as_any()
                .downcast_ref::<UInt64Array>()
                .unwrap()
                .values()
                .to_vec()
        };
        assert_eq!(counts(&expected), vec![15, 15, 15]);
        let tail: FinalTail = Arc::new({
            let sort = sort.clone();
            move |operator, memory| {
                kaveon_exec::partitioned::top_n_operator(
                    operator,
                    sort.clone(),
                    3,
                    memory.map(|m| m.operator("topn")).transpose()?,
                )
            }
        });
        // The rows from one source on the calling thread, and from three
        // sources read on threads of their own (the exchange's payloads).
        let here = |batches: &Vec<RecordBatch>| {
            Sources::Here(Box::new(BatchInput::new(schema.clone(), batches.clone())))
        };
        let threads = |batches: &Vec<RecordBatch>| Sources::Threads {
            schema: schema.clone(),
            openers: batches
                .chunks(batches.len().div_ceil(3))
                .map(|chunk| {
                    let schema = schema.clone();
                    let chunk = chunk.to_vec();
                    Box::new(move || {
                        Ok(Box::new(kaveon_exec::local_parallel::Unreserved(Box::new(
                            BatchInput::new(schema, chunk),
                        )))
                            as Box<dyn kaveon_exec::local_parallel::ThreadSource>)
                    }) as kaveon_exec::local_parallel::SourceOpener
                })
                .collect(),
        };
        type SourcesOf<'a> = &'a dyn Fn(&Vec<RecordBatch>) -> Sources;
        let cases: [(&str, u64, SourcesOf<'_>); 4] = [
            ("roomy", 128 * 1024 * 1024, &here),
            ("tight", 12 * 1024 * 1024, &here),
            ("roomy", 128 * 1024 * 1024, &threads),
            ("tight", 12 * 1024 * 1024, &threads),
        ];
        for (name, budget, sources) in cases {
            let pool = QueryMemoryPool::new(name, budget).unwrap();
            let spill_root = std::env::temp_dir().join(format!("kaveon-final-tail-{name}"));
            let spill =
                kaveon_exec::spill::SpillManager::new(&spill_root, 1024 * 1024 * 1024).unwrap();
            // The tight budget cannot hold the merge in memory, so the
            // spill root stands in for the environment's.
            pool.shared_resource("kaveon.exec.hash-spill.v1", || Ok((spill.clone(), 16usize)))
                .unwrap();
            let merged = compile_final_aggregate_parallel(
                sources(&batches),
                vec!["key".into()],
                expressions.clone(),
                Some(&pool),
                Some(Arc::clone(&tail)),
            )
            .unwrap();
            let mut top = kaveon_exec::partitioned::top_n_operator(
                merged,
                sort.clone(),
                3,
                Some(pool.operator("outer").unwrap()),
            )
            .unwrap();
            let batch = top.next_batch().unwrap().unwrap();
            assert_eq!(counts(&batch), vec![15, 15, 15], "{name}");
            assert!(top.next_batch().unwrap().is_none());
            drop(top);
            assert_eq!(pool.snapshot().current_bytes, 0, "{name}");
            let snapshot = spill.snapshot();
            assert_eq!(snapshot.current_bytes, 0, "{name}");
            if name == "tight" {
                assert!(snapshot.runs_written > 0, "the tight budget spilled");
                assert_eq!(snapshot.compactions, 0, "{name}");
            } else {
                assert_eq!(snapshot.runs_written, 0, "the roomy budget held it");
            }
        }
    }

    #[test]
    fn final_aggregate_repeated_scalar_key_merges_without_retaining_input() {
        let batch = grouped_aggregate_states_to_typed_batch(
            &[GroupedAggregateState {
                group_keys: vec![AggregateValue::Int64(1)],
                states: vec![AggregateState::Count(1)],
            }],
            &[DataType::Int64],
        )
        .unwrap();
        let pool = QueryMemoryPool::new("final-skew", 1024 * 1024).unwrap();
        let spill = kaveon_exec::spill::SpillManager::new(
            std::env::temp_dir().join("kaveon-final-spill-tests"),
            64 * 1024 * 1024,
        )
        .unwrap();
        let mut operator = hybrid_final_aggregate(
            Box::new(BatchInput::new(batch.schema(), vec![batch; 100])),
            vec!["key".into()],
            vec![AggExpr::new(AggFunc::Count, "*")],
            &pool,
            Some((spill.clone(), 16)),
            None,
            None,
            None,
        )
        .unwrap();
        let output = operator.next_batch().unwrap().unwrap();
        assert_eq!(
            output
                .column(1)
                .as_any()
                .downcast_ref::<UInt64Array>()
                .unwrap()
                .value(0),
            100
        );
        assert!(operator.next_batch().unwrap().is_none());
        assert_eq!(pool.snapshot().current_bytes, 0);
        assert_eq!(spill.snapshot().current_bytes, 0);
    }

    #[test]
    fn global_final_aggregate_streams_without_hash_partition_spill() {
        let batches = (0..4)
            .map(|value| {
                grouped_aggregate_states_to_typed_batch(
                    &[GroupedAggregateState {
                        group_keys: vec![],
                        states: vec![AggregateState::CountDistinct(
                            std::collections::HashSet::from([AggregateValue::Int64(value)]),
                        )],
                    }],
                    &[],
                )
                .unwrap()
            })
            .collect::<Vec<_>>();
        let schema = batches[0].schema();
        let pool = QueryMemoryPool::new("global-final", 1024 * 1024).unwrap();
        let mut output = compile_final_aggregate(
            Box::new(BatchInput::new(schema, batches)),
            vec![],
            vec![AggExpr::new(AggFunc::Count, "value").distinct()],
            Some(&pool),
        )
        .unwrap();

        let batch = output.next_batch().unwrap().unwrap();
        assert_eq!(
            batch
                .column(0)
                .as_any()
                .downcast_ref::<UInt64Array>()
                .unwrap()
                .value(0),
            4
        );
        assert!(output.next_batch().unwrap().is_none());
        drop(output);
        assert_eq!(pool.snapshot().current_bytes, 0);
    }

    #[test]
    fn final_grouped_aggregate_preserves_empty_and_all_null_key_types() {
        for data_type in [
            DataType::Int32,
            DataType::Int64,
            DataType::Boolean,
            DataType::Float64,
            DataType::LargeUtf8,
            DataType::UInt64,
            DataType::Decimal128(38, 7),
            DataType::Date32,
            DataType::Date64,
            DataType::Timestamp(arrow::datatypes::TimeUnit::Nanosecond, Some("UTC".into())),
        ] {
            for null_group in [false, true] {
                let groups = if null_group {
                    vec![GroupedAggregateState {
                        group_keys: vec![AggregateValue::Null],
                        states: vec![AggregateState::Count(2)],
                    }]
                } else {
                    vec![]
                };
                let batch = grouped_aggregate_states_to_typed_batch(
                    &groups,
                    std::slice::from_ref(&data_type),
                )
                .unwrap();
                let input = Box::new(BatchInput::new(batch.schema(), vec![batch]));
                let mut output = compile_final_aggregate(
                    input,
                    vec!["key".into()],
                    vec![AggExpr::new(AggFunc::Count, "*")],
                    None,
                )
                .unwrap();
                assert_eq!(output.schema().field(0).data_type(), &data_type);
                let batch = output.next_batch().unwrap().unwrap();
                assert_eq!(batch.num_rows(), usize::from(null_group));
                if null_group {
                    assert_eq!(batch.column(0).null_count(), 1);
                }
            }
        }
    }

    #[test]
    fn final_grouped_aggregate_rejects_mixed_key_schemas() {
        let first = grouped_aggregate_states_to_typed_batch(&[], &[DataType::Int64]).unwrap();
        let second = grouped_aggregate_states_to_typed_batch(&[], &[DataType::Utf8]).unwrap();
        let input = Box::new(BatchInput::new(first.schema(), vec![first, second]));
        assert!(
            compile_final_aggregate(
                input,
                vec!["key".into()],
                vec![AggExpr::new(AggFunc::Count, "*")],
                None
            )
            .is_err()
        );
    }

    #[test]
    fn local_parallel_lazy_final_matches_serial_results() {
        use kaveon_exec::local_parallel::{LazyFinalAggregate, ParallelPartials};
        let batch = RecordBatch::try_from_iter(vec![(
            "v",
            Arc::new(UInt64Array::from(vec![1; 100000])) as ArrayRef,
        )])
        .unwrap();
        let expressions = vec![
            AggExpr::new(AggFunc::Sum, "v").with_alias("total"),
            AggExpr::new(AggFunc::Count, "v")
                .distinct()
                .with_alias("distinct_count"),
        ];
        let probe = HashAggregate::new(
            Box::new(BatchInput::new(batch.schema(), vec![])),
            vec![],
            expressions.clone(),
        )
        .unwrap();
        let pool = QueryMemoryPool::new("local-parallel-final", 4 * 1024 * 1024).unwrap();
        let partials = ParallelPartials::new(
            Box::new(BatchInput::new(batch.schema(), vec![batch])),
            vec![],
            expressions.clone(),
            pool.clone(),
            4,
        )
        .unwrap();
        let merge_pool = pool.clone();
        let mut operator = LazyFinalAggregate::new(
            probe.schema().clone(),
            Box::new(partials),
            Box::new(move |input| {
                compile_final_aggregate(input, vec![], expressions, Some(&merge_pool))
            }),
        );
        assert_eq!(pool.snapshot().current_bytes, 0);
        let result = operator.next_batch().unwrap().unwrap();
        assert_eq!(result.schema(), *operator.schema());
        assert_eq!(
            result
                .column(0)
                .as_any()
                .downcast_ref::<UInt64Array>()
                .unwrap()
                .value(0),
            100000
        );
        assert_eq!(
            result
                .column(1)
                .as_any()
                .downcast_ref::<UInt64Array>()
                .unwrap()
                .value(0),
            1
        );
        assert!(operator.next_batch().unwrap().is_none());
        drop(operator);
        assert_eq!(pool.snapshot().current_bytes, 0);
    }

    #[test]
    fn exact_unsigned_and_decimal_final_results_keep_types_and_overflow_checks() {
        use kaveon_exec::aggregate::{
            aggregate_output_types, grouped_aggregate_states_to_schema_batch,
        };
        let arrays: Vec<ArrayRef> = vec![
            Arc::new(UInt64Array::from(vec![
                Some(u64::MAX - 2),
                Some(1),
                Some(1),
                None,
            ])),
            Arc::new(
                arrow::array::Decimal128Array::from(vec![
                    Some(100000000000000000001i128),
                    Some(-100000000000000000000i128),
                    Some(2),
                    Some(2),
                    None,
                ])
                .with_precision_and_scale(30, 7)
                .unwrap(),
            ),
        ];
        for array in arrays {
            let input = RecordBatch::try_from_iter(vec![("v", array)]).unwrap();
            let expressions = vec![
                AggExpr::new(AggFunc::Sum, "v").with_alias("total"),
                AggExpr::new(AggFunc::Min, "v").with_alias("low"),
                AggExpr::new(AggFunc::Max, "v").with_alias("high"),
                AggExpr::new(AggFunc::Sum, "v")
                    .distinct()
                    .with_alias("unique_total"),
            ];
            let types = aggregate_output_types(&expressions, &input.schema()).unwrap();
            let mut partials = Vec::new();
            for row in 0..input.num_rows() {
                let groups = HashAggregate::new(
                    Box::new(BatchInput::new(input.schema(), vec![input.slice(row, 1)])),
                    vec![],
                    expressions.clone(),
                )
                .unwrap()
                .into_grouped_states()
                .unwrap();
                partials
                    .push(grouped_aggregate_states_to_schema_batch(&groups, &[], &types).unwrap());
            }
            let mut final_operator = compile_final_aggregate_in_memory(
                Box::new(BatchInput::new(partials[0].schema(), partials)),
                vec![],
                expressions.clone(),
                None,
            )
            .unwrap();
            let result = final_operator.next_batch().unwrap().unwrap();
            assert_eq!(
                result
                    .schema()
                    .fields()
                    .iter()
                    .map(|f| f.data_type().clone())
                    .collect::<Vec<_>>(),
                types
            );
            if types[0] == DataType::UInt64 {
                assert_eq!(
                    result
                        .column(0)
                        .as_any()
                        .downcast_ref::<UInt64Array>()
                        .unwrap()
                        .value(0),
                    u64::MAX
                );
                assert_eq!(
                    result
                        .column(3)
                        .as_any()
                        .downcast_ref::<UInt64Array>()
                        .unwrap()
                        .value(0),
                    u64::MAX - 1
                );
            } else {
                assert_eq!(
                    result
                        .column(0)
                        .as_any()
                        .downcast_ref::<arrow::array::Decimal128Array>()
                        .unwrap()
                        .value(0),
                    5
                );
                assert_eq!(
                    result
                        .column(3)
                        .as_any()
                        .downcast_ref::<arrow::array::Decimal128Array>()
                        .unwrap()
                        .value(0),
                    3
                );
            }
            let empty = grouped_aggregate_states_to_schema_batch(&[], &[], &types).unwrap();
            let mut empty_final = compile_final_aggregate_in_memory(
                Box::new(BatchInput::new(empty.schema(), vec![empty])),
                vec![],
                expressions,
                None,
            )
            .unwrap();
            let result = empty_final.next_batch().unwrap().unwrap();
            assert!(result.columns().iter().all(|a| a.is_null(0)));
        }
    }

    #[test]
    fn decimal_final_aggregate_preserves_typed_empty_and_large_values() {
        for value in [None, Some(100000000000000000001i128)] {
            let groups = value
                .map(|sum| {
                    vec![GroupedAggregateState {
                        group_keys: vec![],
                        states: vec![AggregateState::DecimalSum {
                            sum,
                            count: 1,
                            scale: 4,
                        }],
                    }]
                })
                .unwrap_or_default();
            let batch = kaveon_exec::aggregate::grouped_aggregate_states_to_schema_batch(
                &groups,
                &[],
                &[DataType::Decimal128(38, 4)],
            )
            .unwrap();
            let input = Box::new(BatchInput::new(batch.schema(), vec![batch]));
            let mut output =
                compile_final_aggregate(input, vec![], vec![AggExpr::new(AggFunc::Sum, "v")], None)
                    .unwrap();
            assert_eq!(
                output.schema().field(0).data_type(),
                &DataType::Decimal128(38, 4)
            );
            let batch = output.next_batch().unwrap().unwrap();
            let col = batch
                .column(0)
                .as_any()
                .downcast_ref::<arrow::array::Decimal128Array>()
                .unwrap();
            assert_eq!(col.iter().collect::<Vec<_>>(), vec![value]);
        }
    }

    #[test]
    fn rejects_residual_and_unsupported_join_modes_explicitly() {
        let result = join_type(kaveon_core::JoinType::Semi);
        assert!(result.is_err());
        // An inner join carries no residual.
        let fragment = ExecutableFragment {
            version: EXECUTABLE_FRAGMENT_VERSION,
            stage_id: StageId(1),
            root: FragmentNodeId(2),
            nodes: vec![
                node(
                    0,
                    vec![],
                    FragmentOperator::ExchangeInput(ExchangeInput {
                        exchange_id: ExchangeId("input".into()),
                    }),
                ),
                node(
                    1,
                    vec![],
                    FragmentOperator::ExchangeInput(ExchangeInput {
                        exchange_id: ExchangeId("input".into()),
                    }),
                ),
                node(
                    2,
                    vec![0, 1],
                    FragmentOperator::HashJoin(JoinSpec {
                        left_qualifier: None,
                        right_qualifier: None,
                        join_type: kaveon_core::JoinType::Inner,
                        left_keys: vec![Expr::Column("key".into())],
                        right_keys: vec![Expr::Column("key".into())],
                        residual: Some(Expr::Literal(ScalarValue::Bool(true))),
                        broadcast: false,
                    }),
                ),
            ],
        };
        let error = execute_fragment(
            &fragment,
            &CatalogManager::new("test", "default"),
            &inputs(),
            first_partition(),
        )
        .err()
        .expect("an inner join with a residual is refused")
        .to_string();
        assert!(
            error.contains("only implemented for semi and anti joins"),
            "{error}"
        );
    }

    /// A semi join fragment whose residual compares a probe column with a
    /// build column: the probe keeps a row when a build row sharing its
    /// key satisfies the residual, and the anti join keeps the rest.
    #[test]
    fn semi_join_fragments_evaluate_a_residual_over_matching_pairs() {
        let build_schema = Arc::new(Schema::new(vec![
            Field::new("key", DataType::Int64, false),
            Field::new("__kaveon_corr_0", DataType::Int64, false),
        ]));
        let build = RecordBatch::try_new(
            build_schema,
            vec![
                Arc::new(Int64Array::from(vec![1, 1, 2, 3])),
                Arc::new(Int64Array::from(vec![10, 30, 40, 20])),
            ],
        )
        .unwrap();
        let fragment = |anti: bool| ExecutableFragment {
            version: EXECUTABLE_FRAGMENT_VERSION,
            stage_id: StageId(1),
            root: FragmentNodeId(2),
            nodes: vec![
                node(
                    0,
                    vec![],
                    FragmentOperator::ExchangeInput(ExchangeInput {
                        exchange_id: ExchangeId("input".into()),
                    }),
                ),
                node(
                    1,
                    vec![],
                    FragmentOperator::ExchangeInput(ExchangeInput {
                        exchange_id: ExchangeId("build".into()),
                    }),
                ),
                node(
                    2,
                    vec![0, 1],
                    FragmentOperator::HashJoin(JoinSpec {
                        left_qualifier: None,
                        right_qualifier: None,
                        join_type: if anti {
                            kaveon_core::JoinType::Anti
                        } else {
                            kaveon_core::JoinType::Semi
                        },
                        left_keys: vec![Expr::Column("key".into())],
                        right_keys: vec![Expr::Column("key".into())],
                        residual: Some(Expr::BinaryOp {
                            left: Box::new(Expr::Column("__kaveon_corr_0".into())),
                            op: BinaryOp::Ne,
                            right: Box::new(Expr::Column("value".into())),
                        }),
                        broadcast: true,
                    }),
                ),
            ],
        };
        let inputs = Inputs {
            values: HashMap::from([
                (ExchangeId("input".into()), input_batch()),
                (ExchangeId("build".into()), build),
            ]),
        };
        let values = |anti: bool| {
            let execution = execute_fragment(
                &fragment(anti),
                &CatalogManager::new("test", "default"),
                &inputs,
                first_partition(),
            )
            .unwrap();
            execution
                .result_batches
                .iter()
                .flat_map(|batch| {
                    batch
                        .column(1)
                        .as_any()
                        .downcast_ref::<Int64Array>()
                        .unwrap()
                        .values()
                        .to_vec()
                })
                .collect::<Vec<_>>()
        };
        // Probe rows (key, value): (1, 10) (2, 40) (1, 30) (3, 20). Key 1
        // holds 10 and 30 on the build side, so each of its probe rows has
        // a differing pair; keys 2 and 3 each pair only with their own
        // value.
        assert_eq!(values(false), vec![10, 30]);
        assert_eq!(values(true), vec![40, 20]);
    }

    #[test]
    fn applies_execution_partition_to_parquet_scan() {
        let directory =
            std::env::temp_dir().join(format!("kaveon-fragment-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&directory).unwrap();
        let path = directory.join("items.parquet");
        let schema = Arc::new(Schema::new(vec![Field::new(
            "value",
            DataType::Int64,
            false,
        )]));
        let batch = RecordBatch::try_new(
            Arc::clone(&schema),
            vec![Arc::new(Int64Array::from(vec![10, 20, 30, 40]))],
        )
        .unwrap();
        let properties = WriterProperties::builder()
            .set_max_row_group_size(2)
            .build();
        let mut writer = ArrowWriter::try_new(
            File::create(&path).unwrap(),
            Arc::clone(&schema),
            Some(properties),
        )
        .unwrap();
        writer.write(&batch).unwrap();
        writer.close().unwrap();

        let catalog = CatalogManager::new("missing", "missing");
        let fragment = ExecutableFragment {
            version: EXECUTABLE_FRAGMENT_VERSION,
            stage_id: StageId(3),
            root: FragmentNodeId(1),
            nodes: vec![node(
                1,
                vec![],
                FragmentOperator::Scan(ScanSpec {
                    source_uri: path.to_string_lossy().into_owned(),
                    format: DataFormat::Parquet,
                    delta_version: None,
                    iceberg_snapshot_id: None,
                    table: ScanTable {
                        catalog: "test".into(),
                        schema: "default".into(),
                        table: "items".into(),
                    },
                    projection: vec![],
                    predicate: None,
                }),
            )],
        };

        let first = execute_fragment(
            &fragment,
            &catalog,
            &inputs(),
            ScanPartition::new(0, 2).unwrap(),
        )
        .unwrap();
        let second = execute_fragment(
            &fragment,
            &catalog,
            &inputs(),
            ScanPartition::new(1, 2).unwrap(),
        )
        .unwrap();
        let first_rows = first
            .result_batches
            .iter()
            .map(RecordBatch::num_rows)
            .sum::<usize>();
        let second_rows = second
            .result_batches
            .iter()
            .map(RecordBatch::num_rows)
            .sum::<usize>();
        assert_eq!((first_rows, second_rows), (2, 2));
        assert_eq!(first.scan_metrics.len(), 1);
        assert_eq!(first.scan_metrics[0].snapshot().rows_emitted, 2);
        assert_eq!(second.scan_metrics[0].snapshot().rows_emitted, 2);

        fs::remove_dir_all(directory).unwrap();
    }

    /// A fragment whose source is a directory of Parquet files: every task
    /// lists it under the same rule and takes its own files, the rows come
    /// out once across the tasks, and the file counters count files.
    #[test]
    fn a_directory_source_is_read_once_across_the_tasks() {
        let directory =
            std::env::temp_dir().join(format!("kaveon-fragment-{}", uuid::Uuid::new_v4()));
        let table = directory.join("items");
        fs::create_dir_all(table.join("_delta_log")).unwrap();
        let schema = Arc::new(Schema::new(vec![Field::new(
            "value",
            DataType::Int64,
            false,
        )]));
        let mut expected = Vec::new();
        for (name, values, row_group_size) in [
            ("part-1.parquet", (0..8).collect::<Vec<i64>>(), 8),
            ("part-0.parquet", (8..16).collect(), 8),
            ("part-2.parquet", (16..48).collect(), 4),
        ] {
            expected.extend(values.iter().copied());
            let batch = RecordBatch::try_new(
                Arc::clone(&schema),
                vec![Arc::new(Int64Array::from(values))],
            )
            .unwrap();
            let properties = WriterProperties::builder()
                .set_max_row_group_size(row_group_size)
                .build();
            let mut writer = ArrowWriter::try_new(
                File::create(table.join(name)).unwrap(),
                Arc::clone(&schema),
                Some(properties),
            )
            .unwrap();
            writer.write(&batch).unwrap();
            writer.close().unwrap();
        }
        fs::write(table.join("_SUCCESS"), b"").unwrap();
        fs::write(table.join("_delta_log").join("stale.json"), b"{}").unwrap();
        expected.sort_unstable();

        let catalog = CatalogManager::new("missing", "missing");
        let fragment = ExecutableFragment {
            version: EXECUTABLE_FRAGMENT_VERSION,
            stage_id: StageId(3),
            root: FragmentNodeId(1),
            nodes: vec![node(
                1,
                vec![],
                FragmentOperator::Scan(ScanSpec {
                    source_uri: table.to_string_lossy().into_owned(),
                    format: DataFormat::Parquet,
                    delta_version: None,
                    iceberg_snapshot_id: None,
                    table: ScanTable {
                        catalog: "test".into(),
                        schema: "default".into(),
                        table: "items".into(),
                    },
                    projection: vec!["value".into()],
                    predicate: None,
                }),
            )],
        };
        let mut seen = Vec::new();
        let mut files_opened = 0;
        for index in 0..3 {
            let execution = execute_fragment(
                &fragment,
                &catalog,
                &inputs(),
                ScanPartition::new(index, 3).unwrap(),
            )
            .unwrap();
            for batch in &execution.result_batches {
                seen.extend(
                    batch
                        .column(0)
                        .as_any()
                        .downcast_ref::<Int64Array>()
                        .unwrap()
                        .values()
                        .iter()
                        .copied(),
                );
            }
            assert_eq!(execution.scan_metrics.len(), 1);
            let snapshot = execution.scan_metrics[0].snapshot();
            assert_eq!(snapshot.files_opened, snapshot.files_considered);
            files_opened += snapshot.files_opened;
        }
        seen.sort_unstable();
        assert_eq!(seen, expected);
        // The large file is split by row group across the three tasks and the
        // two small ones are read whole by one task each.
        assert_eq!(files_opened, 5);

        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn scan_metric_coverage_rejects_unsupported_readers_but_accepts_exchange_only_fragments() {
        // Delta and Iceberg fragments currently have scan operators without injectable handles.
        assert!(!has_complete_scan_metrics(1, 0));
        // An exchange-only stage has no storage reader and is complete at zero counters.
        assert!(has_complete_scan_metrics(0, 0));
        // Local Parquet retains one reader handle for each scan operator.
        assert!(has_complete_scan_metrics(1, 1));
    }
}
