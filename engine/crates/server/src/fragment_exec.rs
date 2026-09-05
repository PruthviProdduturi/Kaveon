use std::collections::{BTreeMap, HashMap, VecDeque};
use std::path::PathBuf;
use std::sync::Arc;

use arrow::array::{
    ArrayRef, BooleanArray, Float64Array, Int32Array, Int64Array, StringArray, UInt64Array,
};
use arrow::datatypes::{DataType, Field, Schema, SchemaRef};
use arrow::record_batch::RecordBatch;
use kaveon_core::{
    AggregateFunction, AggregateMode, BatchOperator, CatalogManager, DataFormat, ExchangeId,
    ExecutableFragment, Expr, FragmentNode, FragmentNodeId, FragmentOperator, KaveonError,
    Partitioning, QueryMemoryPool, Result,
};
use kaveon_exec::aggregate::{
    AggExpr, AggFunc, AggregateState, AggregateValue, FinalAggregateValue, GroupedAggregateState,
    HashAggregate, finalize_grouped_aggregate_states, grouped_aggregate_states_from_batches,
    grouped_aggregate_states_to_batch, merge_grouped_aggregate_states,
};
use kaveon_exec::distinct::DistinctOperator;
use kaveon_exec::exchange::HashPartitioner;
use kaveon_exec::filter::FilterOperator;
use kaveon_exec::join::{HashJoin, JoinType};
use kaveon_exec::limit::LimitOperator;
use kaveon_exec::offset::OffsetOperator;
use kaveon_exec::project::ProjectOperator;
use kaveon_exec::scan::ScanOperator;
use kaveon_exec::setop::{SetOpMode, SetOpOperator};
use kaveon_exec::sort::{SortExpr, SortOperator};
use kaveon_exec::topn::TopNOperator;
use kaveon_exec::union::UnionOperator;
use kaveon_exec::window::WindowOperator;
use kaveon_storage::{AdlsParquetReader, DeltaTableReader, ParquetReader, ScanPartition};

pub struct ExchangeBatches {
    pub schema: SchemaRef,
    pub batches: Vec<RecordBatch>,
}

pub trait ExchangeInputProvider {
    fn read(&self, exchange_id: &ExchangeId) -> Result<ExchangeBatches>;
}

pub struct FragmentExecution {
    pub result_schema: SchemaRef,
    pub result_batches: Vec<RecordBatch>,
    pub exchange_outputs: BTreeMap<ExchangeId, ExchangeOutputBatches>,
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
    if let FragmentOperator::ExchangeOutput(output) = &root.operator {
        let mut operator = compile_node(
            root.inputs[0],
            &nodes,
            catalog,
            exchanges,
            scan_partition,
            memory,
        )?;
        let schema = Arc::clone(operator.schema());
        let batches = collect(&mut *operator)?;
        let partitions = partition_batches(&batches, &schema, &output.partitioning)?;
        return Ok(FragmentExecution {
            result_schema: Arc::clone(&schema),
            result_batches: Vec::new(),
            exchange_outputs: BTreeMap::from([(
                output.exchange_id.clone(),
                ExchangeOutputBatches { schema, partitions },
            )]),
        });
    }
    let mut operator = compile_node(
        fragment.root,
        &nodes,
        catalog,
        exchanges,
        scan_partition,
        memory,
    )?;
    let result_schema = Arc::clone(operator.schema());
    Ok(FragmentExecution {
        result_schema,
        result_batches: collect(&mut *operator)?,
        exchange_outputs: BTreeMap::new(),
    })
}

