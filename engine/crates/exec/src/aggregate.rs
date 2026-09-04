use arrow::array::{
    Array, ArrayRef, AsArray, BinaryArray, BooleanArray, Float64Array, Int32Array, Int64Array,
    StringArray, UInt8Array, UInt64Array,
};
use arrow::datatypes::{DataType, Field, Float64Type, Int32Type, Int64Type, Schema, SchemaRef};
use arrow::ipc::reader::StreamReader;
use arrow::ipc::writer::StreamWriter;
use arrow::record_batch::RecordBatch;
use kaveon_core::{BatchOperator, KaveonError, Result};
use std::collections::{HashMap, HashSet};
use std::io::Cursor;
use std::sync::Arc;

const AGGREGATE_STATE_VERSION_KEY: &str = "kaveon.aggregate_state.version";
const AGGREGATE_STATE_VERSION: &str = "1";
const STATE_SUM: u8 = 1;
const STATE_COUNT: u8 = 2;
const STATE_MIN: u8 = 3;
const STATE_MAX: u8 = 4;
const STATE_AVG: u8 = 5;
const STATE_COUNT_DISTINCT: u8 = 6;
const VALUE_BOOL: u8 = 1;
const VALUE_INT32: u8 = 2;
const VALUE_INT64: u8 = 3;
const VALUE_UTF8: u8 = 4;
const VALUE_FLOAT64_BITS: u8 = 5;

#[derive(Debug, Clone, Copy)]
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

    fn output_name(&self) -> String {
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
    Utf8(String),
    Float64Bits(u64),
}

#[derive(Debug, Clone, PartialEq)]
pub enum AggregateState {
    Sum { sum: f64, count: u64 },
    Count(u64),
    Min(Option<f64>),
    Max(Option<f64>),
    Avg { sum: f64, count: u64 },
    CountDistinct(HashSet<AggregateValue>),
}

