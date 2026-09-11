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
    grouped_aggregate_states_from_batches, grouped_aggregate_states_to_batch,
    grouped_aggregate_states_to_typed_batch, merge_grouped_aggregate_states,
};
use kaveon_exec::distinct::DistinctOperator;
use kaveon_exec::exchange::{HashPartitionMetrics, HashPartitioner};
use kaveon_exec::filter::FilterOperator;
use kaveon_exec::join::JoinType;
use kaveon_exec::limit::LimitOperator;
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
        let batches = collect(&mut *operator)?;
        let (partitions, hash_partition_metrics) =
            partition_batches(&batches, &schema, &output.partitioning)?;
        let scan_metrics_complete = has_complete_scan_metrics(scan_count, scan_metrics.len());
        return Ok(FragmentExecution {
            result_schema: Arc::clone(&schema),
            result_batches: Vec::new(),
            exchange_outputs: BTreeMap::from([(
                output.exchange_id.clone(),
                ExchangeOutputBatches { schema, partitions },
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
    Ok(FragmentExecution {
        result_schema,
        result_batches: collect(&mut *operator)?,
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
            let group_by = group_by
                .iter()
                .map(|named| expression_column(&named.expression))
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
                        .unwrap_or_else(|| "*".into());
                    let expression = AggExpr::new(function, column).with_alias(&aggregate.output);
                    Ok(if distinct {
                        expression.distinct()
                    } else {
                        expression
                    })
                })
                .collect::<Result<_>>()?;
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
                    let parallelism = kaveon_exec::local_parallel::configured_parallelism()?;
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
                    let output_types = kaveon_exec::aggregate::aggregate_output_types(
                        &aggregates,
                        input.schema(),
                    )?;
                    let group_types = group_by
                        .iter()
                        .map(|name| {
                            input
                                .schema()
                                .field_with_name(name)
                                .map(|field| field.data_type().clone())
                                .map_err(KaveonError::from)
                        })
                        .collect::<Result<Vec<_>>>()?;
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
                    let aggregate = if let Some(memory) = memory {
                        HashAggregate::new_with_memory(
                            input,
                            group_by,
                            aggregates,
                            memory.operator("fragment-partial-hash-aggregate")?,
                        )?
                    } else {
                        HashAggregate::new(input, group_by, aggregates)?
                    };
                    let (states, state_memory) =
                        aggregate.into_grouped_states_with_reservations()?;
                    let encoding_memory = memory
                        .map(|memory| {
                            let bytes = state_memory
                                .iter()
                                .map(|reservation| reservation.bytes())
                                .sum::<u64>()
                                .saturating_mul(4)
                                .saturating_add((states.len() as u64).saturating_mul(4096));
                            memory
                                .operator("fragment-partial-state-encoding")?
                                .reserve(bytes)
                        })
                        .transpose()?;
                    let batch = kaveon_exec::aggregate::grouped_aggregate_states_to_schema_batch(
                        &states,
                        &group_types,
                        &output_types,
                    )?;
                    drop(states);
                    drop(state_memory);
                    drop(encoding_memory);
                    Ok(Box::new(BatchInput::with_memory(
                        batch.schema(),
                        vec![batch],
                        memory,
                    )?))
                }
                AggregateMode::Final => {
                    compile_final_aggregate(input, group_by, aggregates, memory)
                }
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
        FragmentOperator::TopN { keys, limit } => kaveon_exec::partitioned::top_n_operator(
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
        ),
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
            let mut operator = DistinctOperator::new(input);
            if let Some(memory) = memory {
                operator = operator.with_memory(memory.operator("fragment-distinct")?);
            }
            Ok(Box::new(operator))
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
            if join.residual.is_some() {
                return Err(exec_err(
                    "residual fragment join filters are not implemented",
                ));
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

pub(crate) fn compile_final_aggregate(
    input: Box<dyn BatchOperator>,
    group_by: Vec<String>,
    aggregates: Vec<AggExpr>,
    memory: Option<&QueryMemoryPool>,
) -> Result<Box<dyn BatchOperator>> {
    // Hash partitioning cannot divide a global aggregate: every partial has the
    // same empty key and is routed to one partition. Stream those partials into
    // the final merger instead of materializing and spilling an identical spool.
    if should_partition_final_aggregate(&group_by)
        && let Some(memory) = memory
        && let Some((spill, count)) = kaveon_exec::partitioned::spill_from_environment(memory)?
    {
        return partitioned_final_aggregate(input, group_by, aggregates, memory, &spill, count);
    }
    compile_final_aggregate_in_memory(input, group_by, aggregates, memory)
}

fn should_partition_final_aggregate(group_by: &[String]) -> bool {
    !group_by.is_empty()
}

fn partitioned_final_aggregate(
    input: Box<dyn BatchOperator>,
    group_by: Vec<String>,
    aggregates: Vec<AggExpr>,
    memory: &QueryMemoryPool,
    spill: &kaveon_exec::spill::SpillManager,
    count: usize,
) -> Result<Box<dyn BatchOperator>> {
    let group_types = grouped_aggregate_key_types(input.schema())?;
    if group_types.len() != group_by.len() {
        return Err(exec_err("final aggregate group types do not match plan"));
    }
    let output_types = final_output_types(input.schema(), &aggregates)?;
    let schema =
        finalized_aggregate_batch(&group_by, &group_types, &aggregates, &output_types, &[])?
            .schema();
    let keys = if group_by.is_empty() {
        Vec::new()
    } else {
        vec!["group_keys".into()]
    };
    let inputs = kaveon_exec::partitioned::partition_sources(
        input,
        &keys,
        count,
        &memory.operator("fragment-final-partition")?,
        spill,
    )?;
    Ok(Box::new(FinalAggregatePartitions {
        inputs,
        schema,
        group_by,
        aggregates,
        memory: memory.clone(),
    }))
}

struct FinalAggregatePartitions {
    inputs: VecDeque<Box<dyn BatchOperator>>,
    schema: SchemaRef,
    group_by: Vec<String>,
    aggregates: Vec<AggExpr>,
    memory: QueryMemoryPool,
}

impl BatchOperator for FinalAggregatePartitions {
    fn schema(&self) -> &SchemaRef {
        &self.schema
    }
    fn next_batch(&mut self) -> Result<Option<RecordBatch>> {
        let Some(input) = self.inputs.pop_front() else {
            return Ok(None);
        };
        let result = (|| {
            let mut aggregate = compile_final_aggregate_in_memory(
                input,
                self.group_by.clone(),
                self.aggregates.clone(),
                Some(&self.memory),
            )?;
            let batch = aggregate.next_batch()?;
            if let Some(batch) = &batch
                && batch.schema() != self.schema
            {
                return Err(exec_err(
                    "partitioned final aggregate output schema changed",
                ));
            }
            Ok(batch)
        })();
        if result.is_err() {
            self.inputs.clear();
        }
        result
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
        let mut merger =
            kaveon_exec::incremental_aggregate::IncrementalAggregateMerger::new(account.clone());
        while let Some(batch) = input.next_batch()? {
            if grouped_aggregate_key_types(&batch.schema())? != group_types {
                return Err(exec_err("final aggregate input key schema changed"));
            }
            if final_output_types(&batch.schema(), &aggregates)? != output_types {
                return Err(exec_err("final aggregate input output schema changed"));
            }
            merger.push_batch(&batch)?;
        }
        let (mut merged, reservations) = merger.finish()?;
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
        if matches!(output_types[index], DataType::Int32 | DataType::Int64) {
            let values = groups
                .iter()
                .map(|group| match group.values.get(index) {
                    Some(FinalAggregateValue::Integer(value)) => Ok(*value),
                    _ => Err(exec_err("integer final state type mismatch")),
                })
                .collect::<Result<Vec<_>>>()?;
            let column: ArrayRef = if output_types[index] == DataType::Int32 {
                Arc::new(arrow::array::Int32Array::from(
                    values
                        .into_iter()
                        .map(|v| {
                            v.map(|n| {
                                i32::try_from(n).map_err(|_| exec_err("integer aggregate overflow"))
                            })
                            .transpose()
                        })
                        .collect::<Result<Vec<_>>>()?,
                ))
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

fn partition_batches(
    batches: &[RecordBatch],
    schema: &SchemaRef,
    partitioning: &Partitioning,
) -> Result<(Vec<Vec<RecordBatch>>, HashPartitionMetrics)> {
    match partitioning {
        Partitioning::Single | Partitioning::Broadcast => {
            Ok((vec![batches.to_vec()], HashPartitionMetrics::default()))
        }
        Partitioning::RoundRobin { partition_count } => {
            let mut partitions = vec![Vec::new(); *partition_count];
            for (index, batch) in batches.iter().enumerate() {
                partitions[index % partition_count].push(batch.clone());
            }
            Ok((partitions, HashPartitionMetrics::default()))
        }
        Partitioning::Hash {
            columns,
            partition_count,
        } => {
            let partitioner = HashPartitioner::try_new(schema, columns, *partition_count)?;
            let mut partitions = vec![Vec::new(); *partition_count];
            let mut metrics = HashPartitionMetrics::default();
            for batch in batches {
                let (partitioned, batch_metrics) = partitioner.partition_profiled(batch)?;
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
                        partitions[partition].push(batch);
                    }
                }
            }
            Ok((partitions, metrics))
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
    fn distributed_partial_fragment_uses_key_affine_parallelism() {
        const CHILD: &str = "KAVEON_FRAGMENT_AFFINITY_REGRESSION_CHILD";
        if std::env::var_os(CHILD).is_none() {
            let result = std::process::Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "fragment_exec::tests::distributed_partial_fragment_uses_key_affine_parallelism",
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

        let rows = 20_000i64;
        let batch = RecordBatch::try_from_iter(vec![
            (
                "key",
                Arc::new(arrow::array::Int64Array::from_iter_values(0..rows)) as ArrayRef,
            ),
            (
                "value",
                Arc::new(arrow::array::Int64Array::from_iter_values(
                    (0..rows).map(|value| value.saturating_mul(-3)),
                )) as ArrayRef,
            ),
        ])
        .unwrap();
        let fragment = ExecutableFragment {
            version: EXECUTABLE_FRAGMENT_VERSION,
            stage_id: StageId(19),
            root: FragmentNodeId(2),
            nodes: vec![
                node(
                    1,
                    vec![],
                    FragmentOperator::ExchangeInput(ExchangeInput {
                        exchange_id: ExchangeId("raw".into()),
                    }),
                ),
                node(
                    2,
                    vec![1],
                    FragmentOperator::Aggregate {
                        mode: AggregateMode::Partial,
                        group_by: vec![kaveon_core::NamedExpr {
                            name: "key".into(),
                            expression: Expr::Column("key".into()),
                        }],
                        aggregates: vec![
                            AggregateSpec {
                                function: AggregateFunction::Count,
                                argument: None,
                                output: "count".into(),
                            },
                            AggregateSpec {
                                function: AggregateFunction::Sum,
                                argument: Some(Expr::Column("value".into())),
                                output: "sum".into(),
                            },
                        ],
                    },
                ),
            ],
        };
        let pool = QueryMemoryPool::new("fragment-key-affine", 256 * 1024 * 1024).unwrap();
        let execution = execute_fragment_with_memory(
            &fragment,
            &CatalogManager::new("test", "default"),
            &BatchInputs {
                values: HashMap::from([(ExchangeId("raw".into()), vec![batch])]),
            },
            first_partition(),
            Some(&pool),
        )
        .unwrap();
        let partials = grouped_aggregate_states_from_batches(&execution.result_batches).unwrap();
        let merged = merge_grouped_aggregate_states(partials).unwrap();
        assert_eq!(merged.len(), rows as usize);
        assert!(merged.iter().all(|group| {
            let AggregateValue::Int64(key) = group.group_keys[0] else {
                return false;
            };
            group.states
                == vec![
                    AggregateState::Count(1),
                    AggregateState::IntegerSum {
                        sum: key.saturating_mul(-3) as i128,
                        count: 1,
                    },
                ]
        }));
        let metrics = kaveon_exec::aggregate::aggregate_metrics(&pool)
            .unwrap()
            .snapshot();
        assert_eq!(metrics.local_affinity_dispatches, 1);
        assert_eq!(metrics.local_round_robin_dispatches, 0);
        assert_eq!(metrics.local_affinity_routed_rows, rows as u64);
        assert!(metrics.local_affinity_routed_bytes > 0);
        assert_eq!(pool.snapshot().current_bytes, 0);
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
    fn partitioned_final_aggregate_keeps_typed_null_keys_and_bounds_merge_memory() {
        let mut batches = Vec::new();
        for _ in 0..2 {
            for start in (0..100).step_by(5) {
                let groups = (start..start + 5)
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
        let pool = QueryMemoryPool::new("final-spill", 4 * 1024 * 1024).unwrap();
        let expressions = vec![AggExpr::new(AggFunc::Count, "*").with_alias("count")];
        drop(
            compile_final_aggregate_in_memory(
                Box::new(BatchInput::new(schema.clone(), batches.clone())),
                vec!["key".into()],
                expressions.clone(),
                Some(&pool),
            )
            .unwrap(),
        );
        assert_eq!(pool.snapshot().current_bytes, 0);
        let spill = kaveon_exec::spill::SpillManager::new(
            std::env::temp_dir().join("kaveon-final-spill-tests"),
            64 * 1024 * 1024,
        )
        .unwrap();
        let mut operator = partitioned_final_aggregate(
            Box::new(BatchInput::new(schema, batches)),
            vec!["key".into()],
            expressions,
            &pool,
            &spill,
            16,
        )
        .unwrap();
        let mut rows = 0;
        let mut nulls = 0;
        while let Some(batch) = operator.next_batch().unwrap() {
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
        assert_eq!((rows, nulls), (100, 1));
        assert_eq!(pool.snapshot().current_bytes, 0);
        assert!(pool.snapshot().peak_bytes <= 4 * 1024 * 1024);
        assert_eq!(spill.snapshot().current_bytes, 0);
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
        let mut operator = partitioned_final_aggregate(
            Box::new(BatchInput::new(batch.schema(), vec![batch; 100])),
            vec!["key".into()],
            vec![AggExpr::new(AggFunc::Count, "*")],
            &pool,
            &spill,
            16,
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
        assert!(!should_partition_final_aggregate(&[]));
        assert!(should_partition_final_aggregate(&["key".into()]));

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
        let _ = JoinSpec {
            left_qualifier: None,
            right_qualifier: None,
            join_type: kaveon_core::JoinType::Inner,
            left_keys: vec![Expr::Column("key".into())],
            right_keys: vec![Expr::Column("key".into())],
            residual: None,
            broadcast: false,
        };
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
