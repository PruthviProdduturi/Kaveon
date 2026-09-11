use ahash::AHashMap;
use arrow::array::{
    Array, ArrayRef, AsArray, BinaryArray, BooleanArray, Float64Array, Int32Array, Int64Array,
    StringArray, UInt8Array, UInt64Array,
};
use arrow::datatypes::{DataType, Field, Float64Type, Int32Type, Int64Type, Schema, SchemaRef};
use arrow::ipc::reader::StreamReader;
use arrow::ipc::writer::StreamWriter;
use arrow::record_batch::RecordBatch;
use kaveon_core::{BatchOperator, KaveonError, MemoryReservation, OperatorMemoryAccount, Result};
use std::collections::{HashMap, HashSet, hash_map::Entry};
use std::io::Cursor;
use std::sync::Arc;

const AGGREGATE_STATE_VERSION_KEY: &str = "kaveon.aggregate_state.version";
const AGGREGATE_STATE_VERSION: &str = "2";
const GROUPED_STATE_VERSION_KEY: &str = "kaveon.grouped_aggregate_state.version";
const GROUPED_STATE_VERSION: &str = "3";
#[path = "compact_state.rs"]
mod compact_state;
const GROUPED_KEY_TYPES: &str = "kaveon.grouped_aggregate_state.key_types";
const GROUPED_OUTPUT_TYPES: &str = "kaveon.grouped_aggregate_state.output_types";
const STATE_SUM: u8 = 1;
const STATE_COUNT: u8 = 2;
const STATE_MIN: u8 = 3;
const STATE_MAX: u8 = 4;
const STATE_AVG: u8 = 5;
const STATE_COUNT_DISTINCT: u8 = 6;
const STATE_DECIMAL_SUM: u8 = 9;
const STATE_INTEGER_SUM: u8 = 10;
const STATE_INTEGER_MIN: u8 = 11;
const STATE_INTEGER_MAX: u8 = 12;
const STATE_INTEGER_SUM_DISTINCT: u8 = 13;
const VALUE_BOOL: u8 = 1;
const VALUE_INT32: u8 = 2;
const VALUE_INT64: u8 = 3;
const VALUE_UTF8: u8 = 4;
const VALUE_FLOAT64_BITS: u8 = 5;
const VALUE_UINT64: u8 = 6;
const VALUE_DECIMAL128: u8 = 7;
const VALUE_NULL: u8 = 0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AggFunc {
    Sum,
    Count,
    Min,
    Max,
    Avg,
}

#[derive(Debug, Clone)]
pub struct AggExpr {
    pub func: AggFunc,
    pub column: String,
    pub alias: Option<String>,
    pub distinct: bool,
}

impl AggExpr {
    pub fn new(func: AggFunc, column: impl Into<String>) -> Self {
        Self {
            func,
            column: column.into(),
            alias: None,
            distinct: false,
        }
    }

    pub fn distinct(mut self) -> Self {
        self.distinct = true;
        self
    }

    pub fn with_alias(mut self, alias: impl Into<String>) -> Self {
        self.alias = Some(alias.into());
        self
    }

    pub fn output_name(&self) -> String {
        if let Some(ref alias) = self.alias {
            return alias.clone();
        }
        let func_name = match self.func {
            AggFunc::Sum => "sum",
            AggFunc::Count => "count",
            AggFunc::Min => "min",
            AggFunc::Max => "max",
            AggFunc::Avg => "avg",
        };
        format!("{func_name}_{}", self.column)
    }