fn compile_node(
    id: FragmentNodeId,
    nodes: &HashMap<FragmentNodeId, &FragmentNode>,
    catalog: &CatalogManager,
    exchanges: &dyn ExchangeInputProvider,
    scan_partition: ScanPartition,
    memory: Option<&QueryMemoryPool>,
) -> Result<Box<dyn BatchOperator>> {
    let node = nodes[&id];
    match &node.operator {
        FragmentOperator::Scan(scan) => {
            let source: Box<dyn kaveon_core::BatchSource> = match scan.format {
                DataFormat::Parquet => {
                    if scan.source_uri.starts_with("abfss://") {
                        let mut reader = AdlsParquetReader::from_abfss_uri(&scan.source_uri)?
                            .with_partition(scan_partition);
                        if !scan.projection.is_empty() {
                            reader = reader.with_columns(scan.projection.clone());
                        }
                        if let Some(predicate) = &scan.predicate {
                            reader = reader.with_predicate(predicate.clone());
                        }
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
                    Box::new(reader.read()?)
                }
                DataFormat::Delta => {
                    if scan.source_uri.starts_with("abfss://") {
                        return Err(exec_err(
                            "fragment Delta snapshots over ADLS Gen2 are not implemented",
                        ));
                    }
                    let path = local_path(&scan.source_uri)?;
                    let mut reader = DeltaTableReader::new(path).with_partition(scan_partition);
                    if !scan.projection.is_empty() {
                        reader = reader.with_columns(scan.projection.clone());
                    }
                    Box::new(reader.read()?)
                }
                DataFormat::Iceberg => {
                    return Err(exec_err("fragment Iceberg scans are not implemented"));
                }
            };
            Ok(Box::new(ScanOperator::new(source, None)?))
        }
        FragmentOperator::ExchangeInput(input) => {
            let input = exchanges.read(&input.exchange_id)?;
            Ok(Box::new(BatchInput::new(input.schema, input.batches)))
        }
        FragmentOperator::Filter { predicate } => Ok(Box::new(FilterOperator::new(
            compile_input(node, 0, nodes, catalog, exchanges, scan_partition, memory)?,
            predicate.clone(),
        ))),
        FragmentOperator::Project { expressions } => Ok(Box::new(ProjectOperator::new(
            compile_input(node, 0, nodes, catalog, exchanges, scan_partition, memory)?,
            expressions
                .iter()
                .map(|named| Expr::Alias {
                    expr: Box::new(named.expression.clone()),
                    name: named.name.clone(),
                })
                .collect(),
        )?)),
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
            let input = compile_input(node, 0, nodes, catalog, exchanges, scan_partition, memory)?;
            match mode {
                AggregateMode::Single => Ok(Box::new(if let Some(memory) = memory {
                    HashAggregate::new_with_memory(
                        input,
                        group_by,
                        aggregates,
                        memory.operator("fragment-hash-aggregate")?,
                    )?
                } else {
                    HashAggregate::new(input, group_by, aggregates)?
                })),
                AggregateMode::Partial => {
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
                    let states = aggregate.into_grouped_states()?;
                    let batch = grouped_aggregate_states_to_batch(&states)?;
                    Ok(Box::new(BatchInput::new(batch.schema(), vec![batch])))
                }
                AggregateMode::Final => compile_final_aggregate(input, group_by, aggregates),
            }
        }
        FragmentOperator::Sort { keys } => Ok(Box::new(SortOperator::new(
            compile_input(node, 0, nodes, catalog, exchanges, scan_partition, memory)?,
            sort_expressions(keys),
        )?)),
        FragmentOperator::TopN { keys, limit } => Ok(Box::new(TopNOperator::new(
            compile_input(node, 0, nodes, catalog, exchanges, scan_partition, memory)?,
            sort_expressions(keys),
            *limit,
        )?)),
        FragmentOperator::Limit { limit } => Ok(Box::new(LimitOperator::new(
            compile_input(node, 0, nodes, catalog, exchanges, scan_partition, memory)?,
            *limit,
        ))),
        FragmentOperator::Offset { offset } => Ok(Box::new(OffsetOperator::new(
            compile_input(node, 0, nodes, catalog, exchanges, scan_partition, memory)?,
            *offset,
        ))),
        FragmentOperator::Distinct => Ok(Box::new(DistinctOperator::new(compile_input(
            node,
            0,
            nodes,
            catalog,
            exchanges,
            scan_partition,
            memory,
        )?))),
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
            let left = compile_input(node, 0, nodes, catalog, exchanges, scan_partition, memory)?;
            let right = compile_input(node, 1, nodes, catalog, exchanges, scan_partition, memory)?;
            let keys = left_keys.into_iter().zip(right_keys).collect();
            Ok(Box::new(if let Some(memory) = memory {
                HashJoin::try_new_qualified_with_memory(
                    left,
                    right,
                    join_type(join.join_type)?,
                    keys,
                    None,
                    None,
                    memory.operator("fragment-hash-join")?,
                )?
            } else {
                HashJoin::try_new(left, right, join_type(join.join_type)?, keys)?
            }))
        }
        FragmentOperator::Window => {
            let input = compile_input(node, 0, nodes, catalog, exchanges, scan_partition, memory)?;
            Ok(Box::new(WindowOperator::new(input, vec![])))
        }
        FragmentOperator::Intersect => {
            let left = compile_input(node, 0, nodes, catalog, exchanges, scan_partition, memory)?;
            let right = compile_input(node, 1, nodes, catalog, exchanges, scan_partition, memory)?;
            Ok(Box::new(SetOpOperator::new(
                left,
                right,
                SetOpMode::Intersect,
            )))
        }
        FragmentOperator::Except => {
            let left = compile_input(node, 0, nodes, catalog, exchanges, scan_partition, memory)?;
            let right = compile_input(node, 1, nodes, catalog, exchanges, scan_partition, memory)?;
            Ok(Box::new(SetOpOperator::new(left, right, SetOpMode::Except)))
        }
        FragmentOperator::ExchangeOutput(_) => Err(exec_err(
            "ExchangeOutput is supported only as the fragment root",
        )),
    }
}