impl AggregateState {
    pub fn new(expression: &AggExpr) -> Self {
        match (expression.func, expression.distinct) {
            (AggFunc::Sum, false) => Self::Sum { sum: 0.0, count: 0 },
            (AggFunc::Count, false) => Self::Count(0),
            (AggFunc::Min, false) => Self::Min(None),
            (AggFunc::Max, false) => Self::Max(None),
            (AggFunc::Avg, false) => Self::Avg { sum: 0.0, count: 0 },
            (AggFunc::Count, true) => Self::CountDistinct(HashSet::new()),
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
        match self {
            Self::CountDistinct(values) => {
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
        match (self, partial) {
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
            (Self::CountDistinct(values), Self::CountDistinct(other)) => {
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
            _ => Err(exec_err(
                "numeric result requested from a count aggregate state",
            )),
        }
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
            AggregateState::CountDistinct(values) => {
                kinds.push(STATE_COUNT_DISTINCT);
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
        values.insert(decode_aggregate_value(encoded)?);
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

pub struct HashAggregate {
    source: Box<dyn BatchOperator>,
    group_by: Vec<String>,
    aggregates: Vec<AggExpr>,
    output_schema: SchemaRef,
    emitted: bool,
}

impl HashAggregate {
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
        }
        for agg in &aggregates {
            if agg.distinct && !matches!(agg.func, AggFunc::Count) {
                return Err(exec_err("DISTINCT is currently supported only for COUNT"));
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
        for agg in &aggregates {
            fields.push(Field::new(agg.output_name(), agg.output_type(), true));
        }
        let output_schema = Arc::new(Schema::new(fields));

        Ok(Self {
            source,
            group_by,
            aggregates,
            output_schema,
            emitted: false,
        })
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

        let mut groups: HashMap<Vec<GroupKey>, Vec<Accumulator>> = HashMap::new();
        let num_aggs = self.aggregates.len();

        while let Some(batch) = self.source.next_batch()? {
            let schema = batch.schema();

            let group_arrays: Vec<&ArrayRef> = self
                .group_by
                .iter()
                .map(|col| {
                    let idx = schema.index_of(col).unwrap();
                    batch.column(idx)
                })
                .collect();

            let agg_arrays: Vec<Option<&ArrayRef>> = self
                .aggregates
                .iter()
                .map(|agg| {
                    if matches!(agg.func, AggFunc::Count) && agg.column == "*" {
                        None
                    } else {
                        Some(batch.column(schema.index_of(&agg.column).unwrap()))
                    }
                })
                .collect();

            for row in 0..batch.num_rows() {
                let key: Vec<GroupKey> = group_arrays
                    .iter()
                    .map(|arr| extract_key(arr, row))
                    .collect();

                let accums = groups
                    .entry(key)
                    .or_insert_with(|| self.aggregates.iter().map(AggregateState::new).collect());

                for (i, agg) in self.aggregates.iter().enumerate() {
                    if matches!(agg.func, AggFunc::Count) && agg.column == "*" {
                        accums[i].update_count()?;
                    } else if let Some(arr) = agg_arrays[i]
                        && !arr.is_null(row)
                    {
                        if matches!(agg.func, AggFunc::Count) {
                            if agg.distinct {
                                accums[i].update_distinct(extract_key(arr, row).into())?;
                            } else {
                                accums[i].update_count()?;
                            }
                        } else {
                            accums[i].update_numeric(extract_f64(arr, row)?)?;
                        }
                    }
                }
            }
        }

        if groups.is_empty() && !self.group_by.is_empty() {
            return Ok(Some(RecordBatch::new_empty(self.output_schema.clone())));
        }

        let entries: Vec<(Vec<GroupKey>, Vec<Accumulator>)> = if groups.is_empty() {
            vec![(
                vec![],
                self.aggregates.iter().map(AggregateState::new).collect(),
            )]
        } else {
            groups.into_iter().collect()
        };

        let mut columns: Vec<ArrayRef> = Vec::with_capacity(self.group_by.len() + num_aggs);

        for (gi, col_name) in self.group_by.iter().enumerate() {
            let field = self.output_schema.field_with_name(col_name).unwrap();
            let arr = build_group_column(&entries, gi, field.data_type());
            columns.push(arr);
        }

        for (ai, agg) in self.aggregates.iter().enumerate() {
            match agg.output_type() {
                DataType::UInt64 => {
                    let values: Result<Vec<u64>> = entries
                        .iter()
                        .map(|(_, accums)| accums[ai].count_result())
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
    Utf8(String),
    Float64Bits(u64),
}

impl From<GroupKey> for AggregateValue {
    fn from(value: GroupKey) -> Self {
        match value {
            GroupKey::Null => Self::Null,
            GroupKey::Bool(value) => Self::Bool(value),
            GroupKey::Int32(value) => Self::Int32(value),
            GroupKey::Int64(value) => Self::Int64(value),
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
        DataType::Float64 => {
            GroupKey::Float64Bits(arr.as_primitive::<Float64Type>().value(row).to_bits())
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
    )
}

fn build_group_column(
    entries: &[(Vec<GroupKey>, Vec<Accumulator>)],
    group_index: usize,
    data_type: &DataType,
) -> ArrayRef {
    match data_type {
        DataType::Int32 => {
            let values: Vec<Option<i32>> = entries
                .iter()
                .map(|(keys, _)| match &keys[group_index] {
                    GroupKey::Int32(v) => Some(*v),
                    GroupKey::Null => None,
                    _ => None,
                })
                .collect();
            Arc::new(Int32Array::from(values))
        }
        DataType::Int64 => {
            let values: Vec<Option<i64>> = entries
                .iter()
                .map(|(keys, _)| match &keys[group_index] {
                    GroupKey::Int64(v) => Some(*v),
                    GroupKey::Null => None,
                    _ => None,
                })
                .collect();
            Arc::new(Int64Array::from(values))
        }
        DataType::Float64 => {
            let values: Vec<Option<f64>> = entries
                .iter()
                .map(|(keys, _)| match &keys[group_index] {
                    GroupKey::Float64Bits(bits) => Some(f64::from_bits(*bits)),
                    GroupKey::Null => None,
                    _ => None,
                })
                .collect();
            Arc::new(Float64Array::from(values))
        }
        DataType::Boolean => {
            let values: Vec<Option<bool>> = entries
                .iter()
                .map(|(keys, _)| match &keys[group_index] {
                    GroupKey::Bool(v) => Some(*v),
                    GroupKey::Null => None,
                    _ => None,
                })
                .collect();
            Arc::new(BooleanArray::from(values))
        }
        _ => {
            let values: Vec<Option<&str>> = entries
                .iter()
                .map(|(keys, _)| match &keys[group_index] {
                    GroupKey::Utf8(v) => Some(v.as_str()),
                    GroupKey::Null => None,
                    _ => None,
                })
                .collect();
            Arc::new(StringArray::from(values))
        }
    }
}

fn exec_err(msg: impl Into<String>) -> KaveonError {
    KaveonError::Execution(msg.into())
}

#[cfg(test)]
mod tests {
    use super::*;
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
}