    fn output_type(&self) -> DataType {
        match self.func {
            AggFunc::Count => DataType::UInt64,
            _ => DataType::Float64,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum AggregateValue {
    Null,
    Bool(bool),
    Int32(i32),
    Int64(i64),
    UInt64(u64),
    Decimal128(i128, i8),
    Utf8(String),
    Float64Bits(u64),
}

#[derive(Debug, Clone, PartialEq)]
pub enum AggregateState {
    Sum {
        sum: f64,
        count: u64,
    },
    DecimalSum {
        sum: i128,
        count: u64,
        scale: i8,
    },
    Exact {
        function: AggFunc,
        scale: Option<i8>,
        value: Option<i128>,
        distinct: Option<HashSet<AggregateValue>>,
    },
    IntegerSum {
        sum: i128,
        count: u64,
    },
    IntegerMin(Option<i64>),
    IntegerMax(Option<i64>),
    IntegerSumDistinct(HashSet<AggregateValue>),
    Count(u64),
    Min(Option<f64>),
    Max(Option<f64>),
    Avg {
        sum: f64,
        count: u64,
    },
    CountDistinct(HashSet<AggregateValue>),
    SumDistinct(HashSet<AggregateValue>),
    AvgDistinct(HashSet<AggregateValue>),
}

#[derive(Debug, Clone, PartialEq)]
pub struct GroupedAggregateState {
    pub group_keys: Vec<AggregateValue>,
    pub states: Vec<AggregateState>,
}

pub fn aggregate_output_types(aggregates: &[AggExpr], input: &SchemaRef) -> Result<Vec<DataType>> {
    aggregates
        .iter()
        .map(|agg| {
            if let Ok(field) = input.field_with_name(&agg.column)
                && matches!(field.data_type(), DataType::Int32 | DataType::Int64)
            {
                match agg.func {
                    AggFunc::Sum => return Ok(DataType::Int64),
                    AggFunc::Min | AggFunc::Max => return Ok(field.data_type().clone()),
                    _ => {}
                }
            }
            if let Ok(field) = input.field_with_name(&agg.column) {
                if matches!(agg.func, AggFunc::Sum | AggFunc::Min | AggFunc::Max)
                    && field.data_type() == &DataType::UInt64
                {
                    return Ok(DataType::UInt64);
                }
                if matches!(agg.func, AggFunc::Min | AggFunc::Max)
                    && matches!(field.data_type(), DataType::Decimal128(_, _))
                {
                    return Ok(field.data_type().clone());
                }
            }
            if matches!(agg.func, AggFunc::Sum)
                && let Ok(field) = input.field_with_name(&agg.column)
                && let DataType::Decimal128(_, scale) = field.data_type()
            {
                return Ok(DataType::Decimal128(38, *scale));
            }
            Ok(agg.output_type())
        })
        .collect()
}

#[derive(Debug, Clone, PartialEq)]
pub enum FinalAggregateValue {
    Count(u64),
    Numeric(Option<f64>),
    Decimal(Option<i128>, i8),
    Integer(Option<i128>),
}

#[derive(Debug, Clone, PartialEq)]
pub struct FinalizedAggregateGroup {
    pub group_keys: Vec<AggregateValue>,
    pub values: Vec<FinalAggregateValue>,
}

impl AggregateState {
    pub fn new_typed(expression: &AggExpr, output_type: &DataType) -> Self {
        if output_type == &DataType::UInt64 && !matches!(expression.func, AggFunc::Count)
            || matches!(output_type, DataType::Decimal128(_, _))
                && (expression.distinct || matches!(expression.func, AggFunc::Min | AggFunc::Max))
        {
            return Self::Exact {
                function: expression.func,
                scale: match output_type {
                    DataType::Decimal128(_, scale) => Some(*scale),
                    _ => None,
                },
                value: None,
                distinct: (expression.distinct && matches!(expression.func, AggFunc::Sum))
                    .then(HashSet::new),
            };
        }
        if matches!(output_type, DataType::Int32 | DataType::Int64) {
            return match (expression.func, expression.distinct) {
                (AggFunc::Sum, false) => Self::IntegerSum { sum: 0, count: 0 },
                (AggFunc::Sum, true) => Self::IntegerSumDistinct(HashSet::new()),
                (AggFunc::Min, _) => Self::IntegerMin(None),
                (AggFunc::Max, _) => Self::IntegerMax(None),
                _ => Self::new(expression),
            };
        }
        if let DataType::Decimal128(_, scale) = output_type {
            return Self::DecimalSum {
                sum: 0,
                count: 0,
                scale: *scale,
            };
        }
        Self::new(expression)
    }
    pub fn update_exact(&mut self, input: AggregateValue) -> Result<()> {
        let Self::Exact {
            function,
            scale,
            value,
            distinct,
        } = self
        else {
            return Err(exec_err("expected exact numeric state"));
        };
        let number = match (&input, *scale) {
            (AggregateValue::UInt64(n), None) => *n as i128,
            (AggregateValue::Decimal128(n, actual), Some(expected)) if *actual == expected => *n,
            _ => return Err(exec_err("exact numeric input type mismatch")),
        };
        if let Some(values) = distinct {
            values.insert(input);
            return Ok(());
        }
        *value = Some(match (*function, *value) {
            (AggFunc::Sum, Some(old)) => old
                .checked_add(number)
                .ok_or_else(|| exec_err("exact SUM overflow"))?,
            (AggFunc::Min, Some(old)) => old.min(number),
            (AggFunc::Max, Some(old)) => old.max(number),
            (_, None) => number,
            _ => return Err(exec_err("invalid exact aggregate function")),
        });
        Ok(())
    }
    pub fn exact_result(&self) -> Result<Option<i128>> {
        let Self::Exact {
            value, distinct, ..
        } = self
        else {
            return Err(exec_err("expected exact numeric state"));
        };
        if let Some(values) = distinct {
            let mut sum = 0i128;
            for (index, value) in values.iter().enumerate() {
                if index % 1024 == 0 {
                    crate::expr_eval::check_expression_cancelled()?;
                }
                let number = match value {
                    AggregateValue::UInt64(n) => *n as i128,
                    AggregateValue::Decimal128(n, _) => *n,
                    _ => return Err(exec_err("invalid exact distinct value")),
                };
                sum = sum
                    .checked_add(number)
                    .ok_or_else(|| exec_err("exact SUM DISTINCT overflow"))?;
            }
            Ok((!values.is_empty()).then_some(sum))
        } else {
            Ok(*value)
        }
    }
    pub fn update_decimal(&mut self, value: i128) -> Result<()> {
        let Self::DecimalSum { sum, count, .. } = self else {
            return Err(exec_err("decimal update requires decimal SUM state"));
        };
        *sum = sum
            .checked_add(value)
            .ok_or_else(|| exec_err("decimal SUM overflow"))?;
        *count = count
            .checked_add(1)
            .ok_or_else(|| exec_err("decimal SUM count overflow"))?;
        Ok(())
    }
    pub fn update_integer(&mut self, value: i64) -> Result<()> {
        match self {
            Self::IntegerSum { sum, count } => {
                *sum = sum
                    .checked_add(value as i128)
                    .ok_or_else(|| exec_err("integer SUM overflow"))?;
                *count = count
                    .checked_add(1)
                    .ok_or_else(|| exec_err("integer SUM count overflow"))?;
            }
            Self::IntegerMin(current) => {
                *current = Some(current.map_or(value, |old| old.min(value)))
            }
            Self::IntegerMax(current) => {
                *current = Some(current.map_or(value, |old| old.max(value)))
            }
            _ => return Err(exec_err("integer update requires integer aggregate")),
        }
        Ok(())
    }
    pub fn integer_result(&self) -> Result<Option<i128>> {
        match self {
            Self::IntegerSum { sum, count } => Ok((*count > 0).then_some(*sum)),
            Self::IntegerMin(value) | Self::IntegerMax(value) => Ok(value.map(i128::from)),
            Self::IntegerSumDistinct(values) => {
                let mut sum = 0i128;
                for value in values {
                    let n = match value {
                        AggregateValue::Int32(n) => *n as i128,
                        AggregateValue::Int64(n) => *n as i128,
                        _ => return Err(exec_err("invalid integer SUM DISTINCT value")),
                    };
                    sum = sum
                        .checked_add(n)
                        .ok_or_else(|| exec_err("integer SUM DISTINCT overflow"))?;
                }
                Ok((!values.is_empty()).then_some(sum))
            }
            _ => Err(exec_err("integer result requires integer aggregate")),
        }
    }
    pub fn new(expression: &AggExpr) -> Self {
        match (expression.func, expression.distinct) {
            (AggFunc::Sum, false) => Self::Sum { sum: 0.0, count: 0 },
            (AggFunc::Count, false) => Self::Count(0),
            (AggFunc::Min, false) => Self::Min(None),
            (AggFunc::Max, false) => Self::Max(None),
            (AggFunc::Avg, false) => Self::Avg { sum: 0.0, count: 0 },
            (AggFunc::Count, true) => Self::CountDistinct(HashSet::new()),
            (AggFunc::Sum, true) => Self::SumDistinct(HashSet::new()),
            (AggFunc::Avg, true) => Self::AvgDistinct(HashSet::new()),
            (_, true) => unreachable!("aggregate validation rejects unsupported DISTINCT"),
        }
    }

    pub fn update_count(&mut self) -> Result<()> {
        match self {
            Self::Count(count) => {
                *count += 1;
                Ok(())
            }
            _ => Err(exec_err(
                "count update applied to a non-count aggregate state",
            )),
        }
    }

    pub fn update_numeric(&mut self, value: f64) -> Result<()> {
        match self {
            Self::Sum { sum, count } | Self::Avg { sum, count } => {
                *sum += value;
                *count += 1;
            }
            Self::Min(current) => {
                *current = Some(current.map_or(value, |existing| existing.min(value)));
            }
            Self::Max(current) => {
                *current = Some(current.map_or(value, |existing| existing.max(value)));
            }
            _ => {
                return Err(exec_err(
                    "numeric update applied to an incompatible aggregate state",
                ));
            }
        }
        Ok(())
    }

    pub fn update_distinct(&mut self, value: AggregateValue) -> Result<()> {
        if matches!(self, Self::Exact { .. }) {
            return self.update_exact(value);
        }
        match self {
            Self::CountDistinct(values)
            | Self::SumDistinct(values)
            | Self::AvgDistinct(values)
            | Self::IntegerSumDistinct(values) => {
                if !matches!(value, AggregateValue::Null) {
                    values.insert(value);
                }
                Ok(())
            }
            _ => Err(exec_err(
                "distinct update applied to a non-distinct aggregate state",
            )),
        }
    }

    pub fn merge(&mut self, partial: &Self) -> Result<()> {
        if let (
            Self::Exact {
                function,
                scale,
                value,
                distinct,
            },
            Self::Exact {
                function: other,
                scale: other_scale,
                value: other_value,
                distinct: other_distinct,
            },
        ) = (&mut *self, partial)
        {
            if function != other
                || scale != other_scale
                || distinct.is_some() != other_distinct.is_some()
            {
                return Err(exec_err("incompatible exact numeric partials"));
            }
            if let (Some(values), Some(other_values)) = (distinct, other_distinct) {
                values.extend(other_values.iter().cloned());
            } else if let Some(number) = other_value {
                *value = Some(match (*function, *value) {
                    (AggFunc::Sum, Some(old)) => old
                        .checked_add(*number)
                        .ok_or_else(|| exec_err("exact SUM overflow"))?,
                    (AggFunc::Min, Some(old)) => old.min(*number),
                    (AggFunc::Max, Some(old)) => old.max(*number),
                    (_, None) => *number,
                    _ => return Err(exec_err("invalid exact aggregate function")),
                });
            }
            return Ok(());
        }
        match (self, partial) {
            (
                Self::IntegerSum { sum, count },
                Self::IntegerSum {
                    sum: other,
                    count: other_count,
                },
            ) => {
                *sum = sum
                    .checked_add(*other)
                    .ok_or_else(|| exec_err("integer SUM merge overflow"))?;
                *count = count
                    .checked_add(*other_count)
                    .ok_or_else(|| exec_err("integer SUM count overflow"))?;
            }
            (Self::IntegerMin(value), Self::IntegerMin(other)) => {
                if let Some(n) = other {
                    *value = Some(value.map_or(*n, |old| old.min(*n)));
                }
            }
            (Self::IntegerMax(value), Self::IntegerMax(other)) => {
                if let Some(n) = other {
                    *value = Some(value.map_or(*n, |old| old.max(*n)));
                }
            }
            (
                Self::DecimalSum { sum, count, scale },
                Self::DecimalSum {
                    sum: other,
                    count: other_count,
                    scale: other_scale,
                },
            ) if scale == other_scale => {
                *sum = sum
                    .checked_add(*other)
                    .ok_or_else(|| exec_err("decimal SUM merge overflow"))?;
                *count = count
                    .checked_add(*other_count)
                    .ok_or_else(|| exec_err("decimal SUM count overflow"))?;
            }
            (
                Self::Sum { sum, count },
                Self::Sum {
                    sum: other_sum,
                    count: other_count,
                },
            )
            | (
                Self::Avg { sum, count },
                Self::Avg {
                    sum: other_sum,
                    count: other_count,
                },
            ) => {
                *sum += other_sum;
                *count += other_count;
            }
            (Self::Count(count), Self::Count(other)) => *count += other,
            (Self::Min(value), Self::Min(other)) => {
                if let Some(other) = other {
                    *value = Some(value.map_or(*other, |current| current.min(*other)));
                }
            }
            (Self::Max(value), Self::Max(other)) => {
                if let Some(other) = other {
                    *value = Some(value.map_or(*other, |current| current.max(*other)));
                }
            }
            (Self::CountDistinct(values), Self::CountDistinct(other))
            | (Self::IntegerSumDistinct(values), Self::IntegerSumDistinct(other))
            | (Self::SumDistinct(values), Self::SumDistinct(other))
            | (Self::AvgDistinct(values), Self::AvgDistinct(other)) => {
                values.extend(other.iter().cloned());
            }
            _ => return Err(exec_err("cannot merge incompatible aggregate states")),
        }
        Ok(())
    }

    pub fn count_result(&self) -> Result<u64> {
        match self {
            Self::Count(count) => Ok(*count),
            Self::CountDistinct(values) => Ok(values.len() as u64),
            _ => Err(exec_err(
                "count result requested from a non-count aggregate state",
            )),
        }
    }

    pub fn numeric_result(&self) -> Result<Option<f64>> {
        match self {
            Self::Sum { sum, count } => Ok((*count > 0).then_some(*sum)),
            Self::Min(value) | Self::Max(value) => Ok(*value),
            Self::Avg { sum, count } => Ok((*count > 0).then(|| *sum / *count as f64)),
            Self::SumDistinct(values) => {
                if values.is_empty() {
                    Ok(None)
                } else {
                    Ok(Some(values.iter().map(aggregate_value_to_f64).sum::<f64>()))
                }
            }
            Self::AvgDistinct(values) => {
                if values.is_empty() {
                    Ok(None)
                } else {
                    let sum: f64 = values.iter().map(aggregate_value_to_f64).sum();
                    Ok(Some(sum / values.len() as f64))
                }
            }
            _ => Err(exec_err(
                "numeric result requested from a count aggregate state",
            )),
        }
    }
}

pub fn encode_grouped_aggregate_states(groups: &[GroupedAggregateState]) -> Result<Vec<u8>> {
    let batch = grouped_aggregate_states_to_batch(groups)?;
    let schema = batch.schema();
    let mut bytes = Vec::new();
    {
        let mut writer = StreamWriter::try_new(&mut bytes, &schema)?;
        writer.write(&batch)?;
        writer.finish()?;
    }
    Ok(bytes)
}

/// Builds the canonical Arrow batch exchanged between partial and final aggregates.
pub fn grouped_aggregate_states_to_batch(groups: &[GroupedAggregateState]) -> Result<RecordBatch> {
    let count = groups.first().map_or(0, |g| g.group_keys.len());
    let types = (0..count)
        .map(|i| {
            groups
                .iter()
                .filter_map(|g| g.group_keys.get(i))
                .find_map(|value| match value {
                    AggregateValue::Null => None,
                    AggregateValue::Bool(_) => Some(DataType::Boolean),
                    AggregateValue::Int32(_) => Some(DataType::Int32),
                    AggregateValue::Int64(_) => Some(DataType::Int64),
                    AggregateValue::Float64Bits(_) => Some(DataType::Float64),
                    AggregateValue::Utf8(_) => Some(DataType::Utf8),
                    AggregateValue::UInt64(_) => Some(DataType::UInt64),
                    AggregateValue::Decimal128(_, scale) => Some(DataType::Decimal128(38, *scale)),
                })
                .unwrap_or(DataType::Null)
        })
        .collect::<Vec<_>>();
    grouped_aggregate_states_to_typed_batch(groups, &types)
}

/// Explicit key types are mandatory on the execution path: no values exist from
/// which to infer a type when a partition is empty or every group key is NULL.
pub fn grouped_aggregate_states_to_typed_batch(
    groups: &[GroupedAggregateState],
    group_types: &[DataType],
) -> Result<RecordBatch> {
    validate_group_layouts(groups)?;
    validate_group_key_types(groups, group_types)?;
    let mut rows = groups
        .iter()
        .enumerate()
        .map(|(index, group)| {
            if index % 1024 == 0 {
                crate::expr_eval::check_expression_cancelled()?;
            }
            Ok((
                encode_group_keys(&group.group_keys)?,
                compact_state::encode(&group.states)?,
            ))
        })
        .collect::<Result<Vec<_>>>()?;
    rows.sort_unstable_by(|left, right| left.0.cmp(&right.0));
    if rows.windows(2).any(|pair| pair[0].0 == pair[1].0) {
        return Err(exec_err("grouped aggregate state contains duplicate keys"));
    }
    let key_values = rows
        .iter()
        .map(|(keys, _)| Some(keys.as_slice()))
        .collect::<Vec<_>>();
    let state_values = rows
        .iter()
        .map(|(_, states)| Some(states.as_slice()))
        .collect::<Vec<_>>();
    let mut schema = grouped_state_schema().as_ref().clone();
    let type_schema = Schema::new(
        group_types
            .iter()
            .enumerate()
            .map(|(i, t)| Field::new(i.to_string(), t.clone(), true))
            .collect::<Vec<_>>(),
    );
    let mut type_bytes = Vec::new();
    {
        let mut writer = StreamWriter::try_new(&mut type_bytes, &type_schema)?;
        writer.finish()?;
    }
    let names = type_bytes
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    let fields = vec![
        schema
            .field(0)
            .clone()
            .with_metadata(HashMap::from([(GROUPED_KEY_TYPES.into(), names)])),
        schema.field(1).clone(),
    ];
    schema = Schema::new_with_metadata(fields, schema.metadata().clone());
    let schema = Arc::new(schema);
    Ok(RecordBatch::try_new(
        schema.clone(),
        vec![
            Arc::new(BinaryArray::from(key_values)),
            Arc::new(BinaryArray::from(state_values)),
        ],
    )?)
}

pub fn grouped_aggregate_key_types(schema: &SchemaRef) -> Result<Vec<DataType>> {
    validate_grouped_state_schema(schema)?;
    let types = schema
        .field(0)
        .metadata()
        .get(GROUPED_KEY_TYPES)
        .ok_or_else(|| exec_err("aggregate partial is missing group key types"))?;
    if !types.is_ascii() || types.len() % 2 != 0 {
        return Err(exec_err("invalid aggregate key type metadata"));
    }
    let bytes = (0..types.len())
        .step_by(2)
        .map(|i| {
            u8::from_str_radix(&types[i..i + 2], 16)
                .map_err(|_| exec_err("invalid aggregate key type metadata"))
        })
        .collect::<Result<Vec<_>>>()?;
    let reader = StreamReader::try_new(Cursor::new(bytes), None)?;
    Ok(reader
        .schema()
        .fields()
        .iter()
        .map(|f| f.data_type().clone())
        .collect())
}

pub fn grouped_aggregate_states_to_schema_batch(
    groups: &[GroupedAggregateState],
    group_types: &[DataType],
    output_types: &[DataType],
) -> Result<RecordBatch> {
    for group in groups {
        if group.states.len() != output_types.len() {
            return Err(exec_err("aggregate output type count mismatch"));
        }
        for (state, data_type) in group.states.iter().zip(output_types) {
            let expected = match state {
                AggregateState::Exact { scale: None, .. } => DataType::UInt64,
                AggregateState::Exact {
                    scale: Some(scale), ..
                } if matches!(data_type, DataType::Decimal128(_, actual) if actual == scale) => {
                    data_type.clone()
                }
                AggregateState::Count(_) | AggregateState::CountDistinct(_) => DataType::UInt64,
                AggregateState::DecimalSum { scale, .. } => DataType::Decimal128(38, *scale),
                AggregateState::IntegerSum { .. } | AggregateState::IntegerSumDistinct(_) => {
                    DataType::Int64
                }
                AggregateState::IntegerMin(_) | AggregateState::IntegerMax(_)
                    if matches!(data_type, DataType::Int32 | DataType::Int64) =>
                {
                    data_type.clone()
                }
                _ => DataType::Float64,
            };
            if &expected != data_type {
                return Err(exec_err("aggregate state/output type mismatch"));
            }
        }
    }
    let batch = grouped_aggregate_states_to_typed_batch(groups, group_types)?;
    let mut bytes = Vec::new();
    let schema = Schema::new(
        output_types
            .iter()
            .enumerate()
            .map(|(i, t)| Field::new(i.to_string(), t.clone(), true))
            .collect::<Vec<_>>(),
    );
    {
        let mut writer = StreamWriter::try_new(&mut bytes, &schema)?;
        writer.finish()?;
    }
    let types = bytes.iter().map(|b| format!("{b:02x}")).collect::<String>();
    let old = batch.schema();
    let schema = Arc::new(Schema::new_with_metadata(
        vec![
            old.field(0).clone(),
            old.field(1)
                .clone()
                .with_metadata(HashMap::from([(GROUPED_OUTPUT_TYPES.into(), types)])),
        ],
        old.metadata().clone(),
    ));
    Ok(RecordBatch::try_new(schema, batch.columns().to_vec())?)
}

pub fn grouped_aggregate_output_types(schema: &SchemaRef) -> Result<Vec<DataType>> {
    validate_grouped_state_schema(schema)?;
    let Some(types) = schema.field(1).metadata().get(GROUPED_OUTPUT_TYPES) else {
        return Ok(vec![]);
    };
    if !types.is_ascii() || types.len() % 2 != 0 {
        return Err(exec_err("invalid aggregate output type metadata"));
    }
    let bytes = (0..types.len())
        .step_by(2)
        .map(|i| {
            u8::from_str_radix(&types[i..i + 2], 16)
                .map_err(|_| exec_err("invalid aggregate output type metadata"))
        })
        .collect::<Result<Vec<_>>>()?;
    let reader = StreamReader::try_new(Cursor::new(bytes), None)?;
    Ok(reader
        .schema()
        .fields()
        .iter()
        .map(|f| f.data_type().clone())
        .collect())
}

fn validate_group_key_types(groups: &[GroupedAggregateState], types: &[DataType]) -> Result<()> {
    for group in groups {
        if group.group_keys.len() != types.len() {
            return Err(exec_err("aggregate key count does not match typed schema"));
        }
        for (key, data_type) in group.group_keys.iter().zip(types) {
            let decimal_matches = matches!((key, data_type), (AggregateValue::Decimal128(_, scale), DataType::Decimal128(_, expected)) if scale == expected);
            if !decimal_matches
                && !matches!(
                    (key, data_type),
                    (AggregateValue::Null, _)
                        | (AggregateValue::Bool(_), DataType::Boolean)
                        | (AggregateValue::Int32(_), DataType::Int32 | DataType::Date32)
                        | (
                            AggregateValue::Int64(_),
                            DataType::Int64 | DataType::Date64 | DataType::Timestamp(_, _)
                        )
                        | (AggregateValue::UInt64(_), DataType::UInt64)
                        | (AggregateValue::Float64Bits(_), DataType::Float64)
                        | (
                            AggregateValue::Utf8(_),
                            DataType::Utf8 | DataType::LargeUtf8
                        )
                )
            {
                return Err(exec_err("aggregate key value does not match typed schema"));
            }
        }
    }
    Ok(())
}

/// Decodes canonical partial-aggregate Arrow batches received through an exchange.
pub(crate) fn grouped_aggregate_state_row(
    keys: &BinaryArray,
    states: &BinaryArray,
    row: usize,
    types: &[DataType],
) -> Result<GroupedAggregateState> {
    if keys.is_null(row) || states.is_null(row) {
        return Err(exec_err("grouped aggregate state row cannot contain nulls"));
    }
    let group = GroupedAggregateState {
        group_keys: decode_group_keys(keys.value(row))?,
        states: compact_state::decode(states.value(row))?,
    };
    validate_group_key_types(std::slice::from_ref(&group), types)?;
    Ok(group)
}

pub fn grouped_aggregate_states_from_batches(
    batches: &[RecordBatch],
) -> Result<Vec<GroupedAggregateState>> {
    let mut groups = Vec::new();
    let mut expected_types = None;
    for batch in batches {
        validate_grouped_state_schema(&batch.schema())?;
        let types = grouped_aggregate_key_types(&batch.schema())?;
        if expected_types
            .as_ref()
            .is_some_and(|expected| expected != &types)
        {
            return Err(exec_err("aggregate partial group key types differ"));
        }
        expected_types = Some(types.clone());
        let keys = batch.column(0).as_binary::<i32>();
        let states = batch.column(1).as_binary::<i32>();
        for row in 0..batch.num_rows() {
            if row % 1024 == 0 {
                crate::expr_eval::check_expression_cancelled()?;
            }
            if keys.is_null(row) || states.is_null(row) {
                return Err(exec_err("grouped aggregate state row cannot contain nulls"));
            }
            groups.push(GroupedAggregateState {
                group_keys: decode_group_keys(keys.value(row))?,
                states: compact_state::decode(states.value(row))?,
            });
            validate_group_key_types(&groups[groups.len() - 1..], &types)?;
        }
    }
    Ok(groups)
}

pub fn decode_grouped_aggregate_states(bytes: &[u8]) -> Result<Vec<GroupedAggregateState>> {
    let reader = StreamReader::try_new(Cursor::new(bytes), None)?;
    validate_grouped_state_schema(&reader.schema())?;
    let types = grouped_aggregate_key_types(&reader.schema())?;
    let mut groups = Vec::new();
    let mut previous_key: Option<Vec<u8>> = None;
    for batch in reader {
        let batch = batch?;
        validate_grouped_state_schema(&batch.schema())?;
        let keys = batch.column(0).as_binary::<i32>();
        let states = batch.column(1).as_binary::<i32>();
        for row in 0..batch.num_rows() {
            if row % 1024 == 0 {
                crate::expr_eval::check_expression_cancelled()?;
            }
            if keys.is_null(row) || states.is_null(row) {
                return Err(exec_err("grouped aggregate state row cannot contain nulls"));
            }
            let encoded_key = keys.value(row);
            if previous_key
                .as_deref()
                .is_some_and(|previous| previous >= encoded_key)
            {
                return Err(exec_err(
                    "grouped aggregate state keys are not in canonical order",
                ));
            }
            groups.push(GroupedAggregateState {
                group_keys: decode_group_keys(encoded_key)?,
                states: compact_state::decode(states.value(row))?,
            });
            validate_group_key_types(&groups[groups.len() - 1..], &types)?;
            previous_key = Some(encoded_key.to_vec());
        }
    }
    Ok(groups)
}

pub fn merge_grouped_aggregate_states(
    partials: impl IntoIterator<Item = GroupedAggregateState>,
) -> Result<Vec<GroupedAggregateState>> {
    let mut merged = HashMap::<Vec<AggregateValue>, Vec<AggregateState>>::new();
    let mut expected_layout = None;
    for (index, partial) in partials.into_iter().enumerate() {
        if index % 1024 == 0 {
            crate::expr_eval::check_expression_cancelled()?;
        }
        let layout = state_layout(&partial.states)?;
        match &expected_layout {
            Some(expected) if expected != &layout => {
                return Err(exec_err(
                    "grouped aggregate partials contain incompatible state layouts",
                ));
            }
            None => expected_layout = Some(layout),
            _ => {}
        }
        if let Some(states) = merged.get_mut(&partial.group_keys) {
            for (state, other) in states.iter_mut().zip(&partial.states) {
                state.merge(other)?;
            }
        } else {
            merged.insert(partial.group_keys, partial.states);
        }
    }
    let mut groups = merged
        .into_iter()
        .map(|(group_keys, states)| {
            let encoded_key = encode_group_keys(&group_keys)?;
            Ok((encoded_key, GroupedAggregateState { group_keys, states }))
        })
        .collect::<Result<Vec<_>>>()?;
    groups.sort_unstable_by(|left, right| left.0.cmp(&right.0));
    Ok(groups.into_iter().map(|(_, group)| group).collect())
}

fn validate_group_layouts(groups: &[GroupedAggregateState]) -> Result<()> {
    let mut expected = None;
    for group in groups {
        let layout = state_layout(&group.states)?;
        match &expected {
            Some(expected) if expected != &layout => {
                return Err(exec_err(
                    "grouped aggregate states contain incompatible state layouts",
                ));
            }
            None => expected = Some(layout),
            _ => {}
        }
    }
    Ok(())
}

fn aggregate_value_to_f64(v: &AggregateValue) -> f64 {
    match v {
        AggregateValue::Int32(n) => *n as f64,
        AggregateValue::Int64(n) => *n as f64,
        AggregateValue::UInt64(n) => *n as f64,
        AggregateValue::Decimal128(n, scale) => *n as f64 / 10f64.powi(*scale as i32),
        AggregateValue::Float64Bits(b) => f64::from_bits(*b),
        _ => 0.0,
    }
}

const STATE_SUM_DISTINCT: u8 = 7;
const STATE_AVG_DISTINCT: u8 = 8;

fn state_layout(states: &[AggregateState]) -> Result<Vec<(u8, Option<i8>)>> {
    if states.is_empty() {
        return Err(exec_err(
            "grouped aggregate state requires at least one accumulator",
        ));
    }
    Ok(states
        .iter()
        .map(|state| {
            (
                match state {
                    AggregateState::DecimalSum { .. } => STATE_DECIMAL_SUM,
                    AggregateState::Exact {
                        function, distinct, ..
                    } => {
                        14 + match function {
                            AggFunc::Sum => 0,
                            AggFunc::Min => 1,
                            AggFunc::Max => 2,
                            _ => 3,
                        } + u8::from(distinct.is_some()) * 4
                    }
                    AggregateState::IntegerSum { .. } => STATE_INTEGER_SUM,
                    AggregateState::IntegerMin(_) => STATE_INTEGER_MIN,
                    AggregateState::IntegerMax(_) => STATE_INTEGER_MAX,
                    AggregateState::IntegerSumDistinct(_) => STATE_INTEGER_SUM_DISTINCT,
                    AggregateState::Sum { .. } => STATE_SUM,
                    AggregateState::Count(_) => STATE_COUNT,
                    AggregateState::Min(_) => STATE_MIN,
                    AggregateState::Max(_) => STATE_MAX,
                    AggregateState::Avg { .. } => STATE_AVG,
                    AggregateState::CountDistinct(_) => STATE_COUNT_DISTINCT,
                    AggregateState::SumDistinct(_) => STATE_SUM_DISTINCT,
                    AggregateState::AvgDistinct(_) => STATE_AVG_DISTINCT,
                },
                match state {
                    AggregateState::DecimalSum { scale, .. } => Some(*scale),
                    AggregateState::Exact { scale, .. } => *scale,
                    _ => None,
                },
            )
        })
        .collect())
}

pub fn finalize_grouped_aggregate_states(
    groups: &[GroupedAggregateState],
) -> Result<Vec<FinalizedAggregateGroup>> {
    groups
        .iter()
        .map(|group| {
            let values = group
                .states
                .iter()
                .map(|state| match state {
                    AggregateState::Exact { scale, .. } => {
                        state.exact_result().map(|value| match scale {
                            Some(scale) => FinalAggregateValue::Decimal(value, *scale),
                            None => FinalAggregateValue::Integer(value),
                        })
                    }
                    AggregateState::DecimalSum { sum, count, scale } => Ok(
                        FinalAggregateValue::Decimal((*count > 0).then_some(*sum), *scale),
                    ),
                    AggregateState::IntegerSum { .. }
                    | AggregateState::IntegerMin(_)
                    | AggregateState::IntegerMax(_)
                    | AggregateState::IntegerSumDistinct(_) => {
                        state.integer_result().map(FinalAggregateValue::Integer)
                    }
                    AggregateState::Count(_) | AggregateState::CountDistinct(_) => {
                        state.count_result().map(FinalAggregateValue::Count)
                    }
                    AggregateState::SumDistinct(_) | AggregateState::AvgDistinct(_) => {
                        state.numeric_result().map(FinalAggregateValue::Numeric)
                    }
                    _ => state.numeric_result().map(FinalAggregateValue::Numeric),
                })
                .collect::<Result<Vec<_>>>()?;
            Ok(FinalizedAggregateGroup {
                group_keys: group.group_keys.clone(),
                values,
            })
        })
        .collect()
}

fn grouped_state_schema() -> SchemaRef {
    let metadata = HashMap::from([(
        GROUPED_STATE_VERSION_KEY.to_owned(),
        GROUPED_STATE_VERSION.to_owned(),
    )]);
    Arc::new(Schema::new_with_metadata(
        vec![
            Field::new("group_keys", DataType::Binary, false),
            Field::new("aggregate_states", DataType::Binary, false),
        ],
        metadata,
    ))
}

fn validate_grouped_state_schema(schema: &SchemaRef) -> Result<()> {
    let expected = grouped_state_schema();
    if schema.fields().len() != expected.fields().len()
        || schema.fields().iter().zip(expected.fields()).any(|(a, b)| {
            a.name() != b.name()
                || a.data_type() != b.data_type()
                || a.is_nullable() != b.is_nullable()
        })
        || schema
            .metadata()
            .get(GROUPED_STATE_VERSION_KEY)
            .map(String::as_str)
            != Some(GROUPED_STATE_VERSION)
    {
        return Err(exec_err(
            "incompatible grouped aggregate state Arrow schema",
        ));
    }
    Ok(())
}

fn encode_group_keys(keys: &[AggregateValue]) -> Result<Vec<u8>> {
    let mut output = Vec::new();
    write_u64(&mut output, keys.len())?;
    for key in keys {
        let encoded = encode_group_key(key)?;
        write_u64(&mut output, encoded.len())?;
        output.extend_from_slice(&encoded);
    }
    Ok(output)
}

fn decode_group_keys(bytes: &[u8]) -> Result<Vec<AggregateValue>> {
    let mut offset = 0;
    let key_count = usize::try_from(read_u64(bytes, &mut offset)?)
        .map_err(|_| exec_err("aggregate group key count is too large"))?;
    if key_count > bytes.len().saturating_sub(offset) / 9 {
        return Err(exec_err(
            "aggregate group key count exceeds encoded payload",
        ));
    }
    let mut keys = Vec::with_capacity(key_count);
    for _ in 0..key_count {
        let length = usize::try_from(read_u64(bytes, &mut offset)?)
            .map_err(|_| exec_err("aggregate group key is too large"))?;
        let end = offset
            .checked_add(length)
            .ok_or_else(|| exec_err("aggregate group key length overflow"))?;
        let encoded = bytes
            .get(offset..end)
            .ok_or_else(|| exec_err("truncated aggregate group key"))?;
        keys.push(decode_aggregate_value(encoded)?);
        offset = end;
    }
    if offset != bytes.len() {
        return Err(exec_err("aggregate group keys contain trailing bytes"));
    }
    Ok(keys)
}

fn encode_group_key(value: &AggregateValue) -> Result<Vec<u8>> {
    if matches!(value, AggregateValue::Null) {
        Ok(vec![VALUE_NULL])
    } else {
        encode_aggregate_value(value)
    }
}

/// Encodes mergeable aggregate accumulators as a versioned Arrow IPC stream.
///
/// AVG retains both sum and count so final aggregation remains weighted, while
/// COUNT DISTINCT retains every typed value for an exact union on the receiver.
pub fn encode_aggregate_states(states: &[AggregateState]) -> Result<Vec<u8>> {
    let schema = aggregate_state_schema();
    let mut kinds = Vec::with_capacity(states.len());
    let mut sums = Vec::with_capacity(states.len());
    let mut counts = Vec::with_capacity(states.len());
    let mut extrema = Vec::with_capacity(states.len());
    let mut distinct_payloads = Vec::with_capacity(states.len());

    for state in states {
        match state {
            AggregateState::Exact {
                function,
                scale,
                value,
                distinct,
            } => {
                let mut payload = vec![
                    AggregateValue::Int32(match function {
                        AggFunc::Sum => 0,
                        AggFunc::Min => 1,
                        AggFunc::Max => 2,
                        _ => return Err(exec_err("invalid exact aggregate")),
                    }),
                    AggregateValue::Int32(scale.map_or(256, i32::from)),
                    AggregateValue::Bool(distinct.is_some()),
                    value.map_or(AggregateValue::Null, |n| AggregateValue::Decimal128(n, 0)),
                ];
                if let Some(values) = distinct {
                    let mut encoded = values
                        .iter()
                        .map(|v| Ok((encode_aggregate_value(v)?, v.clone())))
                        .collect::<Result<Vec<_>>>()?;
                    encoded.sort_by(|a, b| a.0.cmp(&b.0));
                    payload.extend(encoded.into_iter().map(|(_, v)| v));
                }
                kinds.push(14);
                sums.push(None);
                counts.push(None);
                extrema.push(None);
                distinct_payloads.push(Some(encode_group_keys(&payload)?));
            }
            AggregateState::IntegerSum { sum, count } => {
                kinds.push(STATE_INTEGER_SUM);
                sums.push(None);
                counts.push(Some(*count));
                extrema.push(None);
                distinct_payloads.push(Some(encode_aggregate_value(&AggregateValue::Decimal128(
                    *sum, 0,
                ))?));
            }
            AggregateState::IntegerMin(value) | AggregateState::IntegerMax(value) => {
                kinds.push(if matches!(state, AggregateState::IntegerMin(_)) {
                    STATE_INTEGER_MIN
                } else {
                    STATE_INTEGER_MAX
                });
                sums.push(None);
                counts.push(None);
                extrema.push(None);
                distinct_payloads.push(
                    value
                        .map(|value| encode_aggregate_value(&AggregateValue::Int64(value)))
                        .transpose()?,
                );
            }
            AggregateState::DecimalSum { sum, count, scale } => {
                kinds.push(STATE_DECIMAL_SUM);
                sums.push(None);
                counts.push(Some(*count));
                extrema.push(None);
                distinct_payloads.push(Some(encode_aggregate_value(&AggregateValue::Decimal128(
                    *sum, *scale,
                ))?));
            }
            AggregateState::Sum { sum, count } => {
                kinds.push(STATE_SUM);
                sums.push(Some(*sum));
                counts.push(Some(*count));
                extrema.push(None);
                distinct_payloads.push(None);
            }
            AggregateState::Count(count) => {
                kinds.push(STATE_COUNT);
                sums.push(None);
                counts.push(Some(*count));
                extrema.push(None);
                distinct_payloads.push(None);
            }
            AggregateState::Min(value) => {
                kinds.push(STATE_MIN);
                sums.push(None);
                counts.push(None);
                extrema.push(*value);
                distinct_payloads.push(None);
            }
            AggregateState::Max(value) => {
                kinds.push(STATE_MAX);
                sums.push(None);
                counts.push(None);
                extrema.push(*value);
                distinct_payloads.push(None);
            }
            AggregateState::Avg { sum, count } => {
                kinds.push(STATE_AVG);
                sums.push(Some(*sum));
                counts.push(Some(*count));
                extrema.push(None);
                distinct_payloads.push(None);
            }
            AggregateState::CountDistinct(values)
            | AggregateState::IntegerSumDistinct(values)
            | AggregateState::SumDistinct(values)
            | AggregateState::AvgDistinct(values) => {
                let tag = match state {
                    AggregateState::CountDistinct(_) => STATE_COUNT_DISTINCT,
                    AggregateState::IntegerSumDistinct(_) => STATE_INTEGER_SUM_DISTINCT,
                    AggregateState::SumDistinct(_) => STATE_SUM_DISTINCT,
                    AggregateState::AvgDistinct(_) => STATE_AVG_DISTINCT,
                    _ => unreachable!(),
                };
                kinds.push(tag);
                sums.push(None);
                counts.push(None);
                extrema.push(None);
                distinct_payloads.push(Some(encode_distinct_values(values)?));
            }
        }
    }

    let distinct_slices = distinct_payloads
        .iter()
        .map(|payload| payload.as_deref())
        .collect::<Vec<_>>();
    let batch = RecordBatch::try_new(
        schema.clone(),
        vec![
            Arc::new(UInt8Array::from(kinds)),
            Arc::new(Float64Array::from(sums)),
            Arc::new(UInt64Array::from(counts)),
            Arc::new(Float64Array::from(extrema)),
            Arc::new(BinaryArray::from(distinct_slices)),
        ],
    )?;
    let mut bytes = Vec::new();
    {
        let mut writer = StreamWriter::try_new(&mut bytes, &schema)?;
        writer.write(&batch)?;
        writer.finish()?;
    }
    Ok(bytes)
}

/// Decodes and validates aggregate accumulators from the versioned Arrow wire format.
pub fn decode_aggregate_states(bytes: &[u8]) -> Result<Vec<AggregateState>> {
    let reader = StreamReader::try_new(Cursor::new(bytes), None)?;
    validate_aggregate_state_schema(&reader.schema())?;
    let mut states = Vec::new();
    for batch in reader {
        let batch = batch?;
        validate_aggregate_state_schema(&batch.schema())?;
        let kinds = batch
            .column(0)
            .as_primitive::<arrow::datatypes::UInt8Type>();
        let sums = batch.column(1).as_primitive::<Float64Type>();
        let counts = batch
            .column(2)
            .as_primitive::<arrow::datatypes::UInt64Type>();
        let extrema = batch.column(3).as_primitive::<Float64Type>();
        let distinct = batch.column(4).as_binary::<i32>();
        for row in 0..batch.num_rows() {
            if kinds.is_null(row) {
                return Err(exec_err("aggregate state kind cannot be null"));
            }
            let state = match kinds.value(row) {
                14 => {
                    if distinct.is_null(row) {
                        return Err(exec_err("missing exact numeric payload"));
                    }
                    let values = decode_group_keys(distinct.value(row))?;
                    let [
                        AggregateValue::Int32(function),
                        AggregateValue::Int32(scale),
                        AggregateValue::Bool(is_distinct),
                        value,
                        tail @ ..,
                    ] = values.as_slice()
                    else {
                        return Err(exec_err("invalid exact numeric payload"));
                    };
                    let function = match function {
                        0 => AggFunc::Sum,
                        1 => AggFunc::Min,
                        2 => AggFunc::Max,
                        _ => return Err(exec_err("invalid exact function")),
                    };
                    let scale = if *scale == 256 {
                        None
                    } else {
                        Some(i8::try_from(*scale).map_err(|_| exec_err("invalid exact scale"))?)
                    };
                    let value = match value {
                        AggregateValue::Null => None,
                        AggregateValue::Decimal128(n, 0) => Some(*n),
                        _ => return Err(exec_err("invalid exact value")),
                    };
                    if (!*is_distinct && !tail.is_empty())
                        || (*is_distinct && (value.is_some() || !matches!(function, AggFunc::Sum)))
                    {
                        return Err(exec_err("invalid exact distinct state"));
                    }
                    let mut state = AggregateState::Exact {
                        function,
                        scale,
                        value,
                        distinct: is_distinct.then(HashSet::new),
                    };
                    for item in tail {
                        state.update_exact(item.clone())?;
                    }
                    state
                }

                STATE_SUM => AggregateState::Sum {
                    sum: required_f64(sums, row, "sum")?,
                    count: required_u64(counts, row, "sum count")?,
                },
                STATE_COUNT => AggregateState::Count(required_u64(counts, row, "count")?),
                STATE_MIN => AggregateState::Min(optional_f64(extrema, row)),
                STATE_MAX => AggregateState::Max(optional_f64(extrema, row)),
                STATE_AVG => AggregateState::Avg {
                    sum: required_f64(sums, row, "average sum")?,
                    count: required_u64(counts, row, "average count")?,
                },
                STATE_COUNT_DISTINCT => {
                    if distinct.is_null(row) {
                        return Err(exec_err("COUNT DISTINCT state payload cannot be null"));
                    }
                    AggregateState::CountDistinct(decode_distinct_values(distinct.value(row))?)
                }
                STATE_SUM_DISTINCT | STATE_INTEGER_SUM_DISTINCT => {
                    if distinct.is_null(row) {
                        return Err(exec_err("SUM DISTINCT state payload cannot be null"));
                    }
                    let values = decode_distinct_values(distinct.value(row))?;
                    if kinds.value(row) == STATE_INTEGER_SUM_DISTINCT {
                        AggregateState::IntegerSumDistinct(values)
                    } else {
                        AggregateState::SumDistinct(values)
                    }
                }
                STATE_INTEGER_SUM => {
                    if distinct.is_null(row) {
                        return Err(exec_err("integer SUM payload cannot be null"));
                    }
                    let AggregateValue::Decimal128(sum, 0) =
                        decode_aggregate_value(distinct.value(row))?
                    else {
                        return Err(exec_err("invalid integer SUM payload"));
                    };
                    AggregateState::IntegerSum {
                        sum,
                        count: required_u64(counts, row, "count")?,
                    }
                }
                STATE_INTEGER_MIN | STATE_INTEGER_MAX => {
                    let value = if distinct.is_null(row) {
                        None
                    } else {
                        let AggregateValue::Int64(value) =
                            decode_aggregate_value(distinct.value(row))?
                        else {
                            return Err(exec_err("invalid integer extremum payload"));
                        };
                        Some(value)
                    };
                    if kinds.value(row) == STATE_INTEGER_MIN {
                        AggregateState::IntegerMin(value)
                    } else {
                        AggregateState::IntegerMax(value)
                    }
                }
                STATE_DECIMAL_SUM => {
                    if distinct.is_null(row) {
                        return Err(exec_err("decimal SUM payload cannot be null"));
                    }
                    let AggregateValue::Decimal128(sum, scale) =
                        decode_aggregate_value(distinct.value(row))?
                    else {
                        return Err(exec_err("invalid decimal SUM payload"));
                    };
                    if !(-38..=38).contains(&scale) {
                        return Err(exec_err("invalid decimal SUM scale"));
                    }
                    AggregateState::DecimalSum {
                        sum,
                        count: required_u64(counts, row, "count")?,
                        scale,
                    }
                }
                STATE_AVG_DISTINCT => {
                    if distinct.is_null(row) {
                        return Err(exec_err("AVG DISTINCT state payload cannot be null"));
                    }
                    AggregateState::AvgDistinct(decode_distinct_values(distinct.value(row))?)
                }
                kind => return Err(exec_err(format!("unknown aggregate state kind {kind}"))),
            };
            states.push(state);
        }
    }
    Ok(states)
}

fn aggregate_state_schema() -> SchemaRef {
    let metadata = HashMap::from([(
        AGGREGATE_STATE_VERSION_KEY.to_owned(),
        AGGREGATE_STATE_VERSION.to_owned(),
    )]);
    Arc::new(Schema::new_with_metadata(
        vec![
            Field::new("state_kind", DataType::UInt8, false),
            Field::new("sum", DataType::Float64, true),
            Field::new("count", DataType::UInt64, true),
            Field::new("extremum", DataType::Float64, true),
            Field::new("distinct_values", DataType::Binary, true),
        ],
        metadata,
    ))
}

fn validate_aggregate_state_schema(schema: &SchemaRef) -> Result<()> {
    let expected = aggregate_state_schema();
    if schema.fields() != expected.fields()
        || schema
            .metadata()
            .get(AGGREGATE_STATE_VERSION_KEY)
            .map(String::as_str)
            != Some(AGGREGATE_STATE_VERSION)
    {
        return Err(exec_err("incompatible aggregate state Arrow schema"));
    }
    Ok(())
}

fn required_f64(array: &Float64Array, row: usize, field: &str) -> Result<f64> {
    if array.is_null(row) {
        return Err(exec_err(format!("aggregate state {field} cannot be null")));
    }
    Ok(array.value(row))
}

fn required_u64(array: &UInt64Array, row: usize, field: &str) -> Result<u64> {
    if array.is_null(row) {
        return Err(exec_err(format!("aggregate state {field} cannot be null")));
    }
    Ok(array.value(row))
}

fn optional_f64(array: &Float64Array, row: usize) -> Option<f64> {
    (!array.is_null(row)).then(|| array.value(row))
}

fn encode_distinct_values(values: &HashSet<AggregateValue>) -> Result<Vec<u8>> {
    let mut encoded = values
        .iter()
        .map(encode_aggregate_value)
        .collect::<Result<Vec<_>>>()?;
    encoded.sort_unstable();
    let mut output = Vec::new();
    write_u64(&mut output, encoded.len())?;
    for value in encoded {
        write_u64(&mut output, value.len())?;
        output.extend_from_slice(&value);
    }
    Ok(output)
}

fn encode_aggregate_value(value: &AggregateValue) -> Result<Vec<u8>> {
    let mut output = Vec::new();
    match value {
        AggregateValue::UInt64(value) => {
            output.push(VALUE_UINT64);
            output.extend_from_slice(&value.to_le_bytes());
        }
        AggregateValue::Decimal128(value, scale) => {
            output.push(VALUE_DECIMAL128);
            output.push(*scale as u8);
            output.extend_from_slice(&value.to_le_bytes());
        }
        AggregateValue::Null => return Err(exec_err("null cannot appear in a distinct state")),
        AggregateValue::Bool(value) => {
            output.push(VALUE_BOOL);
            output.push(u8::from(*value));
        }
        AggregateValue::Int32(value) => {
            output.push(VALUE_INT32);
            output.extend_from_slice(&value.to_le_bytes());
        }
        AggregateValue::Int64(value) => {
            output.push(VALUE_INT64);
            output.extend_from_slice(&value.to_le_bytes());
        }
        AggregateValue::Utf8(value) => {
            output.push(VALUE_UTF8);
            output.extend_from_slice(value.as_bytes());
        }
        AggregateValue::Float64Bits(value) => {
            output.push(VALUE_FLOAT64_BITS);
            output.extend_from_slice(&value.to_le_bytes());
        }
    }
    Ok(output)
}

fn decode_distinct_values(bytes: &[u8]) -> Result<HashSet<AggregateValue>> {
    let mut offset = 0;
    let value_count = read_u64(bytes, &mut offset)?;
    if value_count > (bytes.len().saturating_sub(offset) / 9) as u64 {
        return Err(exec_err("distinct state count exceeds encoded payload"));
    }
    let mut values = HashSet::with_capacity(
        usize::try_from(value_count).map_err(|_| exec_err("distinct state is too large"))?,
    );
    for _ in 0..value_count {
        let length = usize::try_from(read_u64(bytes, &mut offset)?)
            .map_err(|_| exec_err("distinct value is too large"))?;
        let end = offset
            .checked_add(length)
            .ok_or_else(|| exec_err("distinct state length overflow"))?;
        let encoded = bytes
            .get(offset..end)
            .ok_or_else(|| exec_err("truncated distinct state payload"))?;
        let value = decode_aggregate_value(encoded)?;
        if matches!(value, AggregateValue::Null) {
            return Err(exec_err("null cannot appear in a distinct state"));
        }
        values.insert(value);
        offset = end;
    }
    if offset != bytes.len() {
        return Err(exec_err("distinct state payload has trailing bytes"));
    }
    Ok(values)
}

fn decode_aggregate_value(bytes: &[u8]) -> Result<AggregateValue> {
    let (&tag, payload) = bytes
        .split_first()
        .ok_or_else(|| exec_err("empty distinct value payload"))?;
    match tag {
        VALUE_UINT64 if payload.len() == 8 => Ok(AggregateValue::UInt64(u64::from_le_bytes(
            payload.try_into().unwrap(),
        ))),
        VALUE_DECIMAL128 if payload.len() == 17 => Ok(AggregateValue::Decimal128(
            i128::from_le_bytes(payload[1..].try_into().unwrap()),
            payload[0] as i8,
        )),
        VALUE_NULL if payload.is_empty() => Ok(AggregateValue::Null),
        VALUE_BOOL if payload.len() == 1 && payload[0] <= 1 => {
            Ok(AggregateValue::Bool(payload[0] == 1))
        }
        VALUE_INT32 if payload.len() == size_of::<i32>() => Ok(AggregateValue::Int32(
            i32::from_le_bytes(payload.try_into().expect("validated i32 length")),
        )),
        VALUE_INT64 if payload.len() == size_of::<i64>() => Ok(AggregateValue::Int64(
            i64::from_le_bytes(payload.try_into().expect("validated i64 length")),
        )),
        VALUE_UTF8 => Ok(AggregateValue::Utf8(
            std::str::from_utf8(payload)
                .map_err(|_| exec_err("distinct string is not valid UTF-8"))?
                .to_owned(),
        )),
        VALUE_FLOAT64_BITS if payload.len() == size_of::<u64>() => Ok(AggregateValue::Float64Bits(
            u64::from_le_bytes(payload.try_into().expect("validated f64 bit length")),
        )),
        _ => Err(exec_err("invalid typed distinct value payload")),
    }
}

fn write_u64(output: &mut Vec<u8>, value: usize) -> Result<()> {
    let value = u64::try_from(value).map_err(|_| exec_err("aggregate state is too large"))?;
    output.extend_from_slice(&value.to_le_bytes());
    Ok(())
}

fn read_u64(bytes: &[u8], offset: &mut usize) -> Result<u64> {
    let end = offset
        .checked_add(size_of::<u64>())
        .ok_or_else(|| exec_err("aggregate state offset overflow"))?;
    let encoded = bytes
        .get(*offset..end)
        .ok_or_else(|| exec_err("truncated aggregate state payload"))?;
    *offset = end;
    Ok(u64::from_le_bytes(
        encoded.try_into().expect("validated u64 length"),
    ))
}

type Accumulator = AggregateState;

const HASH_ENTRY_OVERHEAD_BYTES: u64 = 64;
const VECTOR_OVERHEAD_BYTES: u64 = 24;
const DISTINCT_ENTRY_OVERHEAD_BYTES: u64 = 32;

pub struct HashAggregate {
    source: Box<dyn BatchOperator>,
    group_by: Vec<String>,
    aggregates: Vec<AggExpr>,
    output_schema: SchemaRef,
    memory: Option<OperatorMemoryAccount>,
    emitted: bool,
    input_already_reserved: bool,
}

impl HashAggregate {
    fn new_states(&self) -> Vec<AggregateState> {
        self.aggregates
            .iter()
            .enumerate()
            .map(|(i, expression)| {
                AggregateState::new_typed(
                    expression,
                    self.output_schema
                        .field(self.group_by.len() + i)
                        .data_type(),
                )
            })
            .collect()
    }
    pub fn new(
        source: Box<dyn BatchOperator>,
        group_by: Vec<String>,
        aggregates: Vec<AggExpr>,
    ) -> Result<Self> {
        if aggregates.is_empty() {
            return Err(KaveonError::Execution(
                "at least one aggregate expression required".into(),
            ));
        }
        let source_schema = source.schema().clone();
        for col in &group_by {
            source_schema
                .index_of(col)
                .map_err(|_| exec_err(format!("group-by column '{col}' not in input")))?;
            let data_type = source_schema.field_with_name(col)?.data_type();
            if !matches!(
                data_type,
                DataType::Null
                    | DataType::Boolean
                    | DataType::Int32
                    | DataType::Int64
                    | DataType::UInt64
                    | DataType::Decimal128(_, _)
                    | DataType::Date32
                    | DataType::Date64
                    | DataType::Timestamp(_, _)
                    | DataType::Float64
                    | DataType::Utf8
                    | DataType::LargeUtf8
            ) {
                return Err(exec_err(format!(
                    "unsupported GROUP BY key type: {data_type}"
                )));
            }
        }
        for agg in &aggregates {
            if agg.distinct && matches!(agg.func, AggFunc::Min | AggFunc::Max) {
                return Err(exec_err("DISTINCT is not supported for MIN/MAX"));
            }
            if agg.distinct && agg.column == "*" {
                return Err(exec_err("COUNT(DISTINCT *) is not supported"));
            }
            if !matches!(agg.func, AggFunc::Count) {
                let index = source_schema.index_of(&agg.column).map_err(|_| {
                    exec_err(format!("aggregate column '{}' not in input", agg.column))
                })?;
                if !is_numeric_type(source_schema.field(index).data_type()) {
                    return Err(exec_err(format!(
                        "{} requires a numeric column, got {}",
                        agg.output_name(),
                        source_schema.field(index).data_type()
                    )));
                }
            } else if agg.column != "*" {
                source_schema.index_of(&agg.column).map_err(|_| {
                    exec_err(format!("aggregate column '{}' not in input", agg.column))
                })?;
            }
        }

        let mut fields: Vec<Field> = group_by
            .iter()
            .map(|col| {
                let f = source_schema.field_with_name(col).unwrap();
                f.clone()
            })
            .collect();
        for (agg, data_type) in aggregates
            .iter()
            .zip(aggregate_output_types(&aggregates, &source_schema)?)
        {
            fields.push(Field::new(agg.output_name(), data_type, true));
        }
        let output_schema = Arc::new(Schema::new(fields));

        Ok(Self {
            source,
            group_by,
            aggregates,
            output_schema,
            memory: None,
            emitted: false,
            input_already_reserved: false,
        })
    }

    pub fn new_with_memory(
        source: Box<dyn BatchOperator>,
        group_by: Vec<String>,
        aggregates: Vec<AggExpr>,
        memory: OperatorMemoryAccount,
    ) -> Result<Self> {
        let mut operator = Self::new(source, group_by, aggregates)?;
        operator.memory = Some(memory);
        Ok(operator)
    }

    /// The source must retain a query reservation covering every yielded buffer
    /// until its next next_batch call. Intended for guarded local worker channels.
    pub(crate) fn with_reserved_input(mut self) -> Self {
        self.input_already_reserved = true;
        self
    }

    /// Consumes the input and returns mergeable per-group accumulator state.
    pub fn into_grouped_states(self) -> Result<Vec<GroupedAggregateState>> {
        self.into_grouped_states_with_reservations()
            .map(|(groups, _)| groups)
    }

    /// Keeps accumulator reservations alive across partial-state serialization.
    /// Callers retaining the states must retain the returned reservations too.
    pub fn into_grouped_states_with_reservations(
        mut self,
    ) -> Result<(Vec<GroupedAggregateState>, Vec<MemoryReservation>)> {
        let (groups, reservations) = self.collect_states()?;
        if groups.is_empty() && !self.group_by.is_empty() {
            return Ok((Vec::new(), reservations));
        }
        let groups = if groups.is_empty() {
            vec![(Vec::new(), self.new_states())]
        } else {
            groups.into_iter().collect()
        };
        Ok((
            groups
                .into_iter()
                .map(|(keys, states)| GroupedAggregateState {
                    group_keys: keys.into_iter().map(AggregateValue::from).collect(),
                    states,
                })
                .collect(),
            reservations,
        ))
    }

    fn collect_states(&mut self) -> Result<(GroupStateMap, Vec<MemoryReservation>)> {
        // Group keys are query-local and do not need the standard library's
        // comparatively expensive SipHash. AHash retains per-map randomized
        // seeds while materially reducing the hot-path cost of large GROUP BYs.
        let mut groups: AHashMap<InlineGroupKey, Vec<Accumulator>> = AHashMap::new();
        let mut reservations = Vec::new();
        while let Some(batch) = self.source.next_batch()? {
            let _input_memory = if self.input_already_reserved {
                None
            } else {
                self.memory
                    .as_ref()
                    .map(|account| account.reserve(batch.get_array_memory_size() as u64))
                    .transpose()?
            };
            let schema = batch.schema();
            let group_arrays = self
                .group_by
                .iter()
                .map(|column| batch.column(schema.index_of(column).unwrap()))
                .collect::<Vec<_>>();
            let aggregate_arrays = self
                .aggregates
                .iter()
                .map(|aggregate| {
                    (!(matches!(aggregate.func, AggFunc::Count) && aggregate.column == "*"))
                        .then(|| batch.column(schema.index_of(&aggregate.column).unwrap()))
                })
                .collect::<Vec<_>>();
            if self.group_by.is_empty() {
                if groups.is_empty()
                    && let Some(memory) = &self.memory
                {
                    reservations
                        .push(memory.reserve(estimated_group_bytes(&[], self.aggregates.len()))?);
                }
                let states = groups
                    .entry(InlineGroupKey::Empty)
                    .or_insert_with(|| self.new_states());
                for (index, aggregate) in self.aggregates.iter().enumerate() {
                    let state = &mut states[index];
                    if let AggregateState::Count(count) = state {
                        let count_batch = aggregate_arrays[index]
                            .map_or(batch.num_rows(), |a| a.len() - a.null_count());
                        *count = count
                            .checked_add(count_batch as u64)
                            .ok_or_else(|| exec_err("COUNT overflow"))?;
                        continue;
                    }
                    let array =
                        aggregate_arrays[index].expect("non-count aggregate requires input");
                    for row in 0..batch.num_rows() {
                        if row % 1024 == 0
                            && let Some(memory) = &self.memory
                        {
                            memory.check_cancelled()?;
                        }
                        if array.is_null(row) {
                            continue;
                        }
                        if aggregate.distinct {
                            let value: AggregateValue = extract_key(array, row).into();
                            if distinct_value_is_new(state, &value)
                                && let Some(memory) = &self.memory
                            {
                                reservations
                                    .push(memory.reserve(estimated_distinct_value_bytes(&value))?);
                            }
                            state.update_distinct(value)?;
                        } else if matches!(state, AggregateState::Exact { .. }) {
                            state.update_exact(extract_key(array, row).into())?;
                        } else if matches!(state, AggregateState::DecimalSum { .. }) {
                            state.update_decimal(
                                array
                                    .as_primitive::<arrow::datatypes::Decimal128Type>()
                                    .value(row),
                            )?;
                        } else if matches!(
                            state,
                            AggregateState::IntegerSum { .. }
                                | AggregateState::IntegerMin(_)
                                | AggregateState::IntegerMax(_)
                        ) {
                            let value = if array.data_type() == &DataType::Int32 {
                                array.as_primitive::<Int32Type>().value(row) as i64
                            } else {
                                array.as_primitive::<Int64Type>().value(row)
                            };
                            state.update_integer(value)?;
                        } else {
                            state.update_numeric(extract_f64(array, row)?)?;
                        }
                    }
                }
                continue;
            }
            for row in 0..batch.num_rows() {
                if row % 1024 == 0
                    && let Some(memory) = &self.memory
                {
                    memory.check_cancelled()?;
                }
                let key = if group_arrays.len() == 1 {
                    InlineGroupKey::Single(extract_key(group_arrays[0], row))
                } else {
                    InlineGroupKey::Multiple(
                        group_arrays
                            .iter()
                            .map(|array| extract_key(array, row))
                            .collect(),
                    )
                };
                // Use the entry probe for both admission and lookup. The old
                // contains_key + entry sequence hashed and probed every group
                // key twice, including every row of high-cardinality scans.
                let accumulators = match groups.entry(key) {
                    Entry::Occupied(entry) => entry.into_mut(),
                    Entry::Vacant(entry) => {
                        if let Some(memory) = &self.memory {
                            reservations.push(memory.reserve(estimated_group_bytes(
                                entry.key().as_slice(),
                                self.aggregates.len(),
                            ))?);
                        }
                        entry.insert(self.new_states())
                    }
                };
                for (index, aggregate) in self.aggregates.iter().enumerate() {
                    if matches!(aggregate.func, AggFunc::Count) && aggregate.column == "*" {
                        accumulators[index].update_count()?;
                    } else if let Some(array) = aggregate_arrays[index]
                        && !array.is_null(row)
                    {
                        if aggregate.distinct {
                            let value: AggregateValue = extract_key(array, row).into();
                            if distinct_value_is_new(&accumulators[index], &value)
                                && let Some(memory) = &self.memory
                            {
                                reservations
                                    .push(memory.reserve(estimated_distinct_value_bytes(&value))?);
                            }
                            accumulators[index].update_distinct(value)?;
                        } else if matches!(aggregate.func, AggFunc::Count) {
                            accumulators[index].update_count()?;
                        } else if matches!(accumulators[index], AggregateState::Exact { .. }) {
                            accumulators[index].update_exact(extract_key(array, row).into())?;
                        } else if matches!(accumulators[index], AggregateState::DecimalSum { .. }) {
                            accumulators[index].update_decimal(
                                array
                                    .as_primitive::<arrow::datatypes::Decimal128Type>()
                                    .value(row),
                            )?;
                        } else if matches!(
                            accumulators[index],
                            AggregateState::IntegerSum { .. }
                                | AggregateState::IntegerMin(_)
                                | AggregateState::IntegerMax(_)
                        ) {
                            let value = if array.data_type() == &DataType::Int32 {
                                array.as_primitive::<Int32Type>().value(row) as i64
                            } else {
                                array.as_primitive::<Int64Type>().value(row)
                            };
                            accumulators[index].update_integer(value)?;
                        } else {
                            accumulators[index].update_numeric(extract_f64(array, row)?)?;
                        }
                    }
                }
            }
        }
        Ok((
            groups
                .into_iter()
                .map(|(key, states)| (key.into_vec(), states))
                .collect(),
            reservations,
        ))
    }
}

impl BatchOperator for HashAggregate {
    fn schema(&self) -> &SchemaRef {
        &self.output_schema
    }

    fn next_batch(&mut self) -> Result<Option<RecordBatch>> {
        if self.emitted {
            return Ok(None);
        }
        self.emitted = true;

        let (groups, _reservations) = self.collect_states()?;
        let num_aggs = self.aggregates.len();

        if groups.is_empty() && !self.group_by.is_empty() {
            return Ok(Some(RecordBatch::new_empty(self.output_schema.clone())));
        }

        let entries: Vec<(Vec<GroupKey>, Vec<Accumulator>)> = if groups.is_empty() {
            vec![(vec![], self.new_states())]
        } else {
            groups.into_iter().collect()
        };

        let output_bytes = entries.iter().fold(0_u64, |bytes, (keys, _)| {
            bytes.saturating_add(estimated_group_bytes(keys, num_aggs))
        });
        let _output_memory = self
            .memory
            .as_ref()
            .map(|memory| memory.reserve(output_bytes))
            .transpose()?;
        let mut columns: Vec<ArrayRef> = Vec::with_capacity(self.group_by.len() + num_aggs);

        for (gi, col_name) in self.group_by.iter().enumerate() {
            let field = self.output_schema.field_with_name(col_name).unwrap();
            let arr = build_group_column(&entries, gi, field.data_type())?;
            columns.push(arr);
        }

        for ai in 0..self.aggregates.len() {
            match self
                .output_schema
                .field(self.group_by.len() + ai)
                .data_type()
            {
                DataType::Int32 => {
                    let values = entries
                        .iter()
                        .map(|(_, states)| {
                            states[ai]
                                .integer_result()?
                                .map(|v| {
                                    i32::try_from(v)
                                        .map_err(|_| exec_err("integer aggregate overflow"))
                                })
                                .transpose()
                        })
                        .collect::<Result<Vec<_>>>()?;
                    columns.push(Arc::new(Int32Array::from(values)));
                }
                DataType::Int64 => {
                    let values = entries
                        .iter()
                        .map(|(_, states)| {
                            states[ai]
                                .integer_result()?
                                .map(|v| {
                                    i64::try_from(v).map_err(|_| exec_err("integer SUM overflow"))
                                })
                                .transpose()
                        })
                        .collect::<Result<Vec<_>>>()?;
                    columns.push(Arc::new(Int64Array::from(values)));
                }
                DataType::Decimal128(precision, scale) => {
                    let values = entries
                        .iter()
                        .map(|(_, states)| match &states[ai] {
                            AggregateState::Exact { .. } => states[ai].exact_result(),
                            AggregateState::DecimalSum { sum, count, .. } => {
                                Ok((*count > 0).then_some(*sum))
                            }
                            _ => Err(exec_err("decimal SUM output state mismatch")),
                        })
                        .collect::<Result<Vec<_>>>()?;
                    let array = arrow::array::Decimal128Array::from(values)
                        .with_precision_and_scale(*precision, *scale)?;
                    array.validate_decimal_precision(*precision)?;
                    columns.push(Arc::new(array));
                }
                DataType::UInt64 => {
                    let values: Result<Vec<Option<u64>>> = entries
                        .iter()
                        .map(|(_, accums)| match &accums[ai] {
                            AggregateState::Exact { .. } => accums[ai]
                                .exact_result()?
                                .map(|n| {
                                    u64::try_from(n)
                                        .map_err(|_| exec_err("UInt64 aggregate overflow"))
                                })
                                .transpose(),
                            _ => accums[ai].count_result().map(Some),
                        })
                        .collect();
                    columns.push(Arc::new(UInt64Array::from(values?)));
                }
                _ => {
                    let values: Result<Vec<Option<f64>>> = entries
                        .iter()
                        .map(|(_, accums)| accums[ai].numeric_result())
                        .collect();
                    columns.push(Arc::new(Float64Array::from(values?)));
                }
            }
        }

        let batch = RecordBatch::try_new(self.output_schema.clone(), columns)?;
        Ok(Some(batch))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum GroupKey {
    Null,
    Bool(bool),
    Int32(i32),
    Int64(i64),
    UInt64(u64),
    Decimal128(i128, i8),
    Utf8(String),
    Float64Bits(u64),
}

type GroupStateMap = Vec<(Vec<GroupKey>, Vec<Accumulator>)>;

#[derive(Debug, PartialEq, Eq, Hash)]
enum InlineGroupKey {
    Empty,
    Single(GroupKey),
    Multiple(Vec<GroupKey>),
}
impl InlineGroupKey {
    fn as_slice(&self) -> &[GroupKey] {
        match self {
            Self::Empty => &[],
            Self::Single(key) => std::slice::from_ref(key),
            Self::Multiple(keys) => keys,
        }
    }
    fn into_vec(self) -> Vec<GroupKey> {
        match self {
            Self::Empty => vec![],
            Self::Single(key) => vec![key],
            Self::Multiple(keys) => keys,
        }
    }
}

fn estimated_group_bytes(key: &[GroupKey], aggregate_count: usize) -> u64 {
    let key_bytes = key
        .iter()
        .map(|value| match value {
            GroupKey::Utf8(value) => value.len() as u64,
            GroupKey::Null => 0,
            GroupKey::Bool(_) => 1,
            GroupKey::Int32(_) => 4,
            GroupKey::Int64(_) | GroupKey::UInt64(_) | GroupKey::Float64Bits(_) => 8,
            GroupKey::Decimal128(_, _) => 17,
        })
        .sum::<u64>();
    let fixed_keys = u64::try_from(key.len())
        .unwrap_or(u64::MAX)
        .saturating_mul(std::mem::size_of::<GroupKey>() as u64);
    let accumulators = u64::try_from(aggregate_count)
        .unwrap_or(u64::MAX)
        .saturating_mul(std::mem::size_of::<Accumulator>() as u64);
    HASH_ENTRY_OVERHEAD_BYTES
        .saturating_add(VECTOR_OVERHEAD_BYTES.saturating_mul(2))
        .saturating_add(fixed_keys)
        .saturating_add(key_bytes)
        .saturating_add(accumulators)
}

fn distinct_value_is_new(state: &AggregateState, value: &AggregateValue) -> bool {
    if let AggregateState::Exact {
        distinct: Some(values),
        ..
    } = state
    {
        return !values.contains(value);
    }
    !matches!(value, AggregateValue::Null)
        && matches!(
            state,
            AggregateState::CountDistinct(values)
            | AggregateState::IntegerSumDistinct(values)
            | AggregateState::SumDistinct(values)
            | AggregateState::AvgDistinct(values)
            if !values.contains(value)
        )
}

fn estimated_distinct_value_bytes(value: &AggregateValue) -> u64 {
    let payload = match value {
        AggregateValue::Utf8(value) => value.len() as u64,
        AggregateValue::Null => 0,
        AggregateValue::Bool(_) => 1,
        AggregateValue::Int32(_) => 4,
        AggregateValue::Int64(_) | AggregateValue::UInt64(_) | AggregateValue::Float64Bits(_) => 8,
        AggregateValue::Decimal128(_, _) => 17,
    };
    DISTINCT_ENTRY_OVERHEAD_BYTES
        .saturating_add(std::mem::size_of::<AggregateValue>() as u64)
        .saturating_add(payload)
}

impl From<GroupKey> for AggregateValue {
    fn from(value: GroupKey) -> Self {
        match value {
            GroupKey::Null => Self::Null,
            GroupKey::Bool(value) => Self::Bool(value),
            GroupKey::Int32(value) => Self::Int32(value),
            GroupKey::Int64(value) => Self::Int64(value),
            GroupKey::UInt64(value) => Self::UInt64(value),
            GroupKey::Decimal128(value, scale) => Self::Decimal128(value, scale),
            GroupKey::Utf8(value) => Self::Utf8(value),
            GroupKey::Float64Bits(value) => Self::Float64Bits(value),
        }
    }
}

fn extract_key(arr: &ArrayRef, row: usize) -> GroupKey {
    if arr.is_null(row) {
        return GroupKey::Null;
    }
    match arr.data_type() {
        DataType::Boolean => GroupKey::Bool(
            arr.as_any()
                .downcast_ref::<BooleanArray>()
                .unwrap()
                .value(row),
        ),
        DataType::Int32 => GroupKey::Int32(arr.as_primitive::<Int32Type>().value(row)),
        DataType::Int64 => GroupKey::Int64(arr.as_primitive::<Int64Type>().value(row)),
        DataType::UInt64 => GroupKey::UInt64(
            arr.as_primitive::<arrow::datatypes::UInt64Type>()
                .value(row),
        ),
        DataType::Decimal128(_, scale) => GroupKey::Decimal128(
            arr.as_primitive::<arrow::datatypes::Decimal128Type>()
                .value(row),
            *scale,
        ),
        DataType::Date32 => GroupKey::Int32(
            arr.as_primitive::<arrow::datatypes::Date32Type>()
                .value(row),
        ),
        DataType::Date64 => GroupKey::Int64(
            arr.as_primitive::<arrow::datatypes::Date64Type>()
                .value(row),
        ),
        DataType::Timestamp(unit, _) => GroupKey::Int64(match unit {
            arrow::datatypes::TimeUnit::Second => arr
                .as_primitive::<arrow::datatypes::TimestampSecondType>()
                .value(row),
            arrow::datatypes::TimeUnit::Millisecond => arr
                .as_primitive::<arrow::datatypes::TimestampMillisecondType>()
                .value(row),
            arrow::datatypes::TimeUnit::Microsecond => arr
                .as_primitive::<arrow::datatypes::TimestampMicrosecondType>()
                .value(row),
            arrow::datatypes::TimeUnit::Nanosecond => arr
                .as_primitive::<arrow::datatypes::TimestampNanosecondType>()
                .value(row),
        }),
        DataType::Float64 => {
            let value = arr.as_primitive::<Float64Type>().value(row);
            GroupKey::Float64Bits(if value == 0.0 {
                0
            } else if value.is_nan() {
                f64::NAN.to_bits()
            } else {
                value.to_bits()
            })
        }
        DataType::Utf8 => GroupKey::Utf8(arr.as_string::<i32>().value(row).to_owned()),
        DataType::LargeUtf8 => GroupKey::Utf8(arr.as_string::<i64>().value(row).to_owned()),
        _ => GroupKey::Utf8(format!("{:?}", arr.slice(row, 1))),
    }
}

fn extract_f64(arr: &ArrayRef, row: usize) -> Result<f64> {
    let value = match arr.data_type() {
        DataType::Float64 => arr.as_primitive::<Float64Type>().value(row),
        DataType::Float32 => arr
            .as_primitive::<arrow::datatypes::Float32Type>()
            .value(row) as f64,
        DataType::Int64 => arr.as_primitive::<Int64Type>().value(row) as f64,
        DataType::Int32 => arr.as_primitive::<Int32Type>().value(row) as f64,
        DataType::UInt64 => arr
            .as_primitive::<arrow::datatypes::UInt64Type>()
            .value(row) as f64,
        DataType::Decimal128(_, scale) => {
            let v = arr
                .as_any()
                .downcast_ref::<arrow::array::Decimal128Array>()
                .expect("Decimal128")
                .value(row);
            v as f64 / 10f64.powi(*scale as i32)
        }
        _ => {
            return Err(exec_err(format!(
                "expected numeric aggregate input, got {}",
                arr.data_type()
            )));
        }
    };
    Ok(value)
}

fn is_numeric_type(data_type: &DataType) -> bool {
    matches!(
        data_type,
        DataType::Float32
            | DataType::Float64
            | DataType::Int32
            | DataType::Int64
            | DataType::UInt64
            | DataType::Decimal128(_, _)
    )
}

fn build_group_column(
    entries: &[(Vec<GroupKey>, Vec<Accumulator>)],
    group_index: usize,
    data_type: &DataType,
) -> Result<ArrayRef> {
    let keys = entries
        .iter()
        .map(|(keys, _)| AggregateValue::from(keys[group_index].clone()))
        .collect::<Vec<_>>();
    aggregate_key_column(&keys, data_type)
}

/// Reconstruct exact typed grouping keys, including empty and all-NULL columns.
pub fn aggregate_key_column(keys: &[AggregateValue], data_type: &DataType) -> Result<ArrayRef> {
    macro_rules! values {
        ($variant:ident) => {
            keys.iter()
                .map(|key| match key {
                    AggregateValue::$variant(value) => Some(*value),
                    _ => None,
                })
                .collect::<Vec<_>>()
        };
    }
    let result: ArrayRef = match data_type {
        DataType::Null => arrow::array::new_null_array(data_type, keys.len()),
        DataType::Boolean => Arc::new(BooleanArray::from(values!(Bool))),
        DataType::Int32 => Arc::new(Int32Array::from(values!(Int32))),
        DataType::Int64 => Arc::new(Int64Array::from(values!(Int64))),
        DataType::UInt64 => Arc::new(UInt64Array::from(values!(UInt64))),
        DataType::Decimal128(precision, scale) => {
            let values = keys
                .iter()
                .map(|key| match key {
                    AggregateValue::Decimal128(value, _) => Some(*value),
                    _ => None,
                })
                .collect::<Vec<_>>();
            let array = arrow::array::Decimal128Array::from(values)
                .with_precision_and_scale(*precision, *scale)?;
            array.validate_decimal_precision(*precision)?;
            Arc::new(array)
        }
        DataType::Date32 => arrow::compute::cast(&Int32Array::from(values!(Int32)), data_type)?,
        DataType::Date64 | DataType::Timestamp(_, _) => {
            arrow::compute::cast(&Int64Array::from(values!(Int64)), data_type)?
        }
        DataType::Float64 => Arc::new(Float64Array::from(
            keys.iter()
                .map(|key| match key {
                    AggregateValue::Float64Bits(bits) => Some(f64::from_bits(*bits)),
                    _ => None,
                })
                .collect::<Vec<_>>(),
        )),
        DataType::Utf8 => Arc::new(StringArray::from(
            keys.iter()
                .map(|key| match key {
                    AggregateValue::Utf8(value) => Some(value.as_str()),
                    _ => None,
                })
                .collect::<Vec<_>>(),
        )),
        DataType::LargeUtf8 => Arc::new(arrow::array::LargeStringArray::from(
            keys.iter()
                .map(|key| match key {
                    AggregateValue::Utf8(value) => Some(value.as_str()),
                    _ => None,
                })
                .collect::<Vec<_>>(),
        )),
        _ => {
            return Err(exec_err(format!(
                "unsupported aggregate group type {data_type}"
            )));
        }
    };
    Ok(result)
}

fn exec_err(msg: impl Into<String>) -> KaveonError {
    KaveonError::Execution(msg.into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exact_unsigned_and_decimal_aggregates_survive_typed_partial_merges() {
        let cases: Vec<(ArrayRef, Vec<i128>)> = vec![
            (
                Arc::new(UInt64Array::from(vec![
                    Some(u64::MAX - 2),
                    Some(1),
                    Some(1),
                    None,
                ])),
                vec![
                    u64::MAX as i128,
                    1,
                    (u64::MAX - 2) as i128,
                    (u64::MAX - 1) as i128,
                ],
            ),
            (
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
                vec![5, -100000000000000000000, 100000000000000000001, 3],
            ),
        ];
        for (array, expected) in cases {
            let schema = Arc::new(Schema::new(vec![Field::new(
                "v",
                array.data_type().clone(),
                true,
            )]));
            let batch = RecordBatch::try_new(schema.clone(), vec![array]).unwrap();
            let expressions = vec![
                AggExpr::new(AggFunc::Sum, "v"),
                AggExpr::new(AggFunc::Min, "v"),
                AggExpr::new(AggFunc::Max, "v"),
                AggExpr::new(AggFunc::Sum, "v").distinct(),
            ];
            let types = aggregate_output_types(&expressions, &schema).unwrap();
            let mut local = HashAggregate::new(
                Box::new(Input::new(batch.clone())),
                vec![],
                expressions.clone(),
            )
            .unwrap();
            let actual = local.next_batch().unwrap().unwrap();
            for (index, expected) in expected.iter().enumerate() {
                let value = match actual.column(index).data_type() {
                    DataType::UInt64 => actual
                        .column(index)
                        .as_primitive::<arrow::datatypes::UInt64Type>()
                        .value(0) as i128,
                    DataType::Decimal128(_, _) => actual
                        .column(index)
                        .as_primitive::<arrow::datatypes::Decimal128Type>()
                        .value(0),
                    _ => panic!("lost exact type"),
                };
                assert_eq!(value, *expected);
            }
            let mut partials = Vec::new();
            for start in 0..batch.num_rows() {
                let groups = HashAggregate::new(
                    Box::new(Input::new(batch.slice(start, 1))),
                    vec![],
                    expressions.clone(),
                )
                .unwrap()
                .into_grouped_states()
                .unwrap();
                let encoded =
                    grouped_aggregate_states_to_schema_batch(&groups, &[], &types).unwrap();
                assert_eq!(
                    grouped_aggregate_output_types(&encoded.schema()).unwrap(),
                    types
                );
                partials.extend(grouped_aggregate_states_from_batches(&[encoded]).unwrap());
            }
            let merged = merge_grouped_aggregate_states(partials).unwrap();
            let final_values = finalize_grouped_aggregate_states(&merged).unwrap();
            for (value, expected) in final_values[0].values.iter().zip(expected) {
                let actual = match value {
                    FinalAggregateValue::Integer(Some(n))
                    | FinalAggregateValue::Decimal(Some(n), _) => *n,
                    _ => panic!("lost final exact value"),
                };
                assert_eq!(actual, expected);
            }
            for empty in [
                RecordBatch::new_empty(schema.clone()),
                RecordBatch::try_new(
                    schema.clone(),
                    vec![arrow::array::new_null_array(schema.field(0).data_type(), 3)],
                )
                .unwrap(),
            ] {
                let mut operator =
                    HashAggregate::new(Box::new(Input::new(empty)), vec![], expressions.clone())
                        .unwrap();
                let result = operator.next_batch().unwrap().unwrap();
                assert!(result.columns().iter().all(|array| array.is_null(0)));
                assert_eq!(
                    result
                        .schema()
                        .fields()
                        .iter()
                        .map(|f| f.data_type().clone())
                        .collect::<Vec<_>>(),
                    types
                );
            }
        }
        let batch = RecordBatch::try_from_iter(vec![(
            "v",
            Arc::new(UInt64Array::from(vec![u64::MAX, 1])) as ArrayRef,
        )])
        .unwrap();
        let mut operator = HashAggregate::new(
            Box::new(Input::new(batch)),
            vec![],
            vec![AggExpr::new(AggFunc::Sum, "v")],
        )
        .unwrap();
        assert!(operator.next_batch().is_err());
    }

    #[test]
    fn signed_integer_aggregates_and_distinct_are_exact_above_f64_precision() {
        let schema = Arc::new(Schema::new(vec![Field::new("v", DataType::Int64, true)]));
        let batch = RecordBatch::try_new(
            schema.clone(),
            vec![Arc::new(Int64Array::from(vec![
                Some(9007199254740993),
                Some(-9007199254740992),
                Some(2),
                Some(2),
                None,
            ]))],
        )
        .unwrap();
        let expressions = vec![
            AggExpr::new(AggFunc::Sum, "v"),
            AggExpr::new(AggFunc::Min, "v"),
            AggExpr::new(AggFunc::Max, "v"),
            AggExpr::new(AggFunc::Sum, "v").distinct(),
        ];
        let mut local = HashAggregate::new(
            Box::new(Input::new(batch.clone())),
            vec![],
            expressions.clone(),
        )
        .unwrap();
        let output = local.next_batch().unwrap().unwrap();
        let actual = output
            .columns()
            .iter()
            .map(|a| a.as_primitive::<Int64Type>().value(0))
            .collect::<Vec<_>>();
        assert_eq!(actual, vec![5, -9007199254740992, 9007199254740993, 3]);
        let partial = HashAggregate::new(Box::new(Input::new(batch)), vec![], expressions)
            .unwrap()
            .into_grouped_states()
            .unwrap();
        assert_eq!(
            decode_aggregate_states(&encode_aggregate_states(&partial[0].states).unwrap()).unwrap(),
            partial[0].states
        );
        let typed =
            grouped_aggregate_states_to_schema_batch(&partial, &[], &vec![DataType::Int64; 4])
                .unwrap();
        assert_eq!(
            grouped_aggregate_output_types(&typed.schema()).unwrap(),
            vec![DataType::Int64; 4]
        );
        let overflow =
            RecordBatch::try_new(schema, vec![Arc::new(Int64Array::from(vec![i64::MAX, 1]))])
                .unwrap();
        let mut aggregate = HashAggregate::new(
            Box::new(Input::new(overflow)),
            vec![],
            vec![AggExpr::new(AggFunc::Sum, "v")],
        )
        .unwrap();
        assert!(aggregate.next_batch().is_err());
    }

    #[test]
    fn decimal_sum_preserves_precision_scale_partial_state_and_null_identity() {
        let schema = Arc::new(Schema::new(vec![Field::new(
            "v",
            DataType::Decimal128(30, 4),
            true,
        )]));
        let values = arrow::array::Decimal128Array::from(vec![
            Some(100000000000000000001i128),
            Some(2),
            None,
        ])
        .with_precision_and_scale(30, 4)
        .unwrap();
        let batch = RecordBatch::try_new(schema.clone(), vec![Arc::new(values)]).unwrap();
        let expression = AggExpr::new(AggFunc::Sum, "v");
        let mut local = HashAggregate::new(
            Box::new(Input::new(batch.clone())),
            vec![],
            vec![expression.clone()],
        )
        .unwrap();
        assert_eq!(
            local.schema().field(0).data_type(),
            &DataType::Decimal128(38, 4)
        );
        let output = local.next_batch().unwrap().unwrap();
        assert_eq!(
            output
                .column(0)
                .as_primitive::<arrow::datatypes::Decimal128Type>()
                .value(0),
            100000000000000000003i128
        );
        let partial = HashAggregate::new(
            Box::new(Input::new(batch)),
            vec![],
            vec![expression.clone()],
        )
        .unwrap()
        .into_grouped_states()
        .unwrap();
        let encoded =
            grouped_aggregate_states_to_schema_batch(&partial, &[], &[DataType::Decimal128(38, 4)])
                .unwrap();
        assert_eq!(
            grouped_aggregate_output_types(&encoded.schema()).unwrap(),
            vec![DataType::Decimal128(38, 4)]
        );
        let decoded = grouped_aggregate_states_from_batches(&[encoded]).unwrap();
        assert_eq!(decoded, partial);
        let merged = merge_grouped_aggregate_states(decoded.into_iter().chain(partial)).unwrap();
        assert_eq!(
            finalize_grouped_aggregate_states(&merged).unwrap()[0].values,
            vec![FinalAggregateValue::Decimal(
                Some(200000000000000000006i128),
                4
            )]
        );
        let mut empty = HashAggregate::new(
            Box::new(Input::new(RecordBatch::new_empty(schema))),
            vec![],
            vec![expression],
        )
        .unwrap();
        assert!(empty.next_batch().unwrap().unwrap().column(0).is_null(0));
        let mut state = AggregateState::DecimalSum {
            sum: i128::MAX,
            count: 1,
            scale: 4,
        };
        assert!(state.update_decimal(1).is_err());
    }

    #[test]
    fn typed_partial_schema_survives_empty_and_null_groups() {
        for groups in [
            vec![],
            vec![GroupedAggregateState {
                group_keys: vec![AggregateValue::Null],
                states: vec![AggregateState::Count(1)],
            }],
        ] {
            let batch =
                grouped_aggregate_states_to_typed_batch(&groups, &[DataType::Int64]).unwrap();
            let mut bytes = Vec::new();
            {
                let mut writer = StreamWriter::try_new(&mut bytes, &batch.schema()).unwrap();
                writer.write(&batch).unwrap();
                writer.finish().unwrap();
            }
            let reader = StreamReader::try_new(Cursor::new(&bytes), None).unwrap();
            assert_eq!(
                grouped_aggregate_key_types(&reader.schema()).unwrap(),
                vec![DataType::Int64]
            );
            assert_eq!(decode_grouped_aggregate_states(&bytes).unwrap(), groups);
        }
        let wrong = vec![GroupedAggregateState {
            group_keys: vec![AggregateValue::Utf8("x".into())],
            states: vec![AggregateState::Count(1)],
        }];
        assert!(grouped_aggregate_states_to_typed_batch(&wrong, &[DataType::Int64]).is_err());
    }

    #[test]
    fn malformed_state_counts_fail_before_allocating_declared_capacity() {
        assert!(decode_group_keys(&u64::MAX.to_le_bytes()).is_err());
        assert!(decode_distinct_values(&u64::MAX.to_le_bytes()).is_err());
    }

    #[test]
    fn exact_typed_group_keys_round_trip_local_and_partial() {
        use arrow::datatypes::TimeUnit;
        let cases = [
            (DataType::UInt64, AggregateValue::UInt64(u64::MAX)),
            (
                DataType::Decimal128(38, 7),
                AggregateValue::Decimal128(9999999999999999999999999999999999999, 7),
            ),
            (DataType::Date32, AggregateValue::Int32(20000)),
            (DataType::Date64, AggregateValue::Int64(1728000000000)),
            (
                DataType::Timestamp(TimeUnit::Nanosecond, Some("UTC".into())),
                AggregateValue::Int64(1728000000000000001),
            ),
            (
                DataType::Timestamp(TimeUnit::Microsecond, None),
                AggregateValue::Int64(-1728000000000001),
            ),
        ];
        for (data_type, key) in cases {
            let keys = vec![key.clone(), key.clone(), AggregateValue::Null];
            let col = aggregate_key_column(&keys, &data_type).unwrap();
            let schema = Arc::new(Schema::new(vec![Field::new(
                "key",
                data_type.clone(),
                true,
            )]));
            let batch = RecordBatch::try_new(schema, vec![col]).unwrap();
            let mut local = HashAggregate::new(
                Box::new(Input::new(batch.clone())),
                vec!["key".into()],
                vec![AggExpr::new(AggFunc::Count, "*")],
            )
            .unwrap();
            let output = local.next_batch().unwrap().unwrap();
            assert_eq!(output.schema().field(0).data_type(), &data_type);
            assert_eq!(output.num_rows(), 2);
            let aggregate = HashAggregate::new(
                Box::new(Input::new(batch)),
                vec!["key".into()],
                vec![AggExpr::new(AggFunc::Count, "*")],
            )
            .unwrap();
            let partials = aggregate.into_grouped_states().unwrap();
            assert!(
                partials.iter().any(|g| g.group_keys == vec![key.clone()]
                    && g.states == vec![AggregateState::Count(2)])
            );
            let encoded = grouped_aggregate_states_to_typed_batch(
                &partials,
                std::slice::from_ref(&data_type),
            )
            .unwrap();
            assert_eq!(
                grouped_aggregate_key_types(&encoded.schema()).unwrap(),
                vec![data_type.clone()]
            );
            let decoded = grouped_aggregate_states_from_batches(&[encoded]).unwrap();
            assert!(decoded.iter().any(|g| g.group_keys == vec![key.clone()]));
            let empty =
                grouped_aggregate_states_to_typed_batch(&[], std::slice::from_ref(&data_type))
                    .unwrap();
            assert_eq!(
                grouped_aggregate_key_types(&empty.schema()).unwrap(),
                vec![data_type]
            );
        }
    }
    use std::collections::VecDeque;

    struct Input {
        schema: SchemaRef,
        batches: VecDeque<RecordBatch>,
    }
    impl Input {
        fn new(batch: RecordBatch) -> Self {
            Self {
                schema: batch.schema(),
                batches: VecDeque::from([batch]),
            }
        }
    }
    impl BatchOperator for Input {
        fn schema(&self) -> &SchemaRef {
            &self.schema
        }
        fn next_batch(&mut self) -> Result<Option<RecordBatch>> {
            Ok(self.batches.pop_front())
        }
    }

    #[test]
    fn count_distinct_ignores_nulls_and_deduplicates_values() {
        let schema = Arc::new(Schema::new(vec![Field::new("value", DataType::Utf8, true)]));
        let batch = RecordBatch::try_new(
            schema,
            vec![Arc::new(StringArray::from(vec![
                Some("a"),
                Some("a"),
                None,
                Some("b"),
            ]))],
        )
        .unwrap();
        let mut aggregate = HashAggregate::new(
            Box::new(Input::new(batch)),
            Vec::new(),
            vec![AggExpr::new(AggFunc::Count, "value").distinct()],
        )
        .unwrap();
        let result = aggregate.next_batch().unwrap().unwrap();
        assert_eq!(
            result
                .column(0)
                .as_primitive::<arrow::datatypes::UInt64Type>()
                .value(0),
            2
        );
    }

    #[test]
    fn memory_aware_aggregate_bounds_group_and_distinct_state() {
        let schema = Arc::new(Schema::new(vec![
            Field::new("group_key", DataType::Utf8, false),
            Field::new("value", DataType::Utf8, false),
        ]));
        let batch = RecordBatch::try_new(
            schema,
            vec![
                Arc::new(StringArray::from(vec!["a", "b", "c"])),
                Arc::new(StringArray::from(vec!["one", "two", "three"])),
            ],
        )
        .unwrap();
        let pool = kaveon_core::QueryMemoryPool::new("bounded-aggregate", 256).unwrap();
        let account = pool.operator("hash-aggregate").unwrap();
        let mut aggregate = HashAggregate::new_with_memory(
            Box::new(Input::new(batch)),
            vec!["group_key".into()],
            vec![AggExpr::new(AggFunc::Count, "value").distinct()],
            account,
        )
        .unwrap();

        let error = aggregate.next_batch().unwrap_err().to_string();
        assert!(error.contains("query 'bounded-aggregate' operator 'hash-aggregate'"));
        drop(aggregate);
        assert_eq!(pool.snapshot().current_bytes, 0);
        assert!(pool.snapshot().peak_bytes <= pool.snapshot().limit_bytes);
    }

    #[test]
    fn high_cardinality_integer_groups_are_exact_and_memory_bounded() {
        let rows = 250_000_i64;
        let groups = 4_096_i64;
        let batch = RecordBatch::try_from_iter(vec![
            (
                "group_key",
                Arc::new(Int64Array::from_iter_values(
                    (0..rows).map(|row| row % groups),
                )) as ArrayRef,
            ),
            (
                "value",
                Arc::new(Int64Array::from_iter_values(0..rows)) as ArrayRef,
            ),
        ])
        .unwrap();
        let pool = kaveon_core::QueryMemoryPool::new("high-cardinality", 32 * 1024 * 1024).unwrap();
        let mut aggregate = HashAggregate::new_with_memory(
            Box::new(Input::new(batch)),
            vec!["group_key".into()],
            vec![
                AggExpr::new(AggFunc::Count, "*"),
                AggExpr::new(AggFunc::Sum, "value"),
            ],
            pool.operator("hash-aggregate").unwrap(),
        )
        .unwrap();
        let output = aggregate.next_batch().unwrap().unwrap();
        assert_eq!(output.num_rows(), groups as usize);
        let keys = output.column(0).as_primitive::<Int64Type>();
        let counts = output
            .column(1)
            .as_primitive::<arrow::datatypes::UInt64Type>();
        let sums = output.column(2).as_primitive::<Int64Type>();
        let actual = (0..output.num_rows())
            .map(|row| (keys.value(row), (counts.value(row), sums.value(row))))
            .collect::<std::collections::BTreeMap<_, _>>();
        for key in 0..groups {
            let expected_values = (key..rows).step_by(groups as usize).collect::<Vec<_>>();
            assert_eq!(
                actual[&key],
                (
                    expected_values.len() as u64,
                    expected_values.iter().sum::<i64>()
                )
            );
        }
        drop(output);
        drop(aggregate);
        let snapshot = pool.snapshot();
        assert_eq!(snapshot.current_bytes, 0);
        assert!(snapshot.peak_bytes <= snapshot.limit_bytes);
    }

    #[test]
    fn rejects_nonnumeric_sum_instead_of_silently_returning_zero() {
        let schema = Arc::new(Schema::new(vec![Field::new(
            "value",
            DataType::Utf8,
            false,
        )]));
        let batch =
            RecordBatch::try_new(schema, vec![Arc::new(StringArray::from(vec!["a"]))]).unwrap();
        let result = HashAggregate::new(
            Box::new(Input::new(batch)),
            Vec::new(),
            vec![AggExpr::new(AggFunc::Sum, "value")],
        );
        assert!(
            matches!(result, Err(KaveonError::Execution(message)) if message.contains("numeric"))
        );
    }

    #[test]
    fn empty_global_aggregate_returns_zero_count_and_null_sum() {
        let schema = Arc::new(Schema::new(vec![Field::new(
            "value",
            DataType::Int64,
            true,
        )]));
        let mut aggregate = HashAggregate::new(
            Box::new(Input::new(RecordBatch::new_empty(schema))),
            Vec::new(),
            vec![
                AggExpr::new(AggFunc::Count, "*"),
                AggExpr::new(AggFunc::Sum, "value"),
            ],
        )
        .unwrap();
        let result = aggregate.next_batch().unwrap().unwrap();
        assert_eq!(
            result
                .column(0)
                .as_primitive::<arrow::datatypes::UInt64Type>()
                .value(0),
            0
        );
        assert!(result.column(1).is_null(0));
    }

    #[test]
    fn average_state_merges_weighted_partials() {
        let expression = AggExpr::new(AggFunc::Avg, "value");
        let mut left = AggregateState::new(&expression);
        left.update_numeric(10.0).unwrap();
        left.update_numeric(20.0).unwrap();
        let mut right = AggregateState::new(&expression);
        right.update_numeric(100.0).unwrap();

        left.merge(&right).unwrap();

        assert_eq!(left.numeric_result().unwrap(), Some(130.0 / 3.0));
    }

    #[test]
    fn distinct_count_state_unions_overlapping_partials() {
        let expression = AggExpr::new(AggFunc::Count, "value").distinct();
        let mut left = AggregateState::new(&expression);
        left.update_distinct(AggregateValue::Utf8("a".into()))
            .unwrap();
        left.update_distinct(AggregateValue::Utf8("b".into()))
            .unwrap();
        let mut right = AggregateState::new(&expression);
        right
            .update_distinct(AggregateValue::Utf8("b".into()))
            .unwrap();
        right
            .update_distinct(AggregateValue::Utf8("c".into()))
            .unwrap();

        left.merge(&right).unwrap();

        assert_eq!(left.count_result().unwrap(), 3);
    }

    #[test]
    fn aggregate_states_reject_incompatible_merges() {
        let mut count = AggregateState::new(&AggExpr::new(AggFunc::Count, "*"));
        let sum = AggregateState::new(&AggExpr::new(AggFunc::Sum, "value"));

        let result = count.merge(&sum);

        assert!(
            matches!(result, Err(KaveonError::Execution(message)) if message.contains("incompatible"))
        );
    }

    #[test]
    fn aggregate_state_arrow_stream_round_trips_every_state_kind() {
        let distinct = HashSet::from([
            AggregateValue::Bool(true),
            AggregateValue::Int32(-7),
            AggregateValue::Int64(9),
            AggregateValue::Utf8("east".into()),
            AggregateValue::Float64Bits((-0.0_f64).to_bits()),
        ]);
        let states = vec![
            AggregateState::Sum {
                sum: 12.5,
                count: 3,
            },
            AggregateState::Count(4),
            AggregateState::Min(None),
            AggregateState::Max(Some(99.0)),
            AggregateState::Avg {
                sum: 130.0,
                count: 3,
            },
            AggregateState::CountDistinct(distinct),
        ];

        let encoded = encode_aggregate_states(&states).unwrap();
        let decoded = decode_aggregate_states(&encoded).unwrap();

        assert_eq!(decoded, states);
    }

    #[test]
    fn decoded_average_merges_with_weighted_count() {
        let left = AggregateState::Avg {
            sum: 30.0,
            count: 2,
        };
        let right = AggregateState::Avg {
            sum: 100.0,
            count: 1,
        };
        let mut decoded_left = decode_aggregate_states(&encode_aggregate_states(&[left]).unwrap())
            .unwrap()
            .remove(0);
        let decoded_right = decode_aggregate_states(&encode_aggregate_states(&[right]).unwrap())
            .unwrap()
            .remove(0);

        decoded_left.merge(&decoded_right).unwrap();

        assert_eq!(decoded_left.numeric_result().unwrap(), Some(130.0 / 3.0));
    }

    #[test]
    fn decoded_distinct_states_union_exact_values() {
        let left = AggregateState::CountDistinct(HashSet::from([
            AggregateValue::Utf8("a".into()),
            AggregateValue::Utf8("b".into()),
        ]));
        let right = AggregateState::CountDistinct(HashSet::from([
            AggregateValue::Utf8("b".into()),
            AggregateValue::Utf8("c".into()),
        ]));
        let mut decoded_left = decode_aggregate_states(&encode_aggregate_states(&[left]).unwrap())
            .unwrap()
            .remove(0);
        let decoded_right = decode_aggregate_states(&encode_aggregate_states(&[right]).unwrap())
            .unwrap()
            .remove(0);

        decoded_left.merge(&decoded_right).unwrap();

        assert_eq!(decoded_left.count_result().unwrap(), 3);
    }

    #[test]
    fn rejects_incompatible_aggregate_state_arrow_schema() {
        let schema = Arc::new(Schema::new(vec![Field::new(
            "state_kind",
            DataType::UInt8,
            false,
        )]));
        let batch = RecordBatch::try_new(
            schema.clone(),
            vec![Arc::new(UInt8Array::from(vec![STATE_COUNT]))],
        )
        .unwrap();
        let mut bytes = Vec::new();
        {
            let mut writer = StreamWriter::try_new(&mut bytes, &schema).unwrap();
            writer.write(&batch).unwrap();
            writer.finish().unwrap();
        }

        let result = decode_aggregate_states(&bytes);

        assert!(
            matches!(result, Err(KaveonError::Execution(message)) if message.contains("incompatible"))
        );
    }

    fn grouped_state(
        key: AggregateValue,
        average: (f64, u64),
        distinct: &[&str],
    ) -> GroupedAggregateState {
        GroupedAggregateState {
            group_keys: vec![key],
            states: vec![
                AggregateState::Avg {
                    sum: average.0,
                    count: average.1,
                },
                AggregateState::CountDistinct(
                    distinct
                        .iter()
                        .map(|value| AggregateValue::Utf8((*value).into()))
                        .collect(),
                ),
            ],
        }
    }

    #[test]
    fn grouped_state_arrow_encoding_is_canonical_and_round_trips_null_keys() {
        let first = grouped_state(AggregateValue::Utf8("west".into()), (30.0, 2), &["a", "b"]);
        let second = grouped_state(AggregateValue::Null, (5.0, 1), &["c"]);

        let forward = encode_grouped_aggregate_states(&[first.clone(), second.clone()]).unwrap();
        let reverse = encode_grouped_aggregate_states(&[second, first]).unwrap();

        assert_eq!(forward, reverse);
        let decoded = decode_grouped_aggregate_states(&forward).unwrap();
        assert_eq!(decoded.len(), 2);
        assert!(
            decoded
                .iter()
                .any(|group| group.group_keys == vec![AggregateValue::Null])
        );
    }

    #[test]
    fn grouped_merge_preserves_weighted_average_and_exact_distinct_union() {
        let partials = vec![
            grouped_state(AggregateValue::Utf8("west".into()), (30.0, 2), &["a", "b"]),
            grouped_state(AggregateValue::Utf8("west".into()), (90.0, 1), &["b", "c"]),
            grouped_state(AggregateValue::Utf8("east".into()), (20.0, 1), &["z"]),
        ];

        let merged = merge_grouped_aggregate_states(partials).unwrap();
        let finalized = finalize_grouped_aggregate_states(&merged).unwrap();
        let west = finalized
            .iter()
            .find(|group| group.group_keys == vec![AggregateValue::Utf8("west".into())])
            .unwrap();

        assert_eq!(
            west.values,
            vec![
                FinalAggregateValue::Numeric(Some(40.0)),
                FinalAggregateValue::Count(3),
            ]
        );
    }

    #[test]
    fn grouped_state_rejects_incompatible_accumulator_layouts() {
        let groups = vec![
            GroupedAggregateState {
                group_keys: vec![AggregateValue::Int64(1)],
                states: vec![AggregateState::Count(1)],
            },
            GroupedAggregateState {
                group_keys: vec![AggregateValue::Int64(2)],
                states: vec![AggregateState::Avg { sum: 2.0, count: 1 }],
            },
        ];

        assert!(encode_grouped_aggregate_states(&groups).is_err());
        assert!(merge_grouped_aggregate_states(groups).is_err());
    }
}