fn compile_final_aggregate(
    mut input: Box<dyn BatchOperator>,
    group_by: Vec<String>,
    aggregates: Vec<AggExpr>,
) -> Result<Box<dyn BatchOperator>> {
    let batches = collect(&mut *input)?;
    let mut states = grouped_aggregate_states_from_batches(&batches)?;
    if states.is_empty() && group_by.is_empty() {
        states.push(GroupedAggregateState {
            group_keys: Vec::new(),
            states: aggregates.iter().map(AggregateState::new).collect(),
        });
    }
    let merged = merge_grouped_aggregate_states(states)?;
    let finalized = finalize_grouped_aggregate_states(&merged)?;
    let batch = finalized_aggregate_batch(&group_by, &aggregates, &finalized)?;
    Ok(Box::new(BatchInput::new(batch.schema(), vec![batch])))
}

fn finalized_aggregate_batch(
    group_by: &[String],
    aggregates: &[AggExpr],
    groups: &[kaveon_exec::aggregate::FinalizedAggregateGroup],
) -> Result<RecordBatch> {
    let mut fields = Vec::with_capacity(group_by.len() + aggregates.len());
    let mut columns = Vec::with_capacity(group_by.len() + aggregates.len());
    for (index, name) in group_by.iter().enumerate() {
        let data_type = infer_group_type(groups, index);
        fields.push(Field::new(name, data_type.clone(), true));
        columns.push(group_column(groups, index, &data_type));
    }
    for (index, aggregate) in aggregates.iter().enumerate() {
        if matches!(aggregate.func, AggFunc::Count) {
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
    aggregate
        .alias
        .clone()
        .unwrap_or_else(|| "aggregate".into())
}

fn infer_group_type(
    groups: &[kaveon_exec::aggregate::FinalizedAggregateGroup],
    index: usize,
) -> DataType {
    groups
        .iter()
        .filter_map(|group| group.group_keys.get(index))
        .find_map(|value| match value {
            AggregateValue::Null => None,
            AggregateValue::Bool(_) => Some(DataType::Boolean),
            AggregateValue::Int32(_) => Some(DataType::Int32),
            AggregateValue::Int64(_) => Some(DataType::Int64),
            AggregateValue::Utf8(_) => Some(DataType::Utf8),
            AggregateValue::Float64Bits(_) => Some(DataType::Float64),
        })
        .unwrap_or(DataType::Utf8)
}

fn group_column(
    groups: &[kaveon_exec::aggregate::FinalizedAggregateGroup],
    index: usize,
    data_type: &DataType,
) -> ArrayRef {
    match data_type {
        DataType::Boolean => Arc::new(BooleanArray::from(
            groups
                .iter()
                .map(|group| match group.group_keys.get(index) {
                    Some(AggregateValue::Bool(value)) => Some(*value),
                    _ => None,
                })
                .collect::<Vec<_>>(),
        )),
        DataType::Int32 => Arc::new(Int32Array::from(
            groups
                .iter()
                .map(|group| match group.group_keys.get(index) {
                    Some(AggregateValue::Int32(value)) => Some(*value),
                    _ => None,
                })
                .collect::<Vec<_>>(),
        )),
        DataType::Int64 => Arc::new(Int64Array::from(
            groups
                .iter()
                .map(|group| match group.group_keys.get(index) {
                    Some(AggregateValue::Int64(value)) => Some(*value),
                    _ => None,
                })
                .collect::<Vec<_>>(),
        )),
        DataType::Float64 => Arc::new(Float64Array::from(
            groups
                .iter()
                .map(|group| match group.group_keys.get(index) {
                    Some(AggregateValue::Float64Bits(value)) => Some(f64::from_bits(*value)),
                    _ => None,
                })
                .collect::<Vec<_>>(),
        )),
        _ => Arc::new(StringArray::from(
            groups
                .iter()
                .map(|group| match group.group_keys.get(index) {
                    Some(AggregateValue::Utf8(value)) => Some(value.as_str()),
                    _ => None,
                })
                .collect::<Vec<_>>(),
        )),
    }
}

fn compile_input(
    node: &FragmentNode,
    index: usize,
    nodes: &HashMap<FragmentNodeId, &FragmentNode>,
    catalog: &CatalogManager,
    exchanges: &dyn ExchangeInputProvider,
    scan_partition: ScanPartition,
    memory: Option<&QueryMemoryPool>,
) -> Result<Box<dyn BatchOperator>> {
    compile_node(
        node.inputs[index],
        nodes,
        catalog,
        exchanges,
        scan_partition,
        memory,
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
) -> Result<Vec<Vec<RecordBatch>>> {
    match partitioning {
        Partitioning::Single | Partitioning::Broadcast => Ok(vec![batches.to_vec()]),
        Partitioning::RoundRobin { partition_count } => {
            let mut partitions = vec![Vec::new(); *partition_count];
            for (index, batch) in batches.iter().enumerate() {
                partitions[index % partition_count].push(batch.clone());
            }
            Ok(partitions)
        }
        Partitioning::Hash {
            columns,
            partition_count,
        } => {
            let partitioner = HashPartitioner::try_new(schema, columns, *partition_count)?;
            let mut partitions = vec![Vec::new(); *partition_count];
            for batch in batches {
                for (partition, batch) in partitioner.partition(batch)?.into_iter().enumerate() {
                    if batch.num_rows() > 0 {
                        partitions[partition].push(batch);
                    }
                }
            }
            Ok(partitions)
        }
    }
}

struct BatchInput {
    schema: SchemaRef,
    batches: VecDeque<RecordBatch>,
}

impl BatchInput {
    fn new(schema: SchemaRef, batches: Vec<RecordBatch>) -> Self {
        Self {
            schema,
            batches: batches.into(),
        }
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
                .downcast_ref::<Float64Array>()
                .unwrap()
                .value(west),
            150.0
        );
        assert_eq!(
            result
                .column(3)
                .as_any()
                .downcast_ref::<Float64Array>()
                .unwrap()
                .value(west),
            10.0
        );
        assert_eq!(
            result
                .column(4)
                .as_any()
                .downcast_ref::<Float64Array>()
                .unwrap()
                .value(west),
            100.0
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
    fn rejects_residual_and_unsupported_join_modes_explicitly() {
        let result = join_type(kaveon_core::JoinType::Semi);
        assert!(result.is_err());
        let _ = JoinSpec {
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

        fs::remove_dir_all(directory).unwrap();
    }
}
